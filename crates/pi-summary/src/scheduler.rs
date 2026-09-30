//! Bottom-up summaries, taking nodes as they are marked final.
//! ref: pageindex/utils.py:770-1099 (`_PriorityGate`, `get_intro_text`, `SummaryScheduler`,
//! `summarize_tree`)
//!
//! Each node gets a task that waits for its mark (its children will not change any more), then
//! for its children's tasks, then makes its own call. The result is independent of scheduling:
//! a node's prompt reads only its own pages and its (finished) children's titles/summaries.

use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::{BoxFuture, FutureExt, Shared, join_all};
use pi_llm::{Llm, LlmError, complete_prompt, count_tokens, pyjson};
use pi_optimize::expand::truthy;
use pi_optimize::tree::{Nid, Tree};
use pi_pycompat::pystr;
use serde_json::{Map, Value};
use tokio::sync::{oneshot, watch};

use crate::consts::{
    SUMMARY_CONCURRENCY, SUMMARY_INTRO_MAX_PAGES, SUMMARY_MAX_WORDS, SUMMARY_RAW_TEXT_TOKENS,
};
use crate::parse::{parse_summary, parse_title};
use crate::prompts;
use crate::pyrepr::py_str;

#[derive(Debug, Clone, thiserror::Error)]
pub enum SummaryError {
    /// An unrecoverable model error (`_is_unrecoverable`).
    #[error(transparent)]
    Llm(#[from] LlmError),
    /// `finish()` found a node dropped or changed after it was marked final.
    #[error("node {0:?} was dropped or changed after it was marked final")]
    BrokenPromise(String),
    #[error("{0} node(s) were never marked final; their summaries would wait forever")]
    Undecided(usize),
    #[error(
        "Summary generation failed for all nodes (every summary call failed or returned empty; \
         check the model and its context limits)"
    )]
    AllFailed,
}

// --------------------------------------------------------------------------------------------
// _PriorityGate (utils.py:770)
// --------------------------------------------------------------------------------------------

struct Waiter {
    neg_prio: i64,
    seq: u64,
    tx: oneshot::Sender<()>,
}

impl PartialEq for Waiter {
    fn eq(&self, o: &Self) -> bool {
        (self.neg_prio, self.seq) == (o.neg_prio, o.seq)
    }
}
impl Eq for Waiter {}
impl PartialOrd for Waiter {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Waiter {
    // BinaryHeap is a max-heap; the reference's heapq pops the smallest (-prio, seq)
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        (o.neg_prio, o.seq).cmp(&(self.neg_prio, self.seq))
    }
}

struct GateState {
    free: usize,
    waiters: BinaryHeap<Waiter>,
    seq: u64,
}

/// Semaphore that admits the highest-priority waiter first, FIFO within a priority.
pub struct PriorityGate {
    st: Mutex<GateState>,
}

pub struct GatePermit<'a> {
    gate: &'a PriorityGate,
}

impl Drop for GatePermit<'_> {
    fn drop(&mut self) {
        self.gate.release();
    }
}

/// Releases a permit granted to a waiter that stopped waiting before it saw the grant.
struct PendingGuard<'a> {
    gate: &'a PriorityGate,
    rx: Option<oneshot::Receiver<()>>,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        if let Some(mut rx) = self.rx.take()
            && rx.try_recv().is_ok()
        {
            self.gate.release(); // granted while cancelling: pass the permit on
        }
    }
}

impl PriorityGate {
    pub fn new(permits: usize) -> Self {
        PriorityGate {
            st: Mutex::new(GateState {
                free: permits.max(1),
                waiters: BinaryHeap::new(),
                seq: 0,
            }),
        }
    }

