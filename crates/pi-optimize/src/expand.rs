//! EXPAND: one-step lookahead on collapsed nodes over the trigger, children proposed by the
//! model. ref: pageindex/tree_optimize.py:104-127 (helpers), 173-205, 627-769

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::future::{BoxFuture, FutureExt, join_all};
use pi_llm::{Llm, LlmError, complete_prompt};
use pi_pycompat::pystr;
use serde_json::{Map, Value};
use tokio::sync::Semaphore;

use crate::consts::{PAGE_CHARS, expand_prompt};
use crate::cost::{Candidate, expand_cost, s};
use crate::log::{Event, ExpandDecision};
use crate::merge::{Frozen, ListRef, merge_same_page};
use crate::optimize::{OnFinal, OptimizeError, final_nodes};
use crate::tree::{Nid, Node, Tree};

/// `normalize(text)`: lowercase, runs of anything but `[a-z0-9]` to one space, stripped.
/// ref: tree_optimize.py:104
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut gap = false;
    for c in text.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    out
}

/// Python `s[:n]` on code points.
fn prefix_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// Python `seq[i]` for a possibly negative index.
fn py_index<T>(seq: &[T], i: i64) -> Option<&T> {
    let len = seq.len() as i64;
    let j = if i < 0 { len + i } else { i };
    if (0..len).contains(&j) {
        seq.get(j as usize)
    } else {
        None
    }
}

/// The part of a fenced reply after the first ```` ``` ````, as
/// `re.sub(r"^.*?```(?:json)?\s*", "", text, flags=re.S).split("```")[0]`.
pub(crate) fn unfence(text: &str) -> &str {
    let Some(i) = text.find("```") else {
        return text;
    };
    let rest = &text[i + 3..];
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    let rest = rest.trim_start_matches(pystr::is_py_space);
    match rest.find("```") {
        Some(j) => &rest[..j],
        None => rest,
    }
}

/// `extract_json(content)`: the JSON object in a model reply, fenced or not.
/// ref: tree_optimize.py:115
pub fn extract_json(content: &str) -> Result<Value, String> {
    if content.is_empty() {
        return Err("ValueError: model returned no content".into());
    }
    let mut text = pystr::strip(content);
    if text.contains("```") {
        text = unfence(text);
    }
    let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) else {
        let head: String = content.chars().take(200).collect();
        return Err(format!("ValueError: no JSON object in reply: {head:?}"));
    };
    let slice = if end >= start { &text[start..=end] } else { "" };
    serde_json::from_str(slice).map_err(|e| format!("JSONDecodeError: {e}"))
}

/// Python truthiness of a JSON value.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// What `propose_children` needs to know about the node, read under the tree lock.
#[derive(Debug, Clone)]
pub struct ProposeInput {
    pub title: String,
    pub start: i64,
    pub subtree_end: i64,
    /// `node['node_id']`: `None` when the key is missing (a `KeyError` in the reference, raised
    /// once a subsection is accepted), `Some("None")` for a JSON null.
    pub node_id: Option<String>,
}

impl ProposeInput {
    pub fn of(t: &Tree, n: Nid) -> Self {
        let node_id = match t.get(n, "node_id") {
            None => None,
            Some(Value::Null) => Some("None".to_string()),
            Some(Value::String(s)) => Some(s.clone()),
            Some(other) => Some(other.to_string()),
        };
        ProposeInput {
            title: t.title(n).to_string(),
            start: t.start(n),
            subtree_end: t.subtree_end(n),
            node_id,
        }
    }
}

/// A failed proposal attempt.
#[derive(Debug)]
pub enum ProposeError {
    Llm(LlmError),
    /// Parse / shape errors the reference raises as ValueError, AttributeError, ...
    Other(String),
}

/// The prompt `propose_children` sends, or `None` when the span lies beyond the loaded pages.
pub fn propose_prompt(
    node: &ProposeInput,
    pages: &[String],
) -> Result<Option<(String, i64, i64)>, String> {
    let start = node.start;
    let end = node.subtree_end.min(pages.len() as i64);
    if end < start {
        return Ok(None);
    }
    let mut blocks = Vec::new();
    for n in start..=end {
        let page = py_index(pages, n - 1)
            .ok_or_else(|| "IndexError: list index out of range".to_string())?;
        blocks.push(format!(
            "<page_{n}>\n{}\n</page_{n}>",
            prefix_chars(page, PAGE_CHARS)
        ));
    }
    Ok(Some((
        expand_prompt(&node.title, start, end, &blocks.join("\n")),
        start,
        end,
    )))
}

