//! Per-page horizontal-bucket neighbor map.
//!
//! ref: pageindex/flash/heading_detection/neighbors.py
//!
//! Slot names follow the reference accessors, which do not always describe the geometry
//! (e.g. `neighbor_right` is really the next block below in the same buckets).

use pi_layout::model::block::BlockId;
use pi_layout::model::rects::Bounded;
use pi_layout::phases::DocPage;

use crate::consts::{NEIGHBOR_BUCKET_DIVISOR, NEIGHBOR_BUCKET_MIN_WIDTH};
use crate::model::clamp;

/// ref: heading_detection/neighbors.py::BlockNeighborCache
#[derive(Debug, Clone, Default)]
pub struct BlockNeighborCache {
    /// A body block crosses one of this block's buckets. ref slot: `state_slot`
    pub marked: bool,
    /// `neighbor_above`. ref slot: `tertiary_slot`
    pub above: Option<BlockId>,
    /// Block one column over. ref slot: `measure_slot`
    pub side: Option<BlockId>,
    /// `neighbor_right`. ref slot: `auxiliary_slot`
    pub right: Option<BlockId>,
    /// `body_neighbor_above`. ref slot: `primary_slot`
    pub body_above: Option<BlockId>,
    /// `closest_body_neighbor_above`. ref slot: `secondary_slot`
    pub closest_body: Option<BlockId>,
    /// Reverse link of `side`. ref slot: `option_slot`
    pub side_peer: Option<BlockId>,
}

/// ref: heading_detection/neighbors.py::PageNeighborMap
#[derive(Debug, Clone)]
pub struct PageNeighborMap {
    /// ref slot: `tertiary_slot`
    pub bucket_width: f64,
    /// ref slot: `secondary_slot`
    pub bucket_count: i64,
    /// Indexed by `orig_index`. ref slot: `primary_slot`
    pub entries: Vec<Option<BlockNeighborCache>>,
}

/// Inclusive bucket span `(start, end)` of a block.
// ref: heading_detection/neighbors.py::compute_bucket_span
fn bucket_span(m: &PageNeighborMap, b: &impl Bounded) -> (i64, i64) {
    let hi = (m.bucket_count - 1) as f64;
    let s = clamp((b.left_edge() / m.bucket_width).floor(), 0.0, hi) as i64;
    let e = clamp((b.right_edge() / m.bucket_width).ceil(), 0.0, hi) as i64;
    (s, e)
}