    pub async fn acquire(&self, prio: i64) -> GatePermit<'_> {
        let rx = {
            let mut st = self.st.lock().unwrap();
            if st.free > 0 && st.waiters.is_empty() {
                st.free -= 1;
                return GatePermit { gate: self };
            }
            let (tx, rx) = oneshot::channel();
            let seq = st.seq;
            st.seq += 1;
            st.waiters.push(Waiter {
                neg_prio: -prio,
                seq,
                tx,
            });
            rx
        };
        let mut guard = PendingGuard {
            gate: self,
            rx: Some(rx),
        };
        let rx = guard.rx.as_mut().unwrap();
        // the sender is only dropped after send (release) or never; a closed channel cannot
        // happen while the gate lives
        let _ = rx.await;
        guard.rx = None;
        GatePermit { gate: self }
    }

    fn release(&self) {
        let mut st = self.st.lock().unwrap();
        while let Some(w) = st.waiters.pop() {
            if w.tx.send(()).is_ok() {
                return; // the permit moves straight to this waiter
            }
        }
        st.free += 1;
    }
}

// --------------------------------------------------------------------------------------------
// page text helpers
// --------------------------------------------------------------------------------------------

/// `get_text_of_pdf_pages(pdf_pages, start, end)` (utils.py:565): Python indexing, so a page
/// past the end is an `IndexError`.
pub fn text_of_pages(pages: &[String], start: i64, end: i64) -> Result<String, String> {
    let mut text = String::new();
    let len = pages.len() as i64;
    for page_num in (start - 1)..end {
        let j = if page_num < 0 {
            len + page_num
        } else {
            page_num
        };
        if !(0..len).contains(&j) {
            return Err("IndexError: list index out of range".into());
        }
        text.push_str(&pages[j as usize]);
    }
    Ok(text)
}

/// `get_intro_text(node, pdf_pages, max_pages)` (utils.py:811): pages of the node before its
/// first child starts.
pub fn intro_text(t: &Tree, n: Nid, pages: &[String], max_pages: usize) -> Result<String, String> {
    let children = t.children(n);
    let Some(&first) = children.first() else {
        return Ok(String::new());
    };
    let Some(first) = t.get(first, "start_index").and_then(Value::as_i64) else {
        return Ok(String::new());
    };
    let start = t.start(n);
    if first <= start {
        return Ok(String::new());
    }
    let end = (first - 1).min(start + max_pages as i64 - 1);
    text_of_pages(pages, start, end)
}

// --------------------------------------------------------------------------------------------
// SummaryScheduler (utils.py:895)
// --------------------------------------------------------------------------------------------

type TaskResult = Result<(), LlmError>;
type SharedTask = Shared<BoxFuture<'static, TaskResult>>;

/// Settings of a summary pass.
#[derive(Debug, Clone)]
pub struct SummaryOptions {
    /// The model name, used only to pick the tokenizer for `count_tokens`.
    pub model: Option<String>,
    pub small_node_tokens: usize,
    pub max_intro_pages: usize,
    pub concurrency: Option<usize>,
    pub max_words: Option<usize>,
}

impl Default for SummaryOptions {
    fn default() -> Self {
        SummaryOptions {
            model: None,
            small_node_tokens: SUMMARY_RAW_TEXT_TOKENS,
            max_intro_pages: SUMMARY_INTRO_MAX_PAGES,
            concurrency: None,
            max_words: None,
        }
    }
}

#[derive(Default)]
struct State {
    marks: HashMap<Nid, watch::Sender<bool>>,
    tasks: HashMap<Nid, SharedTask>,
    handles: Vec<tokio::task::AbortHandle>,
    finals: Vec<(Nid, Vec<Nid>)>,
}

struct Inner {
    tree: Arc<Mutex<Tree>>,
    pages: Arc<Vec<String>>,
    llm: Arc<dyn Llm>,
    model: Option<String>,
    small_node_tokens: usize,
    max_intro_pages: usize,
    max_words: usize,
    gate: PriorityGate,
    asked: AtomicBool,
    answered: AtomicBool,
    state: Mutex<State>,
}

