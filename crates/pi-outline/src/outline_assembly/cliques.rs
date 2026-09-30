//! Keyword cliques, clique trees, body-heading discovery, partition and interleave.
//!
//! ref: pageindex/flash/outline_assembly/cliques.py

use std::collections::{HashMap, HashSet};

use pi_layout::model::block::{Block, BlockId, dominant_style_of, is_caps_heavy};
use pi_layout::model::char_stats::info_weight;
use pi_layout::model::rects::{Bounded, tuple_lt, y_overlaps};
use pi_layout::model::span_line::style_key;
use pi_layout::phases::DocPage;
use pi_layout::tokens::tokenize_block;

use super::style_context::{
    Cluster, OutlineContext, StyleCluster, compare_heading_depth, has_style_neighbor,
    is_compatible_with_context,
};
use crate::consts::MAX_CLIQUE_DEPTH;
use crate::heading_detection::keyword_tables::SECTION_KEYWORD_TRIE;
use crate::heading_detection::neighbors::PageNeighborMap;
use crate::heading_detection::style_detectors::is_sentence_like;
use crate::heading_detection::text_checks::is_cover_page;
use crate::model::{
    Cand, Doc, HeadingCandidate, Node, compare_block_order, compare_heading_order,
    heading_order_key, new_node,
};

/// Largest clique of section-keyword headings sharing a font signature.
// ref: outline_assembly/cliques.py::find_keyword_clique
pub fn find_keyword_clique(d: Doc, cands: &[Cand]) -> Option<StyleCluster> {
    let mut buckets: Vec<(String, StyleCluster)> = Vec::new();
    for c in cands {
        let Some(title) = &c.title else { continue };
        if !SECTION_KEYWORD_TRIE.full_match(title) {
            continue;
        }
        let Some(first) = title.first() else { continue };
        if first.anchors.is_empty() {
            continue;
        }
        let fs = d
            .span(c.page, first.first_anchor_span().expect("anchor span"))
            .font_style();
        match buckets.iter_mut().find(|(k, _)| *k == fs) {
            Some((_, sc)) => {
                if sc.has_nearby_duplicate(d, c) {
                    return None;
                }
                if has_style_neighbor(d, sc, c, 2.0) {
                    sc.add(d, c);
                }
            }
            None => {
                let mut sc = StyleCluster::new();
                sc.add(d, c);
                buckets.push((fs, sc));
            }
        }
    }
    let mut winner: Option<usize> = None;
    let mut max_size = 0;
    for (i, (_, sc)) in buckets.iter().enumerate() {
        if sc.size() > max_size {
            winner = Some(i);
            max_size = sc.size();
        }
    }
    let w = winner?;
    if max_size <= 1 {
        return None;
    }
    let mut winner = buckets.swap_remove(w).1;
    for e in cands {
        if winner.contains(d, e) {
            continue;
        }
        if has_style_neighbor(d, &winner, e, 0.5) {
            winner.add(d, e);
        }
    }
    Some(winner)
}

/// ref: outline_assembly/cliques.py::CliqueTreeNode
struct TreeNode {
    heading: Option<Cand>,
    /// The root is its own parent.
    parent: usize,
    /// ref slot: `primary_slot`
    children: Vec<usize>,
    /// Next sibling. ref slot: `secondary_slot`
    next_sib: Option<usize>,
    /// Previous sibling. ref slot: `tertiary_slot`
    prev_sib: Option<usize>,
}

/// Clique tree built with a depth comparator. ref: outline_assembly/cliques.py::CliqueTreeBuilder
pub struct CliqueTree {
    nodes: Vec<TreeNode>,
    /// Builder cursor after the last insertion. ref slot: `primary_slot`
    pub cursor: usize,
}

const ROOT: usize = 0;

impl CliqueTree {
    // ref: outline_assembly/cliques.py::CliqueTreeBuilder.__init__
    pub fn build(headings: &[Cand], mut compare: impl FnMut(&Cand, &Cand) -> i32) -> Self {
        let mut t = CliqueTree {
            nodes: vec![TreeNode {
                heading: None,
                parent: ROOT,
                children: Vec::new(),
                next_sib: None,
                prev_sib: None,
            }],
            cursor: ROOT,
        };
        let mut depth = 0;
        for h in headings {
            loop {
                if t.cursor == ROOT {
                    t.append(ROOT, h);
                    depth += 1;
                    break;
                }
                let cur = t.nodes[t.cursor].heading.clone().expect("non-root heading");
                let cmp = compare(&cur, h);
                if cmp < 0 {
                    t.cursor = t.nodes[t.cursor].parent;
                    depth -= 1;
                } else {
                    if cmp > 0 && depth < MAX_CLIQUE_DEPTH {
                        t.append(t.cursor, h);
                        depth += 1;
                    } else {
                        let p = t.nodes[t.cursor].parent;
                        t.append(p, h);
                    }
                    break;
                }
            }
        }
        t
    }

