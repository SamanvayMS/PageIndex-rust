//! expand / optimize with a scripted model.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use pi_llm::{Llm, LlmError, Message};
use pi_optimize::flash::page_lines;
use pi_optimize::tree::Tree;
use pi_optimize::{OptimizeError, OptimizeOptions, optimize};
use serde_json::{Value, json};

/// Replies in order; after the script runs out, repeats the last reply.
struct Script {
    replies: Vec<Result<String, LlmError>>,
    calls: AtomicUsize,
    prompts: Mutex<Vec<String>>,
}

impl Script {
    fn new(replies: Vec<Result<String, LlmError>>) -> Self {
        Script {
            replies,
            calls: AtomicUsize::new(0),
            prompts: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl Llm for Script {
    async fn complete(&self, messages: &[Message]) -> pi_llm::Result<String> {
        let i = self.calls.fetch_add(1, Ordering::SeqCst);
        self.prompts
            .lock()
            .unwrap()
            .push(messages[0].content.clone());
        self.replies[i.min(self.replies.len() - 1)].clone()
    }
}

fn pages() -> Vec<String> {
    (1..=10)
        .map(|p| match p {
            3 => "Alpha Heading\nbody three".to_string(),
            6 => "some text\nBeta Heading\nmore".to_string(),
            _ => format!("plain page {p}"),
        })
        .collect()
}

fn one_node_tree() -> Tree {
    Tree::from_value(&json!([
        {"title": "Intro", "node_id": "0000", "start_index": 1, "end_index": 1},
        {"title": "Big", "node_id": "0001", "start_index": 1, "end_index": 10}
    ]))
    .unwrap()
}

async fn run(llm: &Script) -> Result<(Value, pi_optimize::Outcome), OptimizeError> {
    let pages = pages();
    let lines: Vec<Vec<String>> = pages.iter().map(|p| page_lines(p)).collect();
    let tree = Mutex::new(one_node_tree());
    let finals = Mutex::new(Vec::new());
    let on_final = |nodes: &[usize]| finals.lock().unwrap().extend_from_slice(nodes);
    let opts = OptimizeOptions {
        page_count: Some(10),
        ..Default::default()
    };
    let out = optimize(
        &tree,
        Some(&pages),
        Some(&lines),
        Some(llm),
        &opts,
        Some(&on_final),
    )
    .await?;
    let t = tree.into_inner().unwrap();
    // every live node was reported final at least once
    let finals = finals.into_inner().unwrap();
    for n in t.live() {
        assert!(finals.contains(&n), "node {n} never marked final");
    }
    Ok((t.to_value(), out))
}

#[tokio::test]
async fn empty_reply_is_retried_then_children_attach() {
    let good = r#"```json
{"subsections": [{"title": "Alpha Heading", "page": 3}, {"title": "Beta Heading", "page": 6}, {"title": "Ghost", "page": 7}]}
```"#;
    let llm = Script::new(vec![Ok(r#"{"subsections": []}"#.into()), Ok(good.into())]);
    let (tree, out) = run(&llm).await.unwrap();
    assert_eq!(llm.calls.load(Ordering::SeqCst), 2);
    let prompt = &llm.prompts.lock().unwrap()[0];
    assert!(prompt.starts_with("You are splitting an over-long section of a PDF into its subsections.\n\nSection title: Big\nPages: 1-10\n\n<page_1>\nplain page 1\n</page_1>\n<page_2>"));
    assert_eq!(out.expands, 1);
    assert_eq!(out.rounds, 2);
    // Alpha opens page 3 (first line) -> nothing; Beta is not first on page 6, so Alpha ends on 6
    assert_eq!(
        tree,
        json!([
            {"title": "Intro", "node_id": "0000", "start_index": 1, "end_index": 1},
            {"title": "Big", "node_id": "0001", "start_index": 1, "end_index": 10, "nodes": [
                {"title": "Alpha Heading", "start_index": 3, "end_index": 6, "node_id": "0002"},
                {"title": "Beta Heading", "start_index": 6, "end_index": 10, "node_id": "0003"}
            ]}
        ])
    );
}

#[tokio::test]
async fn no_children_keeps_node_collapsed() {
    let llm = Script::new(vec![Ok(String::new())]);
    let (tree, out) = run(&llm).await.unwrap();
    assert_eq!(llm.calls.load(Ordering::SeqCst), 2); // one retry
    assert_eq!(out.expands, 0);
    assert!(tree[1].get("nodes").is_none());
}

#[tokio::test]
async fn per_prompt_400_is_absorbed_but_401_fails_the_run() {
    let e400 = LlmError::Status {
        status: 400,
        message: "context_length_exceeded".into(),
    };
    let llm = Script::new(vec![Err(e400)]);
    let (_, out) = run(&llm).await.unwrap();
    assert_eq!(out.expands, 0);

    let e401 = LlmError::Status {
        status: 401,
        message: "bad key".into(),
    };
    let llm = Script::new(vec![Err(e401)]);
    assert!(matches!(run(&llm).await, Err(OptimizeError::Llm(_))));
}