/// A failure inside one node's summary call.
enum Fail {
    Llm(LlmError),
    /// Any other exception (e.g. an `IndexError` on the page list): absorbed.
    Other,
}

impl From<LlmError> for Fail {
    fn from(e: LlmError) -> Self {
        Fail::Llm(e)
    }
}

pub struct SummaryScheduler {
    inner: Arc<Inner>,
}

impl Drop for SummaryScheduler {
    fn drop(&mut self) {
        // a run abandoned mid-way (e.g. expand failed) leaves tasks waiting for marks
        if let Ok(st) = self.inner.state.lock() {
            for h in &st.handles {
                h.abort();
            }
        }
    }
}

impl SummaryScheduler {
    pub fn new(
        tree: Arc<Mutex<Tree>>,
        pages: Arc<Vec<String>>,
        llm: Arc<dyn Llm>,
        opts: &SummaryOptions,
    ) -> Self {
        let inner = Inner {
            tree,
            pages,
            llm,
            model: opts.model.clone(),
            small_node_tokens: opts.small_node_tokens,
            max_intro_pages: opts.max_intro_pages,
            max_words: opts
                .max_words
                .filter(|&w| w != 0)
                .unwrap_or(SUMMARY_MAX_WORDS),
            gate: PriorityGate::new(
                opts.concurrency
                    .filter(|&c| c != 0)
                    .unwrap_or(SUMMARY_CONCURRENCY),
            ),
            asked: AtomicBool::new(false),
            answered: AtomicBool::new(false),
            state: Mutex::new(State::default()),
        };
        SummaryScheduler {
            inner: Arc::new(inner),
        }
    }

    /// `mark_final(nodes)`: these nodes will not gain, lose or swap children; their summaries
    /// may start. Their subtrees get tasks, deepest node first. Must run inside a tokio runtime.
    pub fn mark_final(&self, nodes: &[Nid]) {
        mark_final(&self.inner, nodes);
    }

    /// `finish()`: wait for every summary; fails loud if the model never answered. Strips the
    /// internal keys on success.
    pub async fn finish(&self) -> Result<(), SummaryError> {
        let inner = &self.inner;
        let (roots, undecided) = {
            let t = inner.tree.lock().unwrap();
            let st = inner.state.lock().unwrap();
            let live: HashSet<Nid> = t.live().into_iter().collect();
            for (n, children) in &st.finals {
                if !live.contains(n) || t.children(*n) != children.as_slice() {
                    let name = [t.get(*n, "title"), t.get(*n, "node_id")]
                        .into_iter()
                        .flatten()
                        .find(|v| truthy(v))
                        .map(py_str)
                        .unwrap_or_else(|| "?".into());
                    return Err(SummaryError::BrokenPromise(name));
                }
            }
            let undecided = live
                .iter()
                .filter(|n| !st.marks.get(n).is_some_and(|m| *m.borrow()))
                .count();
            (t.roots.clone(), undecided)
        };
        if undecided != 0 {
            return Err(SummaryError::Undecided(undecided));
        }
        let tasks: Vec<SharedTask> = roots.iter().map(|&r| task(inner, r, 1)).collect();
        for r in join_all(tasks).await {
            if let Err(e) = r
                && e.is_unrecoverable()
            {
                return Err(e.into());
            }
        }
        let mut t = inner.tree.lock().unwrap();
        let any_summary = t
            .live()
            .iter()
            .any(|&n| t.get(n, "summary").is_some_and(truthy));
        if (inner.asked.load(Ordering::SeqCst) && !inner.answered.load(Ordering::SeqCst))
            || !any_summary
        {
            return Err(SummaryError::AllFailed);
        }
        t.strip_internal_keys();
        Ok(())
    }
}

fn mark_sender(inner: &Inner, st: &mut State, n: Nid) -> watch::Sender<bool> {
    let _ = inner;
    st.marks
        .entry(n)
        .or_insert_with(|| watch::channel(false).0)
        .clone()
}

