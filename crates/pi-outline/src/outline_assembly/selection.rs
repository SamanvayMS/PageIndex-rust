//! Heading selection against the outline state, hierarchy stack, and validity gates.
//!
//! ref: pageindex/flash/outline_assembly/selection.py

use std::collections::HashSet;

use pi_layout::model::block::{dominant_font_size, is_caps_heavy};
use pi_layout::model::char_stats::{info_weight, round_half_up_to_int};
use pi_layout::model::rects::Bounded;
use pi_layout::model::span_line::style_key;
use pi_layout::tokens::{TokenView, is_word_token, tokenize_block};
use pi_pycompat::unicode;

use super::style_context::{
    OutlineState, StyleCluster, compare_heading_depth, count_sibling_numberings,
    has_style_neighbor, heading_signature, is_in_oo_range, min_font_distance, parent_signature,
};
use crate::model::{Cand, Doc, HeadingCandidate, Node, new_node};

/// `style_key(first_anchor_span(first_token(title)))`, or "" without a title token.
fn title_style(d: Doc, c: &HeadingCandidate) -> String {
    match c.title.as_ref().and_then(|t| t.first()) {
        Some(t) => style_key(d.span(c.page, t.first_anchor_span().expect("anchor span"))),
        None => String::new(),
    }
}

/// Whether to reject a heading given the current state.
// ref: outline_assembly/selection.py::should_reject_heading
pub fn should_reject_heading(d: Doc, st: &OutlineState, c: &Cand) -> bool {
    if c.kind == 0 {
        for prev in st.levels.iter().flatten() {
            if compare_heading_depth(d, prev, c, None) == 1 {
                continue;
            }
            if let Some(sc) = st.parent_clusters.get(&parent_signature(prev))
                && is_in_oo_range(d, sc, c)
            {
                return true;
            }
        }
    }
    if st.last.as_ref().is_some_and(|l| l.is_prominent) && c.kind == 0 {
        let mut count = 0;
        for t in tokenize_block(d.cblock(c), &d.page(c.page).layout).iter() {
            if is_word_token(t) || t.kind == 1 {
                count += 1;
                if count >= 3 {
                    break;
                }
            }
        }
        if count >= 3 {
            return true;
        }
    }
    if c.kind == 1 && c.numbering.len() <= 1 {
        let siblings = if c.kind != 1 || c.numbering.is_empty() {
            0
        } else {
            count_sibling_numberings(&st.numbering, c, 0)
        };
        if siblings <= 1 {
            return true;
        }
    }
    let reject = if matches!(c.kind, 1 | 5 | 9 | 10 | 7) {
        false
    } else {
        let distance = min_font_distance(d, st, c);
        if distance <= 0.9 {
            false
        } else if distance >= f64::INFINITY {
            true
        } else {
            let b = d.cblock(c);
            !(is_caps_heavy(&b.char_stats)
                && c.anchor.is_some_and(|a| {
                    b.bottom_edge() - d.block(c.page, a).top_edge() < 5.0 * b.bbox_height()
                }))
        }
    };
    if reject {
        return true;
    }
    if c.kind == 1 {
        let first = c.numbering[0];
        if (first < st.max_number && first < st.max_number_in_chapter)
            || (st.max_number > 0.0 && first > st.max_number + 2.0)
        {
            return true;
        }
        if c.numbering.len() == 1
            && let Some(ls) = &st.last_single
        {
            if first == st.max_number_in_chapter {
                return true;
            }
            let existing = d.cblock(ls);
            if title_style(d, c) != title_style(d, ls) {
                let b = d.cblock(c);
                if (dominant_font_size(b) - dominant_font_size(existing)).abs() > 0.5
                    || round_half_up_to_int(b.bold_frac) != round_half_up_to_int(existing.bold_frac)
                {
                    return true;
                }
            }
        }
    }
    if let Some(l) = &st.last
        && c.kind == 4
        && l.kind == 4
        && !l.numbering.is_empty()
        && !c.numbering.is_empty()
        && (l.numbering[0] > c.numbering[0]
            || (c.numbering.len() == 1 && l.numbering[0] == c.numbering[0]))
    {
        return true;
    }
    if let Some(l) = &st.last
        && l.kind == 8
        && c.numbering.is_empty()
    {
        let empty = |t: &Option<TokenView>| t.as_ref().filter(|v| v.length > 0).cloned();
        let ct = empty(&c.title);
        let lt = empty(&l.title);
        let len = |t: &Option<TokenView>| t.as_ref().map_or(0, |v| v.length);
        if len(&ct) == len(&lt) {
            let mut same = true;
            for i in 0..len(&ct) {
                let a = ct.as_ref().and_then(|v| v.token_at(i));
                let b = lt.as_ref().and_then(|v| v.token_at(i));
                let (Some(a), Some(b)) = (a, b) else {
                    same = false;
                    break;
                };
                let na = pi_layout::model::block::strip_diacritics(&unicode::lower(&a.text));
                let nb = pi_layout::model::block::strip_diacritics(&unicode::lower(&b.text));
                if na != nb {
                    same = false;
                    break;
                }
            }
            if same {
                return true;
            }
        }
    }
    false
}

