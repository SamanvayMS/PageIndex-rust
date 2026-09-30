//! Outline assembly, the post-assembly gates' helpers, and the dict-tree conversion.
//!
//! ref: pageindex/flash/outline_assembly/assembly.py

use pi_layout::model::char_stats::trim_unicode_ws;
use pi_layout::model::rects::tuple_lt;
use serde_json::{Map, Value, json};

use super::cliques::{
    CliqueFilterContext, depth_comparator, detect_body_headings, find_keyword_clique,
    interleave_clusters, partition_candidates,
};
use super::selection::{
    HierarchyStack, extract_sub_headings, find_parent_heading, push_heading_to_state,
    should_reject_heading,
};
use super::style_context::{Cluster, OutlineState};
use crate::heading_detection::page_scan::build_doc_heading_candidates;
use crate::model::{Cand, Doc, HeadingCandidate, Node, heading_order_key, new_node};

/// Retype outline blocks as numbered (8) or unnumbered (7) headings.
// ref: outline_assembly/assembly.py::mark_outline_block_types
pub fn mark_outline_block_types(d: Doc, nodes: &[Node]) {
    for n in nodes {
        d.cblock(&n.heading)
            .kind
            .set(if n.heading.has_numbering { 8 } else { 7 });
        mark_outline_block_types(d, &n.children.borrow());
    }
}

/// `{"max_gap", "last_page_position"}`; `max_gap` is `None` while it is still the int `0` the
/// reference starts from (it only becomes a float once a positive gap is seen).
// ref: outline_assembly/assembly.py::compute_max_heading_gap
pub fn compute_max_heading_gap(d: Doc, nodes: &[Node], mut last: f64) -> (Option<f64>, f64) {
    if nodes.is_empty() {
        return (None, last);
    }
    let mut gap: Option<f64> = None;
    let upd = |g: Option<f64>, v: Option<f64>| -> Option<f64> {
        match v {
            Some(v) if v > g.unwrap_or(0.0) => Some(v),
            _ => g,
        }
    };
    for n in nodes {
        let pos = d.cpage_index(&n.heading) as f64 + n.heading.y_frac;
        gap = upd(gap, Some(pos - last));
        last = pos;
        let (rg, rl) = compute_max_heading_gap(d, &n.children.borrow(), last);
        gap = upd(gap, rg);
        last = rl;
    }
    (gap, last)
}

/// Any top-level heading is an abstract (type 5) or prominent.
// ref: outline_assembly/assembly.py::has_table_or_prominent
pub fn has_table_or_prominent(nodes: &[Node]) -> bool {
    nodes
        .iter()
        .any(|n| n.heading.kind == 5 || n.heading.is_prominent)
}

fn sort_by_order(d: Doc, v: &mut Vec<Cand>) {
    pi_pycompat::pysort::sort_by_key_lt(v, |c| heading_order_key(d, c), |a, b| tuple_lt(a, b));
}

/// Stage-06 snapshot hook: the candidates `build_doc_heading_candidates` returned.
pub type CandidateSink<'s> = &'s mut dyn FnMut(&[Cand]);

/// The outline tree as root nodes. `labeled` (the section openers) is extended and sorted in
/// place, as in the reference.
// ref: outline_assembly/assembly.py::assemble_outline
pub fn assemble_outline(d: Doc, labeled: &mut Vec<Node>, sink: Option<CandidateSink>) -> Vec<Node> {
    let mut general = build_doc_heading_candidates(d, labeled);
    if let Some(s) = sink {
        s(&general);
    }
    if !labeled.is_empty() || !general.is_empty() {
        let mut combined = general.clone();
        combined.extend(labeled.iter().map(|n| n.heading.clone()));
        sort_by_order(d, &mut combined);
        let clique = find_keyword_clique(d, &combined);
        let filtered = {
            let ctx = CliqueFilterContext::new(d, &combined, depth_comparator(d, clique.as_ref()));
            detect_body_headings(d, &ctx)
        };
        general.extend(filtered);
        sort_by_order(d, &mut general);
    }
    let mut clusters: Vec<Cluster> = if !labeled.is_empty() {
        let remaining = partition_candidates(d, &general, labeled);
        interleave_clusters(d, &remaining, labeled)
    } else {
        vec![Cluster {
            anchor: None,
            cands: general,
        }]
    };
    let mut st = OutlineState::new(d, &clusters);
    if !st.any_prominent && st.max_page <= st.min_page {
        return Vec::new();
    }
    let mut out: Vec<Node> = Vec::new();
    for cl in &mut clusters {
        let anchor = cl.anchor.clone();
        if let Some(a) = &anchor {
            push_heading_to_state(&mut st, &a.heading);
            out.push(a.clone());
        }
        let sub = extract_sub_headings(d, &mut st, anchor.as_ref(), &mut cl.cands);
        match &anchor {
            Some(a) => a.children.borrow_mut().extend(sub),
            None => out.extend(sub),
        }
        let sub_clique = if cl.cands.is_empty() {
            None
        } else {
            find_keyword_clique(d, &cl.cands)
        };
        let mut stack = HierarchyStack::new(sub_clique);
        for c in &cl.cands {
            if should_reject_heading(d, &st, c) {
                continue;
            }
            push_heading_to_state(&mut st, c);
            d.cblock(c).used_as_heading.set(true);
            let node = new_node(c.clone());
            match find_parent_heading(d, &mut stack, c) {
                Some(p) => p.children.borrow_mut().push(node.clone()),
                None => match &anchor {
                    Some(a) => a.children.borrow_mut().push(node.clone()),
                    None => out.push(node.clone()),
                },
            }
            stack.push(node);
        }
    }
    out
}

