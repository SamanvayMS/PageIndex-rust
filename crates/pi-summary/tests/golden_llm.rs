//! page_index_flash(summary=True, optimize="full") parity on the stage-09 tree of golden docs:
//! - with `StubLlm` against `10b_tree_full_llm.json` dumped by
//!   `parity/dump_reference.py --llm stub` (fixtures/<doc>/10b_stub.json);
//! - with a replay of the prompt-dependent fake recorded by `gen/fake_llm_golden.py`
//!   (fixtures/<doc>/10b_fake.json + fake_llm.jsonl), asserting every fixture key is hit and
//!   no prompt misses.
//!
//! Inputs (09 tree, page texts, doc title) come from env `PI_GOLDEN`, default
//! `/home/user/PageIndex-rust/parity/golden`; skipped when missing.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pi_llm::{Llm, ReplayLlm, StubLlm};
use pi_summary::{FlashOptions, OptimizeMode, page_index_flash_post};
use serde_json::{Map, Value};

const MODEL: &str = "openai/mock"; // dump_reference.full_llm's model name

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn golden_dir() -> Option<PathBuf> {
    let root = std::env::var_os("PI_GOLDEN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/user/PageIndex-rust/parity/golden"));
    if root.is_dir() {
        Some(root)
    } else {
        eprintln!("golden dir {} missing; skipping", root.display());
        None
    }
}

fn load(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

/// The `extract_toc` result as it reaches page_index_flash, rebuilt from the stage dumps.
fn extract_toc_result(dir: &Path) -> Map<String, Value> {
    let s09 = load(&dir.join("09_tree_bookmarks.json"));
    let mut r = Map::new();
    r.insert("doc_name".into(), s09["doc"].clone());
    r.insert(
        "doc_title".into(),
        load(&dir.join("05_classified.json"))["data"]["doc_title"].clone(),
    );
    r.insert("structure".into(), s09["data"]["structure"].clone());
    r.insert(
        "has_abstract_or_references_section".into(),
        load(&dir.join("07_outline.json"))["data"]["has_abstract_or_references"].clone(),
    );
    r.insert(
        "page_texts".into(),
        load(&dir.join("page_texts.json"))["data"].clone(),
    );
    r.insert("toc_source".into(), s09["data"]["toc_source"].clone());
    r
}

/// JSON equality: key order, number kinds and strings exact; floats within 1e-9.
fn diff(a: &Value, b: &Value, path: &str) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let kx: Vec<_> = x.keys().collect();
            let ky: Vec<_> = y.keys().collect();
            if kx != ky {
                return Some(format!("{path}: keys {kx:?} != {ky:?}"));
            }
            x.iter()
                .find_map(|(k, v)| diff(v, &y[k], &format!("{path}.{k}")))
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Some(format!("{path}: len {} != {}", x.len(), y.len()));
            }
            x.iter()
                .zip(y)
                .enumerate()
                .find_map(|(i, (p, q))| diff(p, q, &format!("{path}[{i}]")))
        }
        (Value::Number(x), Value::Number(y)) => {
            if x.is_f64() != y.is_f64() {
                return Some(format!("{path}: number kind {x} vs {y}"));
            }
            if x.is_f64() {
                let (p, q) = (x.as_f64().unwrap(), y.as_f64().unwrap());
                ((p - q).abs() > 1e-9).then(|| format!("{path}: {p} != {q}"))
            } else {
                (x != y).then(|| format!("{path}: {x} != {y}"))
            }
        }
        _ => (a != b).then(|| format!("{path}: {a} != {b}")),
    }
}

async fn run(dir: &Path, llm: Arc<dyn Llm>, concurrency: Option<usize>) -> Value {
    let opts = FlashOptions {
        summary: true,
        optimize: OptimizeMode::Full,
        concurrency,
        max_words: None,
        summary_model: Some(MODEL.into()),
        summary_llm: Some(llm),
        optimize_llm: None,
    };
    Value::Object(
        page_index_flash_post(extract_toc_result(dir), &opts)
            .await
            .unwrap(),
    )
}

fn docs_with(file: &str) -> Vec<String> {
    let mut docs: Vec<String> = std::fs::read_dir(fixtures())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join(file).is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    docs.sort();
    docs
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stub_llm_matches_reference() {
    let Some(golden) = golden_dir() else { return };
    let docs = docs_with("10b_stub.json");
    assert!(docs.len() >= 3);
    for doc in docs {
        let want = load(&fixtures().join(&doc).join("10b_stub.json"));
        let got = run(&golden.join(&doc), Arc::new(StubLlm), None).await;
        if let Some(d) = diff(&got, &want, "$") {
            panic!("{doc}: {d}");
        }
        eprintln!("PASS stub {doc}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fake_llm_replay_matches_reference() {
    let Some(golden) = golden_dir() else { return };
    let docs = docs_with("fake_llm.jsonl");
    assert!(docs.len() >= 3);
    for doc in docs {
        let want = load(&fixtures().join(&doc).join("10b_fake.json"));
        // scheduling must not matter: a serial lane and the default fan-out give the same tree
        for concurrency in [None, Some(1), Some(3)] {
            let replay = Arc::new(
                ReplayLlm::from_path(fixtures().join(&doc).join("fake_llm.jsonl")).unwrap(),
            );
            let got = run(&golden.join(&doc), replay.clone(), concurrency).await;
            assert!(
                replay.misses().is_empty(),
                "{doc}: prompts missing from the fixture: {:?}",
                replay.misses()
            );
            assert!(
                replay.unused_keys().is_empty(),
                "{doc}: fixture keys never requested: {:?}",
                replay.unused_keys()
            );
            if let Some(d) = diff(&got, &want, "$") {
                panic!("{doc} (concurrency {concurrency:?}): {d}");
            }
        }
        eprintln!("PASS fake {doc}");
    }
}