    // ref: outline_assembly/cliques.py::append_tree_child
    fn append(&mut self, parent: usize, h: &Cand) {
        let id = self.nodes.len();
        let last = self.nodes[parent].children.last().copied();
        self.nodes.push(TreeNode {
            heading: Some(h.clone()),
            parent,
            children: Vec::new(),
            next_sib: None,
            prev_sib: last,
        });
        if let Some(l) = last {
            self.nodes[l].next_sib = Some(id);
        }
        self.nodes[parent].children.push(id);
        self.cursor = id;
    }

    // ref: outline_assembly/cliques.py::CliqueTreeNode.next
    fn next(&self, n: usize) -> Option<usize> {
        if let Some(&c) = self.nodes[n].children.first() {
            return Some(c);
        }
        if let Some(s) = self.nodes[n].next_sib {
            return Some(s);
        }
        self.ancestor_next_sibling(self.nodes[n].parent)
    }

    // ref: outline_assembly/cliques.py::find_ancestor_next_sibling
    fn ancestor_next_sibling(&self, n: usize) -> Option<usize> {
        if self.nodes[n].parent == n {
            return None;
        }
        self.nodes[n]
            .next_sib
            .or_else(|| self.ancestor_next_sibling(self.nodes[n].parent))
    }

    // ref: outline_assembly/cliques.py::descend_to_deepest_last
    fn deepest_last(&self, mut n: usize) -> usize {
        while let Some(&l) = self.nodes[n].children.last() {
            n = l;
        }
        n
    }
}

/// Dominant style plus caps-heavy state.
// ref: outline_assembly/cliques.py::block_style_signature
pub fn block_style_signature(b: &Block) -> String {
    format!(
        "{} {}",
        dominant_style_of(b),
        if is_caps_heavy(&b.char_stats) {
            "true"
        } else {
            "false"
        }
    )
}

/// Whether an ancestor in the clique tree already represents the block's style.
// ref: outline_assembly/cliques.py::is_member_of_tree
fn is_member_of_tree(
    d: Doc,
    t: &CliqueTree,
    block: &Block,
    sig: &str,
    sentence_like: bool,
    node: usize,
) -> bool {
    if t.nodes[node].parent == node {
        return false;
    }
    let Some(tp) = &t.nodes[node].heading else {
        return false;
    };
    if tp.kind == 5 || tp.is_prominent {
        return false;
    }
    let parent = t.nodes[node].parent;
    if !tp.numbering.is_empty() {
        return is_member_of_tree(d, t, block, sig, sentence_like, parent);
    }
    let pb = d.cblock(tp);
    if sig != block_style_signature(pb)
        || (sentence_like && is_sentence_like(pb, &d.page(tp.page).layout))
    {
        return is_member_of_tree(d, t, block, sig, sentence_like, parent);
    }
    if info_weight(&block.char_stats)
        >= pi_pycompat::pymath::max(100.0, 4.0 * info_weight(&pb.char_stats))
    {
        return is_member_of_tree(d, t, block, sig, sentence_like, parent);
    }
    true
}

/// Whether two overlapping same-style blocks can share a heading style.
// ref: outline_assembly/cliques.py::can_share_heading_style
fn can_share_heading_style(
    page: &DocPage,
    h: BlockId,
    other: Option<BlockId>,
    nm: &PageNeighborMap,
) -> bool {
    let Some(o) = other else { return false };
    let (hb, ob) = (&page.blocks[h], &page.blocks[o]);
    if !y_overlaps(hb, ob) || dominant_style_of(hb) != dominant_style_of(ob) {
        return false;
    }
    let lab = |x: Option<BlockId>| x.is_some_and(|i| page.blocks[i].caption_label.get() != 0);
    let (ha, oa, hr, or) = (nm.above(hb), nm.above(ob), nm.right(hb), nm.right(ob));
    if lab(ha) || lab(oa) || lab(hr) || lab(or) {
        return true;
    }
    let body = |x: Option<BlockId>| x.is_some_and(|i| page.blocks[i].is_body_paragraph.get());
    if hr != or && body(hr) && body(or) {
        return false;
    }
    true
}