/// Validate a parsed reply into accepted children. ref: tree_optimize.py:638-652
pub fn accept_subsections(
    node: &ProposeInput,
    answer: &Value,
    pages: &[String],
    start: i64,
    end: i64,
) -> Result<Vec<Candidate>, String> {
    let empty = Map::new();
    let answer = answer.as_object().unwrap_or(&empty);
    let subs = answer.get("subsections").cloned().unwrap_or(Value::Null);
    // `answer.get("subsections") or []`, then iterate it the way Python would
    let items: Vec<Value> = if !truthy(&subs) {
        Vec::new()
    } else {
        match subs {
            Value::Array(a) => a,
            Value::Object(o) => o.keys().map(|k| Value::String(k.clone())).collect(),
            Value::String(s) => s.chars().map(|c| Value::String(c.to_string())).collect(),
            other => return Err(format!("TypeError: {other} is not iterable")),
        }
    };
    let node_norm = normalize(&node.title);
    let mut accepted: Vec<Candidate> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for item in items {
        let obj = match &item {
            v if !truthy(v) => Map::new(),
            Value::Object(o) => o.clone(),
            other => return Err(format!("AttributeError: {other} has no attribute 'get'")),
        };
        let title = obj.get("title").cloned().unwrap_or(Value::Null);
        // isinstance(page, int): bool is an int subclass in Python
        let page = match obj.get("page") {
            Some(Value::Number(n)) => n.as_i64(),
            Some(Value::Bool(b)) => Some(*b as i64),
            _ => None,
        };
        let Some(page) = page else { continue };
        if !(start <= page && page <= end && truthy(&title)) {
            continue;
        }
        let Value::String(title) = title else {
            return Err("AttributeError: title has no attribute 'lower'".into());
        };
        let norm = normalize(&title);
        let text = py_index(pages, page - 1)
            .ok_or_else(|| "IndexError: list index out of range".to_string())?;
        if !normalize(text).contains(&norm) {
            continue; // the heading must be printed on that page
        }
        if seen.contains(&norm) || norm == node_norm {
            continue;
        }
        if accepted.last().is_some_and(|last| page < last.start_index) {
            continue;
        }
        seen.push(norm);
        let parent_id = node
            .node_id
            .as_ref()
            .ok_or_else(|| "KeyError: 'node_id'".to_string())?;
        accepted.push(Candidate {
            title: pystr::strip(&title).to_string(),
            start_index: page,
            end_index: end,
            node_id: format!("{parent_id}.{}", accepted.len() + 1),
        });
    }
    Ok(accepted)
}

/// `propose_children(node, pages, args)`: one temporary level of children via the model.
/// ref: tree_optimize.py:627
pub async fn propose_children(
    node: &ProposeInput,
    pages: &[String],
    llm: &dyn Llm,
) -> Result<Vec<Candidate>, ProposeError> {
    let Some((prompt, start, end)) = propose_prompt(node, pages).map_err(ProposeError::Other)?
    else {
        return Ok(Vec::new());
    };
    let reply = complete_prompt(llm, &prompt)
        .await
        .map_err(ProposeError::Llm)?;
    let answer = extract_json(&reply).map_err(ProposeError::Other)?;
    accept_subsections(node, &answer, pages, start, end).map_err(ProposeError::Other)
}

/// `heading_at_page_start(lines, page_no, heading)`. ref: tree_optimize.py:173
pub fn heading_at_page_start(lines: &[Vec<String>], page_no: i64, heading: &str) -> bool {
    match py_index(lines, page_no - 1) {
        Some(page) if !page.is_empty() => normalize(&page[0]).contains(&normalize(heading)),
        _ => false,
    }
}

/// `assign_ends(node, children, lines)`: end = next.start - 1 when the next heading opens its
/// page, else next.start; the last child ends where the node's subtree ends.
/// ref: tree_optimize.py:181
pub fn assign_ends(
    t: &Tree,
    n: Nid,
    children: &[Candidate],
    lines: &[Vec<String>],
) -> Vec<Candidate> {
    let mut sized = children.to_vec();
    let old_end = t.subtree_end(n);
    let len = sized.len();
    for i in 0..len {
        if i + 1 < len {
            let (nxt_start, nxt_title) = (sized[i + 1].start_index, sized[i + 1].title.clone());
            sized[i].end_index = if heading_at_page_start(lines, nxt_start, &nxt_title) {
                sized[i].start_index.max(nxt_start - 1)
            } else {
                nxt_start
            };
        } else {
            sized[i].end_index = old_end;
        }
    }
    sized
}

/// `attach_children(node, children, lines)`: commit a candidate level. ref: tree_optimize.py:200
pub fn attach_children(
    t: &mut Tree,
    n: Nid,
    children: &[Candidate],
    lines: &[Vec<String>],
) -> Vec<Nid> {
    let sized = assign_ends(t, n, children, lines);
    let ids: Vec<Nid> = sized
        .into_iter()
        .map(|c| {
            let mut fields = Map::new();
            fields.insert("title".into(), Value::String(c.title));
            fields.insert("start_index".into(), c.start_index.into());
            fields.insert("end_index".into(), c.end_index.into());
            fields.insert("node_id".into(), Value::String(c.node_id));
            t.push(Node {
                fields,
                children: Vec::new(),
            })
        })
        .collect();
    t.set_children(n, ids.clone());
    ids
}