// ref: outline_assembly/selection.py::push_heading_to_state
pub fn push_heading_to_state(st: &mut OutlineState, c: &Cand) {
    let n = c.numbering.len();
    if n > 0 {
        while st.levels.len() < n {
            st.levels.push(None);
        }
        st.levels[n - 1] = Some(c.clone());
    }
    if c.kind == 1 {
        let first = c.numbering[0];
        st.max_number = pi_pycompat::pymath::max(st.max_number, first);
        st.max_number_in_chapter = pi_pycompat::pymath::max(st.max_number_in_chapter, first);
        if n == 1 {
            st.last_single = Some(c.clone());
        }
    } else if matches!(c.kind, 8 | 9) {
        st.max_number_in_chapter = 0.0;
    }
    st.last = Some(c.clone());
}

/// Stack of currently open outline nodes.
/// ref: outline_assembly/selection.py::HierarchyStack
pub struct HierarchyStack {
    /// ref slot: `auxiliary_slot`
    pub clique: Option<StyleCluster>,
    /// ref slot: `primary_slot`
    pub nodes: Vec<Node>,
    /// A type-4 heading was pushed. ref slot: `secondary_slot`
    pub saw_letter: bool,
    /// A prominent heading was pushed. ref slot: `tertiary_slot`
    pub saw_prominent: bool,
}

impl HierarchyStack {
    pub fn new(clique: Option<StyleCluster>) -> Self {
        HierarchyStack {
            clique,
            nodes: Vec::new(),
            saw_letter: false,
            saw_prominent: false,
        }
    }

    // ref: outline_assembly/selection.py::HierarchyStack.push
    pub fn push(&mut self, n: Node) {
        self.saw_letter = self.saw_letter || n.heading.kind == 4;
        self.saw_prominent = self.saw_prominent || n.heading.is_prominent;
        self.nodes.push(n);
    }
}

/// Pop the stack until a parent for the candidate is found.
// ref: outline_assembly/selection.py::find_parent_heading
pub fn find_parent_heading(d: Doc, stack: &mut HierarchyStack, c: &Cand) -> Option<Node> {
    let mut heading: Option<Cand> = None;
    while let Some(top) = stack.nodes.last().cloned() {
        let sc = top.heading.clone();
        if c.is_prominent && c.numbering.len() <= 1 && sc.kind != 8 {
            stack.nodes.pop();
            heading = Some(sc);
            continue;
        }
        if sc.is_prominent && c.kind == 5 {
            stack.nodes.pop();
            heading = Some(sc);
            continue;
        }
        let cmp = compare_heading_depth(d, &sc, c, stack.clique.as_ref());
        if cmp != -1 {
            if cmp == 1 {
                return Some(top);
            }
            if sc.kind != c.kind
                && matches!(c.kind, 4 | 2)
                && !stack.saw_prominent
                && is_appendix_nesting_ok(stack, c)
            {
                let first = c.numbering.first().copied().unwrap_or(0.0);
                match &heading {
                    None => {
                        if first == 1.0 {
                            return Some(top);
                        }
                    }
                    Some(h) => {
                        if c.kind == h.kind
                            && !c.numbering.is_empty()
                            && !h.numbering.is_empty()
                            && first > h.numbering[0]
                        {
                            return Some(top);
                        }
                    }
                }
            }
        }
        stack.nodes.pop();
        heading = Some(sc);
    }
    None
}

// ref: outline_assembly/selection.py::is_appendix_nesting_ok
pub fn is_appendix_nesting_ok(stack: &HierarchyStack, c: &HeadingCandidate) -> bool {
    if c.kind != 4 || stack.saw_letter {
        return true;
    }
    let Some(top) = stack.nodes.last() else {
        return true;
    };
    match top.heading.numbering.first() {
        None => true,
        Some(&n) => n <= 3.0,
    }
}