/// Whether the candidate sorts before the given page/block position.
// ref: outline_assembly/cliques.py::heading_precedes_line
fn heading_precedes_line(d: Doc, c: &HeadingCandidate, p: usize, b: &Block) -> bool {
    let (cp, pp) = (d.cpage_index(c), d.page_index(p));
    if cp < pp {
        return true;
    }
    if cp != pp {
        return false;
    }
    compare_block_order(d, c.page, d.cblock(c), b) < 0.0
}

/// State for clique-based body-heading discovery.
/// ref: outline_assembly/cliques.py::CliqueFilterContext
pub struct CliqueFilterContext {
    /// Candidate blocks. ref slot: `state_slot`
    blocks: HashSet<(usize, BlockId)>,
    /// Signature counts of unnumbered candidates. ref slot: `tertiary_slot`
    sig_counts: HashMap<String, u64>,
    /// ref slot: `measure_slot`
    forward: CliqueTree,
    /// ref slot: `option_slot`
    reverse: CliqueTree,
    candidates: Vec<Cand>,
}

impl CliqueFilterContext {
    // ref: outline_assembly/cliques.py::CliqueFilterContext.__init__
    pub fn new(d: Doc, candidates: &[Cand], mut compare: impl FnMut(&Cand, &Cand) -> i32) -> Self {
        let mut blocks = HashSet::new();
        let mut sig_counts: HashMap<String, u64> = HashMap::new();
        for c in candidates {
            blocks.insert((c.page, c.block));
            if c.has_numbering || !c.numbering.is_empty() {
                continue;
            }
            *sig_counts
                .entry(block_style_signature(d.cblock(c)))
                .or_insert(0) += 1;
        }
        let forward = CliqueTree::build(candidates, &mut compare);
        let rev: Vec<Cand> = candidates.iter().rev().cloned().collect();
        let reverse = CliqueTree::build(&rev, &mut compare);
        CliqueFilterContext {
            blocks,
            sig_counts,
            forward,
            reverse,
            candidates: candidates.to_vec(),
        }
    }
}

/// Discover body headings by walking both clique trees alongside the document.
// ref: outline_assembly/cliques.py::detect_body_headings
pub fn detect_body_headings(d: Doc, ctx: &CliqueFilterContext) -> Vec<Cand> {
    let mut out = Vec::new();
    if ctx.candidates.is_empty() {
        return out;
    }
    let mut fwd = ROOT;
    let mut rev = ctx.reverse.cursor;
    for (p, page) in d.pages().iter().enumerate() {
        if is_cover_page(d.doc, page) {
            continue;
        }
        if page.output.is_empty() {
            continue;
        }
        let lay = &page.layout;
        let nc = PageNeighborMap::new(page);
        for &bid in &page.reading {
            let block = &page.blocks[bid];
            loop {
                let Some(nx) = ctx.forward.next(fwd) else {
                    break;
                };
                let Some(h) = &ctx.forward.nodes[nx].heading else {
                    break;
                };
                if !heading_precedes_line(d, h, p, block) {
                    break;
                }
                fwd = nx;
            }
            while let Some(h) = &ctx.reverse.nodes[rev].heading {
                if !heading_precedes_line(d, h, p, block) {
                    break;
                }
                rev = match ctx.reverse.nodes[rev].prev_sib {
                    Some(l) => ctx.reverse.deepest_last(l),
                    None => ctx.reverse.nodes[rev].parent,
                };
                if rev == ROOT {
                    break;
                }
            }
            if ctx.blocks.contains(&(p, bid)) {
                continue;
            }
            if ctx.forward.nodes[fwd].heading.is_none() {
                continue;
            }
            let cs = &block.char_stats;
            if block.char_count() == 0
                || block.weighted_skew > 1.0
                || (block.char_count() <= 1 && cs.first_cat != 4)
                || block.line_count() >= 5
                || block.kind.get() != 0
                || block.caption_label.get() != 0
                || (cs.category_counts[2] == 0 && cs.category_counts[4] == 0)
            {
                continue;
            }
            if block.caption_claimed.get() {
                continue;
            }
            let v = block.bold_frac;
            if 0.1 < v && v < 0.9 {
                continue;
            }
            let bs = dominant_style_of(block);
            let tokens = tokenize_block(block, lay);
            let anchor = tokens
                .first()
                .and_then(|t| t.anchors.last())
                .and_then(|a| a.span);
            if bs != style_key(d.first_span(p, block))
                && anchor.is_none_or(|a| bs != style_key(&lay.spans[a]))
                && bs != style_key(d.last_span(p, block))
            {
                continue;
            }
            if bs == lay.stats.dominant_style {
                continue;
            }
            let above = nc.above(block).map(|i| &page.blocks[i]);
            if let Some(a) = above
                && a.bottom_edge() - block.top_edge() < 0.3 * block.weighted_font_size
                && block.line_count() > 1
            {
                continue;
            }
            if above.is_some_and(|a| a.kind.get() == 3) {
                continue;
            }
            let sig = block_style_signature(block);
            let pred = nc.right(block).map(|i| &page.blocks[i]);
            if above.is_some_and(|a| sig == block_style_signature(a)) {
                continue;
            }
            if pred.is_some_and(|x| sig == block_style_signature(x)) {
                continue;
            }
            let o = block.orig_index.get() as i64;
            if can_share_heading_style(page, bid, d.output_at(p, o - 1), &nc) {
                continue;
            }
            if can_share_heading_style(page, bid, d.output_at(p, o + 1), &nc) {
                continue;
            }
            if ctx.sig_counts.get(&sig).copied().unwrap_or(0) < 3 {
                continue;
            }
            let (mut total, mut g3) = (0u64, 0u64);
            for t in tokens.iter() {
                if t.kind != 2 || t.len < 5 {
                    continue;
                }
                total += 1;
                if t.first_cat == 3 {
                    g3 += 1;
                }
            }
            let sentence_like = g3 as f64 >= pi_pycompat::pymath::max(2.0, total as f64 / 2.0);
            if !(is_member_of_tree(d, &ctx.forward, block, &sig, sentence_like, fwd)
                || is_member_of_tree(d, &ctx.reverse, block, &sig, sentence_like, rev))
            {
                continue;
            }
            out.push(HeadingCandidate::new(
                d,
                0,
                p,
                bid,
                nc.closest_body(block),
                Vec::new(),
                None,
                Some(tokens),
                false,
                false,
            ));
        }
    }
    out
}