/// Settings `expand` reads (`args` in the reference).
pub struct ExpandArgs<'a> {
    pub llm: &'a dyn Llm,
    pub routing: i64,
    pub trigger_pages: i64,
    pub min_gain_ratio: f64,
    pub empty_retries: u32,
    pub do_merge: bool,
    pub concurrency: usize,
    pub on_final: Option<&'a OnFinal<'a>>,
}

struct Ctx<'a> {
    tree: &'a Mutex<Tree>,
    pages: &'a [String],
    lines: &'a [Vec<String>],
    args: &'a ExpandArgs<'a>,
    log: &'a Mutex<Vec<Event>>,
    frozen: &'a Mutex<Frozen>,
    sem: Semaphore,
    changed: AtomicBool,
}

impl Ctx<'_> {
    fn settled(&self, n: Nid) {
        if let Some(cb) = self.args.on_final {
            let nodes = {
                let t = self.tree.lock().unwrap();
                let frozen = self.frozen.lock().unwrap();
                final_nodes(&t, &[n], self.args.trigger_pages, &frozen)
            };
            cb(&nodes);
        }
    }

    fn log(&self, e: Event) {
        self.log.lock().unwrap().push(e);
    }

    /// The model half of one node's lookahead (`proposals_for`). ref: tree_optimize.py:666
    async fn proposals_for(
        &self,
        input: &ProposeInput,
    ) -> Result<(Option<Vec<Candidate>>, u32, Vec<Event>), OptimizeError> {
        let mut entries = Vec::new();
        let mut attempts = 0u32;
        while attempts <= self.args.empty_retries {
            attempts += 1;
            let proposed = {
                let _permit = self.sem.acquire().await.expect("semaphore never closed");
                propose_children(input, self.pages, self.args.llm).await
            };
            match proposed {
                Err(ProposeError::Llm(e)) if e.is_unrecoverable() => {
                    return Err(OptimizeError::Llm(e));
                }
                Err(e) => {
                    let detail = match e {
                        ProposeError::Llm(e) => e.to_string(),
                        ProposeError::Other(s) => s,
                    };
                    entries.push(Event::Expand {
                        node_id: input.node_id.clone(),
                        decision: ExpandDecision::Error {
                            attempt: attempts,
                            detail,
                        },
                    });
                    continue;
                }
                Ok(proposed) if !proposed.is_empty() => {
                    return Ok((Some(proposed), attempts, entries));
                }
                Ok(_) => {} // an empty answer is retried, not trusted
            }
        }
        Ok((None, attempts, entries))
    }

    fn process(&self, n: Nid) -> BoxFuture<'_, Result<(), OptimizeError>> {
        async move {
            let (span, input) = {
                let t = self.tree.lock().unwrap();
                let frozen = self.frozen.lock().unwrap();
                if !t.is_frontier(n) || frozen.contains(&t.node_id(n)) {
                    return Ok(());
                }
                (s(&t, n), ProposeInput::of(&t, n))
            };
            if span <= self.args.trigger_pages {
                return Ok(()); // below the trigger, stay collapsed
            }
            let node_id = {
                let t = self.tree.lock().unwrap();
                t.node_id(n)
            };
            let (proposed, attempts, entries) = self.proposals_for(&input).await?;
            self.log.lock().unwrap().extend(entries);
            let Some(proposed) = proposed else {
                self.log(Event::Expand {
                    node_id: node_id.clone(),
                    decision: ExpandDecision::NoChildren { s: span, attempts },
                });
                self.frozen.lock().unwrap().insert(node_id);
                self.settled(n);
                return Ok(());
            };
            let (keep, children) = {
                let t = self.tree.lock().unwrap();
                // one candidate source (the model); the cached per-page detection is not
                // used by the flash pipeline (`cache=None`)
                let sized = assign_ends(&t, n, &proposed, self.lines);
                let (cost, _residual) = expand_cost(&t, n, &sized, self.args.routing);
                let gain = span - cost;
                let ratio = if span != 0 {
                    gain as f64 / span as f64
                } else {
                    0.0
                };
                // strict: at cost == span the next round's merge would fold the node back
                let keep = cost < span && ratio >= self.args.min_gain_ratio;
                let decision = if keep {
                    ExpandDecision::Expand {
                        s: span,
                        expand_cost: cost,
                        children: sized.len(),
                    }
                } else {
                    ExpandDecision::KeepCollapsed {
                        s: span,
                        expand_cost: cost,
                    }
                };
                self.log(Event::Expand {
                    node_id: node_id.clone(),
                    decision,
                });
                (keep, sized)
            };
            self.frozen.lock().unwrap().insert(node_id);
            if !keep {
                self.settled(n);
                return Ok(());
            }
            self.changed.store(true, Ordering::SeqCst);
            let kids = {
                let mut t = self.tree.lock().unwrap();
                attach_children(&mut t, n, &children, self.lines);
                if self.args.do_merge {
                    let mut log = self.log.lock().unwrap();
                    merge_same_page(&mut t, ListRef::Single(n), &mut log);
                }
                t.children(n).to_vec()
            };
            // settle after the fusion: the summaries snapshot the children
            self.settled(n);
            let results = join_all(kids.into_iter().map(|c| self.process(c))).await;
            results.into_iter().collect::<Result<Vec<()>, _>>()?;
            Ok(())
        }
        .boxed()
    }
}