impl PageNeighborMap {
    // ref: heading_detection/neighbors.py::PageNeighborMap.__init__
    pub fn new(page: &DocPage) -> Self {
        let blocks: Vec<BlockId> = page.output.clone();
        let bw = page.layout.bounds.bbox_width();
        let tw = pi_pycompat::pymath::max(NEIGHBOR_BUCKET_MIN_WIDTH, bw / NEIGHBOR_BUCKET_DIVISOR);
        let n = (bw / tw).floor() as i64;
        let mut m = PageNeighborMap {
            bucket_width: tw,
            bucket_count: n,
            entries: vec![None; blocks.len().max(1) + 1],
        };
        let nu = n.max(0) as usize;
        let mut marked = vec![false; nu];
        for &id in &blocks {
            let b = &page.blocks[id];
            if !b.is_body_paragraph.get() {
                continue;
            }
            let (s, e) = bucket_span(&m, b);
            for i in s..e {
                if 0 <= i && i < n {
                    marked[i as usize] = true;
                }
            }
        }
        let mut recent_height: Vec<i64> = vec![-1; nu];
        let mut recent_block_index: Vec<i64> = vec![-1; nu.max(1)];
        let mut recent: Vec<Option<BlockId>> = vec![None; nu];
        let mut pending: Vec<Vec<usize>> = vec![Vec::new(); nu];
        let blk = |i: i64| -> Option<BlockId> {
            (0 <= i && (i as usize) < blocks.len()).then(|| blocks[i as usize])
        };

        for &cid in &blocks {
            let cur = &page.blocks[cid];
            if cur.char_count() == 0
                || cur.weighted_skew > 1.0
                || matches!(cur.kind.get(), 1 | 2 | 12)
            {
                continue;
            }
            let ci = cur.orig_index.get();
            let mut w = BlockNeighborCache::default();
            let (left, right) = bucket_span(&m, cur);
            let value = recent_block_index[left as usize];
            let adj_ix = if left > 0 && cur.left_edge() < (left as f64 + 0.5) * tw {
                left - 1
            } else if left < n - 1 {
                left + 1
            } else {
                left
            };
            let adjacent = recent_block_index[adj_ix as usize];
            if value >= 0 || adjacent >= 0 {
                let same = blk(value);
                let adjb = blk(adjacent);
                let picked = match same {
                    Some(s)
                        if adjb.is_none_or(|a| {
                            page.blocks[s].bottom_edge() < page.blocks[a].bottom_edge()
                        }) =>
                    {
                        value
                    }
                    _ => adjacent,
                };
                if let Some(pb) = blk(picked) {
                    w.side = Some(pb);
                    if let Some(Some(e)) = m.entries.get_mut(picked as usize) {
                        e.side_peer = Some(cid);
                    }
                }
            }
            recent_block_index[left as usize] = ci as i64;
            // Stash `w` so updates to other entries (and to itself through `recent`) apply.
            m.entries[ci] = Some(w);
            for col in left..right {
                if !(0 <= col && col < n) {
                    continue;
                }
                let c = col as usize;
                {
                    let w = m.entries[ci].as_mut().expect("entry");
                    w.marked = w.marked || marked[c];
                    if let Some(rb) = recent[c] {
                        let better = match w.body_above {
                            None => true,
                            Some(cur_b) => {
                                page.blocks[rb].bottom_edge() < page.blocks[cur_b].bottom_edge()
                            }
                        };
                        if better {
                            w.body_above = Some(rb);
                        }
                    }
                }
                let prev_ix = recent_height[c];
                recent_height[c] = ci as i64;
                if prev_ix >= 0 && (prev_ix as usize) < blocks.len() {
                    let same = blocks[prev_ix as usize];
                    {
                        let w = m.entries[ci].as_mut().expect("entry");
                        let better = match w.above {
                            None => true,
                            Some(a) => {
                                page.blocks[same].bottom_edge() < page.blocks[a].bottom_edge()
                            }
                        };
                        if better {
                            w.above = Some(same);
                        }
                    }
                    if let Some(Some(pc)) = m.entries.get_mut(prev_ix as usize) {
                        let better = match pc.right {
                            None => true,
                            Some(r) => cur.top_edge() > page.blocks[r].top_edge(),
                        };
                        if better {
                            pc.right = Some(cid);
                        }
                    }
                }
                if cur.is_body_paragraph.get() {
                    for &pi in &pending[c] {
                        if pi < m.entries.len()
                            && let Some(pn) = m.entries[pi].as_mut()
                        {
                            let better = match pn.closest_body {
                                None => true,
                                Some(cb) => cur.top_edge() > page.blocks[cb].top_edge(),
                            };
                            if better {
                                pn.closest_body = Some(cid);
                            }
                        }
                    }
                    recent[c] = Some(cid);
                    pending[c].clear();
                }
                pending[c].push(ci);
            }
        }
        m
    }

    fn entry(&self, b: &pi_layout::model::block::Block) -> Option<&BlockNeighborCache> {
        self.entries
            .get(b.orig_index.get())
            .and_then(|e| e.as_ref())
    }

    /// `wn_entry` lookup by `orig_index` (`None` past the end or for skipped blocks).
    pub fn at(&self, orig_index: usize) -> Option<&BlockNeighborCache> {
        self.entries.get(orig_index).and_then(|e| e.as_ref())
    }

    // ref: heading_detection/neighbors.py::neighbor_above
    pub fn above(&self, b: &pi_layout::model::block::Block) -> Option<BlockId> {
        self.entry(b).and_then(|e| e.above)
    }

    // ref: heading_detection/neighbors.py::body_neighbor_above
    pub fn body_above(&self, b: &pi_layout::model::block::Block) -> Option<BlockId> {
        self.entry(b).and_then(|e| e.body_above)
    }

    // ref: heading_detection/neighbors.py::neighbor_right
    pub fn right(&self, b: &pi_layout::model::block::Block) -> Option<BlockId> {
        self.entry(b).and_then(|e| e.right)
    }

    // ref: heading_detection/neighbors.py::closest_body_neighbor_above
    pub fn closest_body(&self, b: &pi_layout::model::block::Block) -> Option<BlockId> {
        self.entry(b).and_then(|e| e.closest_body)
    }
}