/// Split candidates into those compatible with the labeled context (appended to `labeled`,
/// which is then sorted by heading order) and the rest.
// ref: outline_assembly/cliques.py::partition_candidates
pub fn partition_candidates(d: Doc, cands: &[Cand], labeled: &mut Vec<Node>) -> Vec<Cand> {
    let heads: Vec<Cand> = labeled.iter().map(|n| n.heading.clone()).collect();
    let mut ctx = OutlineContext::new(d, &heads);
    let mut remaining = Vec::new();
    for e in cands {
        if is_compatible_with_context(d, &ctx, e) {
            labeled.push(new_node(e.clone()));
            ctx.add(d, e);
        } else {
            remaining.push(e.clone());
        }
    }
    pi_pycompat::pysort::sort_by_key_lt(
        labeled,
        |n| heading_order_key(d, &n.heading),
        |a, b| tuple_lt(a, b),
    );
    remaining
}

/// General candidates grouped between successive labeled headings.
// ref: outline_assembly/cliques.py::interleave_clusters
pub fn interleave_clusters(d: Doc, cands: &[Cand], labeled: &[Node]) -> Vec<Cluster> {
    let mut out = Vec::new();
    let mut index = 0;
    let mut previous: Option<Node> = None;
    let mut acc: Vec<Cand> = Vec::new();
    for ln in labeled {
        while index < cands.len() && compare_heading_order(d, &cands[index], &ln.heading) < 0.0 {
            acc.push(cands[index].clone());
            index += 1;
        }
        if !acc.is_empty() || previous.is_some() {
            out.push(Cluster {
                anchor: previous.clone(),
                cands: std::mem::take(&mut acc),
            });
        }
        acc = Vec::new();
        previous = Some(ln.clone());
    }
    acc.extend(cands[index..].iter().cloned());
    out.push(Cluster {
        anchor: previous,
        cands: acc,
    });
    out
}

/// The depth comparator bound to a clique, as `assemble_outline` passes it.
pub fn depth_comparator<'a>(
    d: Doc<'a>,
    clique: Option<&'a StyleCluster>,
) -> impl FnMut(&Cand, &Cand) -> i32 + 'a {
    move |a, b| compare_heading_depth(d, a, b, clique)
}