fn mark_final(inner: &Arc<Inner>, nodes: &[Nid]) {
    let order = {
        let t = inner.tree.lock().unwrap();
        let mut st = inner.state.lock().unwrap();
        for &n in nodes {
            let mark = mark_sender(inner, &mut st, n);
            if !*mark.borrow() {
                mark.send_replace(true);
                st.finals.push((n, t.children(n).to_vec()));
            }
        }
        let marked: HashSet<Nid> = nodes.iter().copied().collect();
        let mut order: Vec<(i64, Nid)> = Vec::new();
        fn walk(
            t: &Tree,
            nodes: &[Nid],
            depth: i64,
            inside: bool,
            marked: &HashSet<Nid>,
            order: &mut Vec<(i64, Nid)>,
        ) {
            for &n in nodes {
                let here = inside || marked.contains(&n);
                if here {
                    order.push((depth, n));
                }
                walk(t, t.children(n), depth + 1, here, marked, order);
            }
        }
        walk(&t, &t.roots, 1, false, &marked, &mut order);
        order.sort_by_key(|&(d, _)| -d); // stable, deepest first
        order
    };
    for (depth, n) in order {
        drop(task(inner, n, depth)); // create it; the spawned task runs on its own
    }
}

/// `_task(node, depth)`: the node's summary task, created on first request.
fn task(inner: &Arc<Inner>, n: Nid, depth: i64) -> SharedTask {
    let mut st = inner.state.lock().unwrap();
    if let Some(t) = st.tasks.get(&n) {
        return t.clone();
    }
    let handle = tokio::spawn(visit(inner.clone(), n, depth));
    st.handles.push(handle.abort_handle());
    let shared = async move {
        match handle.await {
            Ok(r) => r,
            Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
            Err(_) => Err(LlmError::Other("summary task cancelled".into())),
        }
    }
    .boxed()
    .shared();
    st.tasks.insert(n, shared.clone());
    shared
}