/// Whether nothing but headers, footers, watermarks or empty blocks precedes the heading.
// ref: outline_assembly/assembly.py::_heading_appears_at_page_top
fn heading_appears_at_page_top(d: Doc, h: &HeadingCandidate) -> bool {
    let page = d.page(h.page);
    let group_index = d.cblock(h).reading_order_index.get();
    for &bid in &page.reading {
        let b = &page.blocks[bid];
        if bid == h.block || b.reading_order_index.get() >= group_index {
            continue;
        }
        if b.char_count() == 0 {
            continue;
        }
        if matches!(b.kind.get(), 1 | 2 | 12) {
            continue;
        }
        return false;
    }
    true
}

struct TreeNode {
    title: String,
    start: usize,
    end: usize,
    appear_start: bool,
    children: Vec<usize>,
}

/// The outline as the PageIndex JSON tree (`title`, `node_id`, `start_index`, `end_index`,
/// `nodes`).
// ref: outline_assembly/assembly.py::outline_to_dict_tree
pub fn outline_to_dict_tree(d: Doc, nodes: &[Node], total_pages: usize) -> Vec<Value> {
    let mut arena: Vec<TreeNode> = Vec::new();
    // `flat_nodes` order = creation order = DFS pre-order of kept nodes.
    fn walk(d: Doc, items: &[Node], arena: &mut Vec<TreeNode>) -> Vec<usize> {
        let mut result = Vec::new();
        for it in items {
            let h = &it.heading;
            let child = h.prefix.as_ref().map_or(String::new(), |p| {
                trim_unicode_ws(&p.to_string_py()).to_string()
            });
            let node = h.title.as_ref().map_or(String::new(), |t| {
                trim_unicode_ws(&t.to_string_py()).to_string()
            });
            let title = if child.is_empty() {
                node
            } else {
                format!("{child} {node}")
            };
            let kids = it.children.borrow();
            if title.is_empty() {
                if !kids.is_empty() {
                    result.extend(walk(d, &kids, arena));
                }
                continue;
            }
            let pi = d.cpage_index(h);
            let id = arena.len();
            arena.push(TreeNode {
                title,
                start: pi,
                end: pi,
                appear_start: heading_appears_at_page_top(d, h),
                children: Vec::new(),
            });
            let ch = if kids.is_empty() {
                Vec::new()
            } else {
                walk(d, &kids, arena)
            };
            arena[id].children = ch;
            result.push(id);
        }
        result
    }
    let root = walk(d, nodes, &mut arena);

    fn collect(arena: &[TreeNode], ids: &[usize], flat: &mut Vec<usize>) {
        for &i in ids {
            flat.push(i);
            collect(arena, &arena[i].children, flat);
        }
    }
    let mut flat = Vec::new();
    collect(&arena, &root, &mut flat);
    for (k, &i) in flat.iter().enumerate() {
        let boundary = match flat.get(k + 1) {
            Some(&n) => {
                if arena[n].appear_start {
                    arena[n].start as i64 - 1
                } else {
                    arena[n].start as i64
                }
            }
            None => total_pages as i64,
        };
        let s = arena[i].start as i64;
        arena[i].end = s.max(if boundary > s { boundary } else { s }) as usize;
    }
    if let Some(&last) = flat.last() {
        arena[last].end = arena[last].start.max(total_pages);
    }
    fn promote(arena: &mut [TreeNode], ids: &[usize]) -> usize {
        let mut end = 0;
        for &c in ids {
            if !arena[c].children.is_empty() {
                let kids = arena[c].children.clone();
                let sub = promote(arena, &kids);
                arena[c].end = arena[c].end.max(sub);
            }
            end = end.max(arena[c].end);
        }
        end
    }
    promote(&mut arena, &root);
    let mut ids = vec![String::new(); arena.len()];
    for (k, &i) in flat.iter().enumerate() {
        ids[i] = format!("{k:04}");
    }
    fn emit(arena: &[TreeNode], ids: &[String], list: &[usize]) -> Vec<Value> {
        list.iter()
            .map(|&i| {
                let n = &arena[i];
                let mut m = Map::new();
                m.insert("title".into(), json!(n.title));
                m.insert("node_id".into(), json!(ids[i]));
                m.insert("start_index".into(), json!(n.start));
                m.insert("end_index".into(), json!(n.end));
                if !n.children.is_empty() {
                    m.insert("nodes".into(), Value::Array(emit(arena, ids, &n.children)));
                }
                Value::Object(m)
            })
            .collect()
    }
    emit(&arena, &ids, &root)
}