/// Walk a cluster's leading abstract/keyword candidates; consumes them from `cands`.
// ref: outline_assembly/selection.py::extract_sub_headings
pub fn extract_sub_headings(
    d: Doc,
    st: &mut OutlineState,
    parent: Option<&Node>,
    cands: &mut Vec<Cand>,
) -> Vec<Node> {
    if cands.is_empty() {
        return Vec::new();
    }
    let first = cands[0].clone();
    let first_pi = d.cpage_index(&first);
    let mut page_index = parent.map_or(0, |p| d.cpage_index(&p.heading) as i64 - 1);
    let mut acc = 0.0;
    let end_pg = first_pi.min(d.pages().len()) as i64;
    let first_roi = d.cblock(&first).reading_order_index.get();
    while page_index < end_pg {
        let hp = d.page(page_index as usize);
        if hp.has_body.get() {
            for &bid in &hp.output {
                let b = &hp.blocks[bid];
                if page_index >= first_pi as i64 - 1 && b.reading_order_index.get() >= first_roi {
                    break;
                }
                if b.is_body_paragraph.get() {
                    acc += info_weight(&b.char_stats);
                    if acc >= 1000.0 {
                        return Vec::new();
                    }
                }
            }
        }
        page_index += 1;
    }
    let mut out: Vec<Node> = Vec::new();
    let mut anchor: Option<Node> = parent.filter(|p| p.heading.kind == 5).cloned();
    let mut seen: HashSet<String> = HashSet::new();
    let mut sc = StyleCluster::new();
    let mut saw_numbered = false;
    let mut index = 0;
    while index < cands.len() {
        let cc = cands[index].clone();
        if !(cc.kind == 5
            || cc.kind == 6
            || (cc.kind == 11 && cc.has_numbering && anchor.is_some() && index <= 1))
        {
            if let Some(nx) = cands.get(index + 1)
                && nx.kind == 5
                && nx.page == cc.page
                && nx.anchor == cc.anchor
            {
                index += 1;
                continue;
            }
            break;
        }
        let sig = heading_signature(&cc);
        if seen.contains(&sig) {
            index += 1;
            continue;
        }
        if should_reject_heading(d, st, &cc) {
            index += 1;
            continue;
        }
        push_heading_to_state(st, &cc);
        seen.insert(sig);
        if cc.has_numbering {
            saw_numbered = true;
        } else if saw_numbered {
            break;
        }
        let Some(a) = &anchor else {
            let n = new_node(cc.clone());
            out.push(n.clone());
            anchor = Some(n);
            sc.add(d, &cc);
            index += 1;
            continue;
        };
        let ah = a.heading.clone();
        if d.cpage_index(&cc) > d.cpage_index(&ah) {
            break;
        }
        if compare_heading_depth(d, &ah, &cc, None) != 1 {
            if !has_style_neighbor(d, &sc, &cc, 1.0) {
                break;
            }
            let n = new_node(cc.clone());
            out.push(n.clone());
            anchor = Some(n);
            sc.add(d, &cc);
        }
        index += 1;
    }
    cands.drain(..index);
    if (out.len() >= 3
        || (out.len() == 2 && out[0].heading.has_numbering && out[1].heading.has_numbering))
        && out[0].heading.kind != 5
    {
        return Vec::new();
    }
    out
}

/// Top-level prominent headings of the outline.
// ref: outline_assembly/selection.py::extract_top_level_headings
pub fn extract_top_level_headings(nodes: &[Node]) -> Vec<Node> {
    let mut out = Vec::new();
    let mut saw = false;
    for n in nodes {
        if n.heading.is_prominent {
            if !saw {
                out.push(n.clone());
            }
            saw = true;
        } else {
            saw = false;
            out.extend(extract_top_level_headings(&n.children.borrow()));
        }
    }
    out
}

/// Top-level headings span a meaningful fraction of the document.
// ref: outline_assembly/selection.py::is_outline_valid
pub fn is_outline_valid(d: Doc, nodes: &[Node]) -> bool {
    let top = extract_top_level_headings(nodes);
    if top.len() < 3 {
        return false;
    }
    if top.len() >= 5 {
        return true;
    }
    let mut last_page = 1.0;
    let n = d.pages().len() as f64;
    for t in &top {
        let line = d.cpage_index(&t.heading) as f64;
        if line - last_page > 0.5 * n {
            return false;
        }
        last_page = line;
    }
    true
}

/// Chapter count and inter-chapter span check.
// ref: outline_assembly/selection.py::is_chapter_outline_valid
pub fn is_chapter_outline_valid(d: Doc, nodes: &[Node]) -> bool {
    let mut chapters = 0i64;
    let mut span = 0i64;
    let mut previous = -1i64;
    let n = d.pages().len() as i64;
    for c in nodes {
        let cp = d.cpage_index(&c.heading) as i64;
        if previous >= 0 {
            span += cp - previous;
            previous = -1;
        }
        if c.heading.kind == 8 {
            chapters += 1;
            previous = cp;
        }
    }
    if previous >= 0 {
        span += n - previous + 1;
    }
    chapters >= 3
        && span as f64 >= 0.7 * n as f64
        && (span as f64) / (chapters.max(1) as f64) < 100.0
}
