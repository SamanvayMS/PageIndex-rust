//! Clusters lines into blocks and splits heading-body blocks.
//!
//! ref: pageindex/flash/blocks/build.py

use pi_pycompat::pysort;

use super::join_rules::{BlockClusterContext, SECTION_HEADING_TRIE, should_join_line_to_block};
use crate::labels::caption_text::{advance_past_line, trie_matches_all};
use crate::model::block::{Block, BlockId, first_span_of, last_span_of};
use crate::model::rects::{Bounded, left_edge_key, reading_order_key, tuple_eq, tuple_lt};
use crate::model::span_line::style_key;
use crate::phases::PageLayout;
use crate::tokens::tokenize_block;

/// Block arena + ids, as the reference's list of block objects (which may, in principle, hold
/// the same block twice).
pub struct BlockList {
    pub arena: Vec<Block>,
    pub ids: Vec<BlockId>,
}

// ref: blocks/build.py::split_heading_body_blocks
fn split_heading_body_blocks(
    arena: &mut Vec<Block>,
    input: Vec<BlockId>,
    page: &PageLayout,
) -> Vec<BlockId> {
    let mut out = Vec::new();
    for bid in input {
        let b = &arena[bid];
        let first_line = &page.lines[b.first_line()];
        if b.line_count() <= 1
            || (b.bbox_height() >= 0.6 * b.bbox_width()
                && (b.char_count() as f64) < 20.0 * b.line_count() as f64)
            || (style_key(&page.spans[first_span_of(b, page)])
                == style_key(&page.spans[last_span_of(b, page)])
                && first_line.bbox_width() > 0.5 * b.bbox_width())
        {
            out.push(bid);
            continue;
        }
        let toks = tokenize_block(b, page);
        let first_line_toks = toks.slice(0, advance_past_line(&toks, b.first_line(), 0));
        let split = toks.token_at(first_line_toks.length);
        if split.is_none_or(|t| t.first_cat == 3) {
            out.push(bid);
            continue;
        }
        if !trie_matches_all(&SECTION_HEADING_TRIE, &first_line_toks) {
            out.push(bid);
            continue;
        }
        let lines = b.lines.clone();
        let mut head = Block::new();
        head.add_line(lines[0], page);
        let mut body = Block::new();
        for &lid in &lines[1..] {
            body.add_line(lid, page);
        }
        arena.push(head);
        out.push(arena.len() - 1);
        arena.push(body);
        out.push(arena.len() - 1);
    }
    out
}

/// The reference's `SortedKeyList(key=left_edge_key)` of open blocks with set-style insertion.
struct BlockTree {
    ids: Vec<BlockId>,
}

impl BlockTree {
    fn bisect_left(&self, arena: &[Block], k: &[f64; 4]) -> usize {
        self.ids
            .partition_point(|&x| tuple_lt(&left_edge_key(&arena[x]), k))
    }

    fn bisect_right(&self, arena: &[Block], k: &[f64; 4]) -> usize {
        self.ids
            .partition_point(|&x| !tuple_lt(k, &left_edge_key(&arena[x])))
    }

    // ref: blocks/build.py::_set_add — equal left-edge keys drop the new block.
    fn set_add(&mut self, arena: &[Block], id: BlockId) {
        let k = left_edge_key(&arena[id]);
        let idx = self.bisect_left(arena, &k);
        if idx < self.ids.len() && tuple_eq(&left_edge_key(&arena[self.ids[idx]]), &k) {
            return;
        }
        self.ids.insert(idx, id);
    }

    /// `try: tree.remove(block) except ValueError: pass` (identity among equal keys).
    fn remove(&mut self, arena: &[Block], id: BlockId) {
        let k = left_edge_key(&arena[id]);
        let mut i = self.bisect_left(arena, &k);
        while i < self.ids.len() && tuple_eq(&left_edge_key(&arena[self.ids[i]]), &k) {
            if self.ids[i] == id {
                self.ids.remove(i);
                return;
            }
            i += 1;
        }
    }
}

/// Walks lines, extends compatible open blocks or opens new ones, splits heading+body blocks,
/// and returns the blocks sorted by reading-order key.
// ref: blocks/build.py::cluster_lines_into_blocks
pub fn cluster_lines_into_blocks(ctx: &BlockClusterContext) -> BlockList {
    let page = ctx.page;
    let mut arena: Vec<Block> = Vec::new();
    let mut tree = BlockTree { ids: Vec::new() };
    let mut clustered: Vec<BlockId> = Vec::new();
    let n = page.lines.len();
    for li in 0..n {
        let cand = &page.lines[li];
        let next = page.lines.get(li + 1);
        let mut seed = Block::new();
        seed.add_line(li, page);
        arena.push(seed);
        let seed_id = arena.len() - 1;
        let seed_key = left_edge_key(&arena[seed_id]);

        let mut cands: Vec<BlockId> = Vec::new();
        let idx_pred = tree.bisect_right(&arena, &seed_key);
        let mut b = idx_pred as i64 - 1;
        while b >= 0 {
            let eb = tree.ids[b as usize];
            if arena[eb].right_edge() < cand.left_edge() {
                break;
            }
            cands.push(eb);
            b -= 1;
        }
        let mut b = tree.bisect_left(&arena, &seed_key);
        while b < tree.ids.len() {
            let eb = tree.ids[b];
            if arena[eb].left_edge() > cand.right_edge() {
                break;
            }
            cands.push(eb);
            b += 1;
        }
        {
            let a = &arena;
            pysort::sort_by_key_lt(
                &mut cands,
                |&id| {
                    let x = &a[id];
                    [x.bottom_edge(), x.top_edge(), x.left_edge(), x.right_edge()]
                },
                |p, q| tuple_lt(p, q),
            );
        }

        let mut did_join = false;
        let first = cands.first().copied();
        for &eb in &cands {
            let join = !did_join
                && first.is_some_and(|f| {
                    should_join_line_to_block(ctx, &arena[eb], cand, next, &arena[f], eb == f)
                });
            if join {
                tree.remove(&arena, eb);
                arena[eb].add_line(li, page);
                tree.set_add(&arena, eb);
                did_join = true;
            } else {
                clustered.push(eb);
                tree.remove(&arena, eb);
            }
        }
        if !did_join {
            tree.set_add(&arena, seed_id);
        }
    }
    clustered.extend(tree.ids.iter().copied());
    let mut ids = split_heading_body_blocks(&mut arena, clustered, page);
    {
        let a = &arena;
        pysort::sort_by_key_lt(
            &mut ids,
            |&id| reading_order_key(&a[id]),
            |p, q| tuple_lt(p, q),
        );
    }
    BlockList { arena, ids }
}
