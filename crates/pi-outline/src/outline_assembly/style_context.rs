//! Style clusters, outline contexts, numbering trie, global outline state and the pairwise
//! heading-depth comparator.
//!
//! ref: pageindex/flash/outline_assembly/style_context.py (+ the StyleCluster helpers of
//! outline_assembly/candidates.py)
//!
//! `SortedKeyList`s become sorted `Vec`s searched with Python-tuple `<` (`tuple_lt`); the
//! reference only ever inserts keys that are not already present, so an insertion at the
//! `bisect_left` position reproduces its order.

use std::collections::HashMap;

use pi_layout::model::block::{
    dominant_font_size, dominant_style_of, heading_score, is_caps_heavy,
};
use pi_layout::model::rects::{tuple_eq, tuple_lt};
use pi_layout::model::span_line::style_key;
use pi_layout::tokens::is_char_token;
use pi_pycompat::unicode;

use crate::model::{
    Cand, Doc, HeadingCandidate, Node, Num, compare_heading_order, heading_order_key, num_str,
};

/// `(heading_score(block), heading_order_key(candidate))`.
pub type ClusterKey = [f64; 7];

fn cluster_key(d: Doc, c: &HeadingCandidate) -> ClusterKey {
    let k = heading_order_key(d, c);
    [
        heading_score(d.cblock(c)),
        k[0],
        k[1],
        k[2],
        k[3],
        k[4],
        k[5],
    ]
}

