//! Deterministic merge passes and relabelling. ref: pageindex/tree_optimize.py:470-591, 208

use std::collections::HashSet;

use indexmap::IndexMap;
use serde_json::Value;

use crate::consts::{NODE_ID_WIDTH, TITLE_MAX_CHARS};
use crate::cost::{s, tree_cost};
use crate::log::Event;
use crate::tree::{Nid, Tree};

/// Frozen node ids (`frozen` in the reference; `None` stands for a node without an id).
pub type Frozen = HashSet<Option<String>>;

/// `page_label(node)`: `p.X` or `p.X-Y`. ref: tree_optimize.py:470
pub fn page_label(t: &Tree, n: Nid) -> String {
    let (start, end) = (t.start(n), t.subtree_end(n));
    if start == end {
        format!("p.{start}")
    } else {
        format!("p.{start}-{end}")
    }
}

/// `union_title(titles, node)`. ref: tree_optimize.py:476
pub fn union_title(t: &Tree, titles: &[String], n: Nid) -> String {
    let joined = titles
        .iter()
        .filter(|s| !s.is_empty())
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("; ");
    if joined.is_empty() || joined.chars().count() > TITLE_MAX_CHARS {
        return page_label(t, n);
    }
    joined
}

fn strings(v: &[String]) -> Value {
    Value::Array(v.iter().cloned().map(Value::String).collect())
}

/// `merge_same_page(structure, log)`: collapse frontier siblings covering exactly the same
/// pages. `nodes` is the list visited (the whole structure, or `[node]` after an expand).
/// Returns whether anything changed. ref: tree_optimize.py:492
pub fn merge_same_page(t: &mut Tree, list: ListRef, log: &mut Vec<Event>) -> bool {
    let mut changed = false;
    visit_same_page(t, list, log, &mut changed);
    changed
}

/// Which sibling list a pass operates on.
#[derive(Debug, Clone, Copy)]
pub enum ListRef {
    Roots,
    /// A single-element list `[node]` that is not stored anywhere (`merge_same_page([node])`).
    Single(Nid),
    Children(Nid),
}

fn list_of(t: &Tree, list: ListRef) -> Vec<Nid> {
    match list {
        ListRef::Roots => t.roots.clone(),
        ListRef::Single(n) => vec![n],
        ListRef::Children(n) => t.children(n).to_vec(),
    }
}

fn remove_from(t: &mut Tree, list: ListRef, dropped: &[Nid]) {
    let keep = |v: &mut Vec<Nid>| v.retain(|n| !dropped.contains(n));
    match list {
        ListRef::Roots => keep(&mut t.roots),
        ListRef::Single(_) => {} // a temporary list: removing from it has no effect on the tree
        ListRef::Children(p) => keep(&mut t.node_mut(p).children),
    }
}

fn visit_same_page(t: &mut Tree, list: ListRef, log: &mut Vec<Event>, changed: &mut bool) {
    let nodes = list_of(t, list);
    let mut groups: IndexMap<(i64, i64), Vec<Nid>> = IndexMap::new();
    for &n in &nodes {
        if !t.children(n).is_empty() {
            visit_same_page(t, ListRef::Children(n), log, changed);
        }
        if t.is_frontier(n) {
            groups
                .entry((t.start(n), t.subtree_end(n)))
                .or_default()
                .push(n);
        }
    }
    for (span, group) in groups.into_iter() {
        if group.len() < 2 {
            continue;
        }
        let (keeper, dropped) = (group[0], group[1..].to_vec());
        let mut titles = Vec::new();
        for &n in &group {
            titles.push(t.title(n).to_string());
            titles.extend(t.key_items(n));
        }
        log.push(Event::MergeSamePage {
            node_id: t.node_id(keeper),
            pages: span,
            dropped: dropped.len(),
        });
        t.set(keeper, "key_items", strings(&titles));
        let title = union_title(t, &titles, keeper);
        t.set(keeper, "title", Value::String(title));
        t.set(keeper, "_same_page", Value::Bool(true));
        remove_from(t, list, &dropped);
        *changed = true;
    }
}

/// `merge(structure, routing, log, frozen)`: bottom-up, collapse every subtree whose structure
/// does not beat a linear scan (`S(v) <= tree_cost(v)`, ties merge). ref: tree_optimize.py:535
pub fn merge(t: &mut Tree, routing: i64, log: &mut Vec<Event>, frozen: &mut Frozen) -> bool {
    let mut changed = false;
    for root in t.roots.clone() {
        visit_merge(t, root, routing, log, frozen, &mut changed);
    }
    changed
}

fn visit_merge(
    t: &mut Tree,
    n: Nid,
    routing: i64,
    log: &mut Vec<Event>,
    frozen: &mut Frozen,
    changed: &mut bool,
) {
    if t.is_frontier(n) {
        return;
    }
    for c in t.children(n).to_vec() {
        visit_merge(t, c, routing, log, frozen, changed);
    }
    if t.is_frontier(n) {
        return;
    }
    let cost = tree_cost(t, n, routing);
    let span = s(t, n);
    if span <= cost {
        let below = t.flatten(t.children(n));
        let mut titles = Vec::new();
        for &c in &below {
            titles.push(t.title(c).to_string());
            titles.extend(t.key_items(c));
        }
        log.push(Event::Merge {
            node_id: t.node_id(n),
            s: span,
            tree_cost: cost,
            removed: below.len(),
        });
        let end = t.subtree_end(n);
        t.set(n, "end_index", end.into());
        t.pop(n, crate::tree::NODES);
        if !titles.is_empty() {
            t.set(n, "key_items", strings(&titles));
        }
        frozen.insert(t.node_id(n));
        *changed = true;
    }
}

/// `merge_tree(structure)`: the deterministic no-LLM merge. ref: tree_optimize.py:584
pub fn merge_tree(t: &mut Tree) {
    merge(
        t,
        crate::consts::ROUTING_COST,
        &mut Vec::new(),
        &mut Frozen::new(),
    );
}

/// `relabel(structure)`: renumber node ids in pre-order, `0000`, `0001`, ... Returns the
/// old -> new mapping. ref: tree_optimize.py:208
pub fn relabel(t: &mut Tree) -> Vec<(String, String)> {
    let mut mapping = Vec::new();
    for (counter, n) in t.live().into_iter().enumerate() {
        let new = format!("{counter:0width$}", width = NODE_ID_WIDTH);
        if let Some(old) = t.node_id(n) {
            mapping.push((old, new.clone()));
        }
        t.set(n, "node_id", Value::String(new));
    }
    mapping
}