/// `_visit(node, depth)`. ref: utils.py:1025
async fn visit(inner: Arc<Inner>, n: Nid, depth: i64) -> TaskResult {
    let mut rx = {
        let mut st = inner.state.lock().unwrap();
        mark_sender(&inner, &mut st, n).subscribe()
    };
    let _ = rx.wait_for(|done| *done).await;
    let children = inner.tree.lock().unwrap().children(n).to_vec();
    if !children.is_empty() {
        let tasks: Vec<SharedTask> = children
            .iter()
            .map(|&c| task(&inner, c, depth + 1))
            .collect();
        for r in join_all(tasks).await {
            if let Err(e) = r
                && e.is_unrecoverable()
            {
                return Err(e);
            }
        }
    }
    if inner
        .tree
        .lock()
        .unwrap()
        .get(n, "summary")
        .is_some_and(truthy)
    {
        return Ok(());
    }
    let res = if children.is_empty() {
        leaf_summary(&inner, n, depth).await
    } else {
        parent_summary(&inner, n, depth).await
    };
    let (summary, err) = match res {
        Ok(s) => (s, None),
        Err(Fail::Llm(e)) => (String::new(), e.is_unrecoverable().then_some(e)),
        Err(Fail::Other) => (String::new(), None),
    };
    inner
        .tree
        .lock()
        .unwrap()
        .set(n, "summary", Value::String(summary));
    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// `_ask(prompt, prio)`. ref: utils.py:955
async fn ask(inner: &Inner, prompt: &str, prio: i64) -> Result<String, LlmError> {
    inner.asked.store(true, Ordering::SeqCst);
    let reply = {
        let _slot = inner.gate.acquire(prio).await;
        complete_prompt(inner.llm.as_ref(), prompt).await?
    };
    if !reply.is_empty() {
        inner.answered.store(true, Ordering::SeqCst);
    }
    Ok(reply)
}

/// `_leaf_summary(node, prio)`. ref: utils.py:963
async fn leaf_summary(inner: &Inner, n: Nid, prio: i64) -> Result<String, Fail> {
    let (text, retitle, titles) = {
        let t = inner.tree.lock().unwrap();
        let text = text_of_pages(&inner.pages, t.start(n), t.end(n)).map_err(|_| Fail::Other)?;
        let retitle = t.get(n, "_same_page").is_some_and(truthy);
        (text, retitle, t.key_items(n).join("; "))
    };
    if count_tokens(&text, inner.model.as_deref()) < inner.small_node_tokens {
        return Ok(pystr::strip(&text).to_string());
    }
    let prompt = prompts::leaf_summary(inner.max_words, &text, retitle.then_some(titles.as_str()));
    let reply = ask(inner, &prompt, prio).await?;
    if retitle {
        let written = parse_title(&reply);
        if !written.is_empty() {
            inner
                .tree
                .lock()
                .unwrap()
                .set(n, "title", Value::String(written));
        }
    }
    Ok(parse_summary(&reply))
}

/// `_parent_summary(node, prio)`. ref: utils.py:1000
async fn parent_summary(inner: &Inner, n: Nid, prio: i64) -> Result<String, Fail> {
    let prompt = {
        let t = inner.tree.lock().unwrap();
        let intro =
            intro_text(&t, n, &inner.pages, inner.max_intro_pages).map_err(|_| Fail::Other)?;
        let listing: Vec<Value> = t
            .children(n)
            .iter()
            .map(|&c| {
                let mut m = Map::new();
                m.insert(
                    "title".into(),
                    t.get(c, "title").cloned().unwrap_or_else(|| "".into()),
                );
                m.insert(
                    "summary".into(),
                    t.get(c, "summary").cloned().unwrap_or_else(|| "".into()),
                );
                Value::Object(m)
            })
            .collect();
        let title = t.get(n, "title").map(py_str).unwrap_or_default();
        prompts::parent_summary(inner.max_words, &title, &intro, &pyjson::dumps(&listing))
    };
    Ok(parse_summary(&ask(inner, &prompt, prio).await?))
}

/// `summarize_tree(structure, pdf_pages, ...)`: bottom-up summaries over a finished tree.
/// ref: utils.py:1082
pub async fn summarize_tree(
    tree: Arc<Mutex<Tree>>,
    pages: Arc<Vec<String>>,
    llm: Arc<dyn Llm>,
    opts: &SummaryOptions,
) -> Result<(), SummaryError> {
    let scheduler = SummaryScheduler::new(tree.clone(), pages, llm, opts);
    let all = tree.lock().unwrap().live();
    scheduler.mark_final(&all);
    scheduler.finish().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn gate_admits_highest_priority_first() {
        let gate = Arc::new(PriorityGate::new(1));
        let order = Arc::new(Mutex::new(Vec::new()));
        let first = gate.acquire(0).await;
        let mut handles = Vec::new();
        for (i, prio) in [(0, 1), (1, 3), (2, 3), (3, 2)] {
            let (g, o) = (gate.clone(), order.clone());
            handles.push(tokio::spawn(async move {
                let _p = g.acquire(prio).await;
                o.lock().unwrap().push(i);
            }));
            // make sure each waiter is queued before the next (FIFO within a priority)
            while gate.st.lock().unwrap().waiters.len() < i + 1 {
                tokio::task::yield_now().await;
            }
        }
        drop(first);
        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(*order.lock().unwrap(), vec![1, 2, 3, 0]);
    }

    #[test]
    fn page_text_uses_python_indexing() {
        let pages: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(text_of_pages(&pages, 1, 3).unwrap(), "abc");
        assert_eq!(text_of_pages(&pages, 3, 2).unwrap(), "");
        assert!(text_of_pages(&pages, 2, 4).is_err());
        assert_eq!(text_of_pages(&pages, 0, 1).unwrap(), "ca");
    }
}