/// `bisect.bisect_left` over keys with tuple `<`.
fn bisect_left<const N: usize>(keys: &[[f64; N]], k: &[f64; N]) -> usize {
    let (mut lo, mut hi) = (0, keys.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if tuple_lt(&keys[mid], k) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

/// `bisect.bisect_right` over keys with tuple `<`.
fn bisect_right<const N: usize>(keys: &[[f64; N]], k: &[f64; N]) -> usize {
    let (mut lo, mut hi) = (0, keys.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if tuple_lt(k, &keys[mid]) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}

/// Full heading signature including numbering or text.
// ref: outline_assembly/candidates.py::heading_signature
pub fn heading_signature(c: &HeadingCandidate) -> String {
    if !c.numbering.is_empty() {
        let nums: Vec<String> = c.numbering.iter().map(|&n| num_str(n)).collect();
        return format!("{}|{}", c.kind, nums.join(","));
    }
    let mut s = format!("{}|", c.kind);
    if let Some(t) = &c.title {
        for tok in t.iter() {
            if is_char_token(tok) {
                s.push_str(&unicode::lower(&tok.text));
            }
        }
    }
    s
}

/// Signature of the parent numbering prefix.
// ref: outline_assembly/candidates.py::parent_signature
pub fn parent_signature(c: &HeadingCandidate) -> String {
    let mut s = format!("{}|", c.kind);
    let n = c.numbering.len();
    for i in 0..n.saturating_sub(1) {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&num_str(c.numbering[i]));
    }
    s
}

/// Group of headings sharing a font/style signature.
/// ref: outline_assembly/style_context.py::StyleCluster
#[derive(Default)]
pub struct StyleCluster {
    /// signature -> candidate. ref slot: `state_slot`
    by_sig: HashMap<String, Cand>,
    /// ref slot: `secondary_slot` (SortedKeyList)
    keys: Vec<ClusterKey>,
    items: Vec<Cand>,
    /// Min by heading order. ref slot: `primary_slot`
    pub first: Option<Cand>,
    /// Max by heading order. ref slot: `tertiary_slot`
    pub last: Option<Cand>,
}

impl StyleCluster {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn size(&self) -> usize {
        self.items.len()
    }

    pub fn items(&self) -> &[Cand] {
        &self.items
    }

    /// Key-based containment. ref: StyleCluster.contains
    pub fn contains(&self, d: Doc, c: &HeadingCandidate) -> bool {
        let k = cluster_key(d, c);
        let i = bisect_left(&self.keys, &k);
        i < self.keys.len() && tuple_eq(&self.keys[i], &k)
    }

    // ref: outline_assembly/style_context.py::StyleCluster.add
    pub fn add(&mut self, d: Doc, c: &Cand) {
        self.by_sig.insert(heading_signature(c), c.clone());
        let k = cluster_key(d, c);
        let i = bisect_left(&self.keys, &k);
        if i >= self.keys.len() || !tuple_eq(&self.keys[i], &k) {
            self.keys.insert(i, k);
            self.items.insert(i, c.clone());
        }
        if self
            .first
            .as_ref()
            .is_none_or(|f| compare_heading_order(d, c, f) < 0.0)
        {
            self.first = Some(c.clone());
        }
        if self
            .last
            .as_ref()
            .is_none_or(|l| compare_heading_order(d, c, l) > 0.0)
        {
            self.last = Some(c.clone());
        }
    }

    /// A matching signature within +/- 20 pages.
    // ref: outline_assembly/style_context.py::StyleCluster.has_nearby_duplicate
    pub fn has_nearby_duplicate(&self, d: Doc, c: &HeadingCandidate) -> bool {
        self.by_sig
            .get(&heading_signature(c))
            .is_some_and(|e| (d.cpage_index(c) as i64 - d.cpage_index(e) as i64).abs() < 20)
    }
}

/// Whether a candidate lies within a cluster's heading-order range.
// ref: outline_assembly/candidates.py::is_in_oo_range
pub fn is_in_oo_range(d: Doc, s: &StyleCluster, c: &HeadingCandidate) -> bool {
    let (Some(f), Some(l)) = (&s.first, &s.last) else {
        return false;
    };
    compare_heading_order(d, c, f) >= 0.0 && compare_heading_order(d, c, l) <= 0.0
}

/// Whether a candidate is close to a compatible style neighbor in the cluster.
// ref: outline_assembly/candidates.py::has_style_neighbor
pub fn has_style_neighbor(d: Doc, s: &StyleCluster, c: &HeadingCandidate, tol: f64) -> bool {
    let mb = d.cblock(c);
    let score = heading_score(mb);
    let matched = std::cell::Cell::new(false);
    let fcheck = |item: &HeadingCandidate| -> bool {
        let lb = d.cblock(item);
        if (score - heading_score(lb)).abs() >= tol {
            return true;
        }
        let state = if (heading_score(mb) - heading_score(lb)).abs() >= tol {
            false
        } else if !c.numbering.is_empty() && !item.numbering.is_empty() {
            c.kind == item.kind && c.numbering.len() == item.numbering.len()
        } else if (c.numbering.is_empty() && item.numbering.len() > 1)
            || (item.numbering.is_empty() && c.numbering.len() > 1)
        {
            false
        } else {
            let bc = mb.isolated_centered.get();
            let oc = lb.isolated_centered.get();
            if bc || oc {
                bc == oc
            } else if dominant_style_of(mb) == dominant_style_of(lb) {
                true
            } else if d.first_span(c.page, mb).font_style()
                != d.first_span(item.page, lb).font_style()
            {
                false
            } else {
                (dominant_font_size(mb) - dominant_font_size(lb)).abs() < tol
            }
        };
        if state {
            matched.set(true);
            return true;
        }
        false
    };
    let idx = bisect_right(&s.keys, &cluster_key(d, c));
    for i in idx..s.items.len() {
        if fcheck(&s.items[i]) {
            break;
        }
    }
    if !matched.get() {
        for i in (0..idx).rev() {
            if fcheck(&s.items[i]) {
                break;
            }
        }
    }
    matched.get()
}

/// Style clusters for chapter, appendix, numbered, and general headings.
/// ref: outline_assembly/style_context.py::OutlineContext
pub struct OutlineContext {
    /// type 10. ref slot: `secondary_slot`
    pub appendix: StyleCluster,
    /// type 8. ref slot: `primary_slot`
    pub chapter: StyleCluster,
    /// Numbered. ref slot: `auxiliary_slot`
    pub numbered: StyleCluster,
    /// Everything else. ref slot: `tertiary_slot`
    pub general: StyleCluster,
    /// Initial headings, unique by heading-order key, sorted. ref slot: `state_slot`
    pub ordered: Vec<Cand>,
}

impl OutlineContext {
    // ref: outline_assembly/style_context.py::OutlineContext.__init__
    pub fn new(d: Doc, headings: &[Cand]) -> Self {
        let mut ctx = OutlineContext {
            appendix: StyleCluster::new(),
            chapter: StyleCluster::new(),
            numbered: StyleCluster::new(),
            general: StyleCluster::new(),
            ordered: Vec::new(),
        };
        let mut seen: Vec<[f64; 6]> = Vec::new();
        for h in headings {
            ctx.add(d, h);
            let k = heading_order_key(d, h);
            // `key in set`: tuple equality (hash-equal floats, -0.0 == 0.0).
            if !seen.iter().any(|s| tuple_eq(s, &k)) {
                seen.push(k);
                ctx.ordered.push(h.clone());
            }
        }
        pi_pycompat::pysort::sort_by_key_lt(
            &mut ctx.ordered,
            |c| heading_order_key(d, c),
            |a, b| tuple_lt(a, b),
        );
        ctx
    }

    // ref: outline_assembly/style_context.py::pick_style_bucket
    pub fn bucket(&self, c: &HeadingCandidate) -> &StyleCluster {
        if c.kind == 10 {
            &self.appendix
        } else if c.kind == 8 {
            &self.chapter
        } else if !c.numbering.is_empty() {
            &self.numbered
        } else {
            &self.general
        }
    }

    fn bucket_mut(&mut self, c: &HeadingCandidate) -> &mut StyleCluster {
        if c.kind == 10 {
            &mut self.appendix
        } else if c.kind == 8 {
            &mut self.chapter
        } else if !c.numbering.is_empty() {
            &mut self.numbered
        } else {
            &mut self.general
        }
    }

    // ref: outline_assembly/style_context.py::OutlineContext.add
    pub fn add(&mut self, d: Doc, c: &Cand) {
        self.bucket_mut(c).add(d, c);
    }

    // ref: outline_assembly/style_context.py::OutlineContext.has_nearby_duplicate
    pub fn has_nearby_duplicate(&self, d: Doc, c: &HeadingCandidate) -> bool {
        self.bucket(c).has_nearby_duplicate(d, c)
    }
}

/// Whether a candidate conflicts with the existing outline context.
// ref: outline_assembly/style_context.py::has_conflict_in_context
pub fn has_conflict_in_context(d: Doc, ctx: &OutlineContext, c: &HeadingCandidate) -> bool {
    if c.kind != 10 && is_in_oo_range(d, &ctx.appendix, c) {
        return true;
    }
    if c.kind != 8 && is_in_oo_range(d, &ctx.chapter, c) {
        return true;
    }
    if c.numbering.is_empty() && is_in_oo_range(d, &ctx.numbered, c) {
        return true;
    }
    c.kind != 8 && !c.numbering.is_empty() && ctx.chapter.size() > 0
}

/// Whether a candidate can join the outline context.
// ref: outline_assembly/style_context.py::is_compatible_with_context
pub fn is_compatible_with_context(d: Doc, ctx: &OutlineContext, c: &HeadingCandidate) -> bool {
    if has_conflict_in_context(d, ctx, c) {
        return false;
    }
    let mut text: Option<&Cand> = None;
    for item in &ctx.ordered {
        if compare_heading_order(d, item, c) <= 0.0 {
            if text.is_none_or(|t| compare_heading_order(d, item, t) > 0.0) {
                text = Some(item);
            }
        } else {
            break;
        }
    }
    if let Some(t) = text {
        let cb = d.cblock(c);
        let pb = d.cblock(t);
        if pb.isolated_centered.get() && !cb.isolated_centered.get() {
            return false;
        }
        if !c.is_prominent && heading_score(pb) > heading_score(cb) + 0.5 {
            return false;
        }
        if t.is_prominent && !c.is_prominent && heading_score(pb) > heading_score(cb) - 0.5 {
            return false;
        }
    }
    let sc = ctx.bucket(c);
    if !sc.has_nearby_duplicate(d, c) && has_style_neighbor(d, sc, c, 1.0) {
        return true;
    }
    c.numbering.len() == 1 && has_style_neighbor(d, &ctx.general, c, 1.0)
}

/// Recursive map over numbering prefixes.
/// ref: outline_assembly/style_context.py::NumberingTrie
#[derive(Default)]
pub struct NumberingTrie {
    /// ref slot: `primary_slot` (dict; int/float keys compare by value)
    children: Vec<(Num, NumberingTrie)>,
    /// ref slot: `secondary_slot`
    count: u64,
}

impl NumberingTrie {
    fn child(&self, n: Num) -> Option<&NumberingTrie> {
        self.children.iter().find(|(k, _)| *k == n).map(|(_, t)| t)
    }
}

// ref: outline_assembly/style_context.py::insert_numbering
pub fn insert_numbering(t: &mut NumberingTrie, c: &HeadingCandidate, index: usize) {
    if index == c.numbering.len() {
        t.count += 1;
        return;
    }
    let n = c.numbering[index];
    let pos = match t.children.iter().position(|(k, _)| *k == n) {
        Some(p) => p,
        None => {
            t.children.push((n, NumberingTrie::default()));
            t.children.len() - 1
        }
    };
    insert_numbering(&mut t.children[pos].1, c, index + 1);
}

// ref: outline_assembly/style_context.py::count_sibling_numberings
pub fn count_sibling_numberings(t: &NumberingTrie, c: &HeadingCandidate, index: usize) -> u64 {
    if index as i64 >= c.numbering.len() as i64 - 1 {
        return t.children.iter().filter(|(_, r)| r.count > 0).count() as u64;
    }
    match t.child(c.numbering[index]) {
        None => 0,
        Some(r) => count_sibling_numberings(r, c, index + 1),
    }
}

/// One labeled heading with the general candidates that follow it.
pub struct Cluster {
    /// ref: `labeled_anchor`
    pub anchor: Option<Node>,
    /// ref: `cluster_candidates`
    pub cands: Vec<Cand>,
}

/// Per-font-size tree entry: `{"size", "heading"}` keyed by `(size, heading_order_key)`.
struct FontEntry {
    key: ClusterKey,
    size: f64,
    heading: Cand,
}

/// Global state of the outline assembly walk.
/// ref: outline_assembly/style_context.py::OutlineState
pub struct OutlineState {
    /// font_style -> sorted entries. ref slot: `state_slot`
    trees: HashMap<String, Vec<FontEntry>>,
    /// ref slot: `cache_slot`
    pub numbering: NumberingTrie,
    /// parent signature -> dominant cluster. ref slot: `marker_slot`
    pub parent_clusters: HashMap<String, StyleCluster>,
    /// ref slot: `previous_slot`
    pub min_page: f64,
    /// ref slot: `option_slot`
    pub max_page: f64,
    /// ref slot: `measure_slot`
    pub any_prominent: bool,
    /// Last heading per numbering depth. ref slot: `style_slot`
    pub levels: Vec<Option<Cand>>,
    /// ref slot: `secondary_slot`
    pub max_number: Num,
    /// Reset by chapters. ref slot: `tertiary_slot`
    pub max_number_in_chapter: Num,
    /// Last single-level numbered heading. ref slot: `auxiliary_slot`
    pub last_single: Option<Cand>,
    /// Last pushed heading. ref slot: `primary_slot`
    pub last: Option<Cand>,
}

impl OutlineState {
    // ref: outline_assembly/style_context.py::OutlineState.__init__
    pub fn new(d: Doc, clusters: &[Cluster]) -> Self {
        let mut st = OutlineState {
            trees: HashMap::new(),
            numbering: NumberingTrie::default(),
            parent_clusters: HashMap::new(),
            min_page: f64::INFINITY,
            max_page: f64::NEG_INFINITY,
            any_prominent: false,
            levels: Vec::new(),
            max_number: 0.0,
            max_number_in_chapter: 0.0,
            last_single: None,
            last: None,
        };
        let mut by_parent: Vec<(String, Vec<StyleCluster>)> = Vec::new();
        for cl in clusters {
            if let Some(a) = &cl.anchor {
                apply_heading_to_state(d, &mut st, &a.heading);
            }
            for sc in &cl.cands {
                apply_heading_to_state(d, &mut st, sc);
                if sc.numbering.is_empty() {
                    continue;
                }
                let key = parent_signature(sc);
                let pos = match by_parent.iter().position(|(k, _)| *k == key) {
                    Some(p) => p,
                    None => {
                        by_parent.push((key, Vec::new()));
                        by_parent.len() - 1
                    }
                };
                let list = &mut by_parent[pos].1;
                let mut placed = list.iter().position(|s| {
                    !s.has_nearby_duplicate(d, sc) && has_style_neighbor(d, s, sc, 1.0)
                });
                if placed.is_none() && list.len() < 3 {
                    list.push(StyleCluster::new());
                    placed = Some(list.len() - 1);
                }
                if let Some(p) = placed {
                    list[p].add(d, sc);
                }
            }
        }
        for (key, group) in by_parent {
            // `group.sort(key=-size)` (stable) then `group[0]`: the first cluster of max size.
            let mut best: Option<StyleCluster> = None;
            for g in group {
                if best.as_ref().is_none_or(|b| g.size() > b.size()) {
                    best = Some(g);
                }
            }
            let best = best.expect("non-empty group");
            if best.size() <= 2 {
                continue;
            }
            st.parent_clusters.insert(key, best);
        }
        st
    }

    /// Entries of the per-font-size tree, in key order.
    fn tree(&self, font_style: &str) -> Option<&Vec<FontEntry>> {
        self.trees.get(font_style)
    }
}

/// Add a heading to the per-font trees and update document-level outline state.
// ref: outline_assembly/style_context.py::_apply_heading_to_state
pub fn apply_heading_to_state(d: Doc, st: &mut OutlineState, c: &Cand) {
    let mut seen: Vec<String> = Vec::new();
    let line = d.cblock(c).first_line();
    for tl in [&c.prefix, &c.title].into_iter().flatten() {
        for t in tl.iter() {
            if t.line() != Some(line) {
                break;
            }
            if t.kind != 2 {
                continue;
            }
            for a in &t.anchors {
                if a.line != Some(line) {
                    break;
                }
                let span = d.span(c.page, a.span.expect("anchor span"));
                let style = style_key(span);
                if seen.contains(&style) {
                    continue;
                }
                seen.push(style);
                let fs = span.font_style();
                let tree = st.trees.entry(fs).or_default();
                let k = heading_order_key(d, c);
                let key: ClusterKey = [span.font_size, k[0], k[1], k[2], k[3], k[4], k[5]];
                let keys: Vec<ClusterKey> = tree.iter().map(|e| e.key).collect();
                let i = bisect_left(&keys, &key);
                if i >= tree.len() || !tuple_eq(&tree[i].key, &key) {
                    tree.insert(
                        i,
                        FontEntry {
                            key,
                            size: span.font_size,
                            heading: c.clone(),
                        },
                    );
                }
            }
        }
    }
    if c.kind == 1 && !c.numbering.is_empty() {
        insert_numbering(&mut st.numbering, c, 0);
    }
    let pi = d.cpage_index(c) as f64;
    st.min_page = pi_pycompat::pymath::min(st.min_page, pi);
    st.max_page = pi_pycompat::pymath::max(st.max_page, pi);
    if !st.any_prominent {
        st.any_prominent = c.is_prominent;
    }
}

/// Minimum font-size distance to another heading of the same font style on the heading's line.
// ref: outline_assembly/selection.py::min_font_distance
pub fn min_font_distance(d: Doc, st: &OutlineState, c: &Cand) -> f64 {
    let mut min = f64::INFINITY;
    let line = d.cblock(c).first_line();
    for tl in [&c.prefix, &c.title].into_iter().flatten() {
        for t in tl.iter() {
            if t.kind != 2 {
                continue;
            }
            for a in &t.anchors {
                if a.line != Some(line) {
                    return min;
                }
                let span = d.span(c.page, a.span.expect("anchor span"));
                let Some(tree) = st.tree(&span.font_style()) else {
                    continue;
                };
                for e in tree {
                    if std::rc::Rc::ptr_eq(&e.heading, c) {
                        continue;
                    }
                    let diff = (span.font_size - e.size).abs();
                    if diff < min {
                        min = diff;
                    }
                    if diff <= 0.0 {
                        return 0.0;
                    }
                }
            }
        }
    }
    min
}

/// Relative nesting depth: `1` when `a` is shallower (b nests under a), `-1` when deeper, `0`
/// for the same level. The docstring in the reference states the opposite sign; this follows
/// the code (see parity/KNOWN_DIFFS.md).
// ref: outline_assembly/style_context.py::compare_heading_depth
pub fn compare_heading_depth(
    d: Doc,
    a: &HeadingCandidate,
    b: &HeadingCandidate,
    clique: Option<&StyleCluster>,
) -> i32 {
    let special = matches!(a.kind, 8..=10);
    let other_special = matches!(b.kind, 8..=10);
    if special && other_special {
        return 0;
    }
    if special || other_special {
        return if special { 1 } else { -1 };
    }
    if (a.kind == 1 && b.kind == 1) || (a.kind == 4 && b.kind == 4) {
        let (l, r) = (a.numbering.len(), b.numbering.len());
        if l == r {
            return 0;
        }
        return if l < r { 1 } else { -1 };
    }
    if a.kind == 2 && b.kind == 2 {
        return 0;
    }
    if a.kind == 11 && b.numbering.len() == 1 {
        return -1;
    }
    let hb = d.cblock(a);
    let bb = d.cblock(b);
    if a.has_numbering != b.has_numbering {
        return if a.has_numbering { -1 } else { 1 };
    }
    if a.has_numbering
        && b.has_numbering
        && (d.first_span(a.page, hb).font_size - d.first_span(b.page, bb).font_size).abs() < 0.9
    {
        return 0;
    }
    let score = heading_score(hb);
    let other = heading_score(bb);
    let a_in = clique.is_some_and(|q| q.contains(d, a));
    let b_in = clique.is_some_and(|q| q.contains(d, b));
    let gap = (score - other).abs();
    if gap > 1.9 || (gap > 0.9 && (!a_in || !b_in)) {
        return if score > other { 1 } else { -1 };
    }
    let a_caps = is_caps_heavy(&hb.char_stats);
    let b_caps = is_caps_heavy(&bb.char_stats);
    if a_caps != b_caps {
        return if a_caps { 1 } else { -1 };
    }
    let centered = hb.isolated_centered.get();
    if centered != bb.isolated_centered.get() {
        return if centered { 1 } else { -1 };
    }
    if !(a.kind == 5 && b.kind == 5) {
        let a_sk = hb.italic_frac > 0.99;
        let b_sk = bb.italic_frac > 0.99;
        if a_sk != b_sk {
            return if a_sk { -1 } else { 1 };
        }
    }
    if a_caps || (a_in && b_in) {
        return 0;
    }
    if (a_in && b.kind != 4) || (b_in && a.kind != 4) {
        return if a_in { 1 } else { -1 };
    }
    let bold = hb.bold_frac > 0.5;
    let other_bold = bb.bold_frac > 0.5;
    if bold != other_bold {
        return if bold { 1 } else { -1 };
    }
    0
}