/// `expand(structure, pages, lines, args, log, frozen)`: every collapsed node over the trigger,
/// recursively, concurrently. Returns whether anything was expanded. ref: tree_optimize.py:655
pub async fn expand(
    tree: &Mutex<Tree>,
    pages: &[String],
    lines: &[Vec<String>],
    args: &ExpandArgs<'_>,
    log: &Mutex<Vec<Event>>,
    frozen: &Mutex<Frozen>,
) -> Result<bool, OptimizeError> {
    let ctx = Ctx {
        tree,
        pages,
        lines,
        args,
        log,
        frozen,
        sem: Semaphore::new(args.concurrency.max(1)),
        changed: AtomicBool::new(false),
    };
    let all = {
        let t = tree.lock().unwrap();
        t.live()
    };
    let results = join_all(all.into_iter().map(|n| ctx.process(n))).await;
    results.into_iter().collect::<Result<Vec<()>, _>>()?;
    Ok(ctx.changed.load(Ordering::SeqCst))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalize_matches_reference() {
        assert_eq!(
            normalize("  1.2 Types and Values!  "),
            "1 2 types and values"
        );
        assert_eq!(normalize("Été — Überblick"), "t berblick");
        assert_eq!(normalize(""), "");
        assert_eq!(normalize("---"), "");
        assert_eq!(normalize("İstanbul"), "i stanbul");
    }

    #[test]
    fn extract_json_handles_fences() {
        assert_eq!(
            extract_json("```json\n{\"a\": 1}\n```").unwrap(),
            json!({"a": 1})
        );
        assert_eq!(
            extract_json("noise {\"a\": [1]} tail").unwrap(),
            json!({"a": [1]})
        );
        assert!(extract_json("").is_err());
        assert!(extract_json("no object").is_err());
        assert!(extract_json("} {").is_err());
        assert_eq!(extract_json("```\n{\"b\":2}```x").unwrap(), json!({"b": 2}));
    }

    fn input() -> ProposeInput {
        ProposeInput {
            title: "Chapter".into(),
            start: 1,
            subtree_end: 3,
            node_id: Some("0007".into()),
        }
    }

    #[test]
    fn accept_filters_like_reference() {
        let pages: Vec<String> = vec![
            "Chapter\nIntro text".into(),
            "1.1 Alpha\nbody".into(),
            "1.2 Beta\nbody 1.3 Gamma".into(),
        ];
        let ans = json!({"subsections": [
            {"title": "1.2 Beta", "page": 3},
            {"title": "1.1 Alpha", "page": 2},      // goes backwards: dropped
            {"title": "Missing", "page": 3},        // not on the page
            {"title": "1.2 beta", "page": 3},       // duplicate after normalize
            {"title": "Chapter", "page": 1},        // the node's own title
            {"title": " 1.3 Gamma ", "page": 3},
            {"title": "x", "page": 9},              // out of range
            {"title": "1.1 Alpha", "page": 2.0},    // not an int
            null, 0
        ]});
        let got = accept_subsections(&input(), &ans, &pages, 1, 3).unwrap();
        let titles: Vec<_> = got
            .iter()
            .map(|c| (c.title.as_str(), c.start_index, c.node_id.as_str()))
            .collect();
        assert_eq!(
            titles,
            vec![("1.2 Beta", 3, "0007.1"), ("1.3 Gamma", 3, "0007.2")]
        );
        assert!(
            accept_subsections(&input(), &json!({"subsections": ["s"]}), &pages, 1, 3).is_err()
        );
        assert!(accept_subsections(&input(), &json!({"subsections": 3}), &pages, 1, 3).is_err());
        assert!(
            accept_subsections(
                &input(),
                &json!({"subsections": [{"title": 5, "page": 2}]}),
                &pages,
                1,
                3
            )
            .is_err()
        );
        assert!(
            accept_subsections(&input(), &json!({}), &pages, 1, 3)
                .unwrap()
                .is_empty()
        );
    }
}
