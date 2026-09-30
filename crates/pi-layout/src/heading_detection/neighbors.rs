//! Per-page block neighborhood maps and neighbor lookups.
//!
//! ref: pageindex/flash/heading_detection/neighbors.py

use pi_pycompat::pymath;

use crate::model::block::BlockId;
use crate::model::rects::Bounded;
use crate::phases::DocPage;
use crate::tokens::clamp_value;

/// ref: heading_detection/neighbors.py::BlockNeighborCache
#[derive(Debug, Clone, Default)]
pub struct BlockNeighborCache {
    /// A body block spans one of this block's buckets. ref slot: `state_slot`
    pub has_body_above: bool,
    /// ref slot: `tertiary_slot` (`neighbor_above`)
    pub above: Option<BlockId>,
    /// Block one column to the left. ref slot: `measure_slot`
    pub left_sibling: Option<BlockId>,
    /// ref slot: `auxiliary_slot` (`neighbor_right`)
    pub right: Option<BlockId>,
    /// ref slot: `primary_slot` (`body_neighbor_above`)
    pub body_above: Option<BlockId>,
    /// ref slot: `secondary_slot` (`closest_body_neighbor_above`)
    pub closest_body_above: Option<BlockId>,
    /// Block one column to the right. ref slot: `option_slot`
    pub right_sibling: Option<BlockId>,
}

/// ref: heading_detection/neighbors.py::PageNeighborMap
#[derive(Debug)]
pub struct PageNeighborMap {
    /// Indexed by `orig_index`. ref slot: `primary_slot`
    pub cache: Vec<Option<BlockNeighborCache>>,
}

// ref: heading_detection/neighbors.py::compute_bucket_span
fn bucket_span(bw: f64, nb: i64, left: f64, right: f64) -> (i64, i64) {
    let s = clamp_value((left / bw).floor(), 0.0, (nb - 1) as f64) as i64;
    let e = clamp_value((right / bw).ceil(), 0.0, (nb - 1) as f64) as i64;
    (s, e)
}

impl PageNeighborMap {
    // ref: heading_detection/neighbors.py::PageNeighborMap.__init__
    pub fn new(page: &DocPage) -> Self {
        let blocks = &page.output;
        let pw = page.layout.bounds.bbox_width();
        let bw = pymath::max(5.0, pw / 300.0);
        let nb = (pw / bw).floor() as i64;
        let nbu = nb.max(0) as usize;
        let mut cache: Vec<Option<BlockNeighborCache>> = vec![None; blocks.len().max(1) + 1];
        let mut marked = vec![false; nbu];
        for &bid in blocks {
            let b = &page.blocks[bid];
            if !b.is_body_paragraph.get() {
                continue;
            }
            let (s, e) = bucket_span(bw, nb, b.left_edge(), b.right_edge());
            for i in s..e {
                if 0 <= i && i < nb {
                    marked[i as usize] = true;
                }
            }
        }
        let mut recent_body: Vec<i64> = vec![-1; nbu];
        let mut recent_block_index: Vec<i64> = vec![-1; nbu.max(1)];
        let mut recent: Vec<Option<BlockId>> = vec![None; nbu];
        let mut pending: Vec<Vec<usize>> = vec![Vec::new(); nbu];
        let get = |v: i64| -> Option<BlockId> {
            (v >= 0 && (v as usize) < blocks.len()).then(|| blocks[v as usize])
        };
        for &bid in blocks {
            let cb = &page.blocks[bid];
            if cb.char_count() == 0 || cb.weighted_skew > 1.0 || matches!(cb.kind.get(), 1 | 2 | 12)
            {
                continue;
            }
            let oi = cb.orig_index.get();
            cache[oi] = Some(BlockNeighborCache::default());
            let (left, right) = bucket_span(bw, nb, cb.left_edge(), cb.right_edge());
            let value = recent_block_index[left as usize];
            let adj_pos = if left > 0 && cb.left_edge() < (left as f64 + 0.5) * bw {
                left - 1
            } else if left < nb - 1 {
                left + 1
            } else {
                left
            };
            let adj = recent_block_index[adj_pos as usize];
            if value >= 0 || adj >= 0 {
                let same = get(value).map(|id| &page.blocks[id]);
                let adjb = get(adj).map(|id| &page.blocks[id]);
                let picked = match same {
                    Some(s) if adjb.is_none_or(|a| s.bottom_edge() < a.bottom_edge()) => value,
                    _ => adj,
                };
                if let Some(pid) = get(picked) {
                    cache[oi].as_mut().expect("just set").left_sibling = Some(pid);
                    if let Some(Some(pc)) = cache.get_mut(picked as usize) {
                        pc.right_sibling = Some(bid);
                    }
                }
            }
            recent_block_index[left as usize] = oi as i64;
            for col in left..right {
                if !(0 <= col && col < nb) {
                    continue;
                }
                let c = col as usize;
                let w = cache[oi].as_mut().expect("just set");
                w.has_body_above = w.has_body_above || marked[c];
                if let Some(rb) = recent[c]
                    && w.body_above.is_none_or(|x| {
                        page.blocks[rb].bottom_edge() < page.blocks[x].bottom_edge()
                    })
                {
                    w.body_above = Some(rb);
                }
                let pbi = recent_body[c];
                recent_body[c] = oi as i64;
                if pbi >= 0 && (pbi as usize) < blocks.len() {
                    let sb = blocks[pbi as usize];
                    if w.above.is_none_or(|x| {
                        page.blocks[sb].bottom_edge() < page.blocks[x].bottom_edge()
                    }) {
                        w.above = Some(sb);
                    }
                    if let Some(pc) = cache[pbi as usize].as_mut()
                        && pc
                            .right
                            .is_none_or(|x| cb.top_edge() > page.blocks[x].top_edge())
                    {
                        pc.right = Some(bid);
                    }
                }
                if cb.is_body_paragraph.get() {
                    for &pi in &pending[c] {
                        if pi < cache.len()
                            && let Some(pn) = cache[pi].as_mut()
                            && pn
                                .closest_body_above
                                .is_none_or(|x| cb.top_edge() > page.blocks[x].top_edge())
                        {
                            pn.closest_body_above = Some(bid);
                        }
                    }
                    recent[c] = Some(bid);
                    pending[c].clear();
                }
                pending[c].push(oi);
            }
        }
        PageNeighborMap { cache }
    }

    fn entry(&self, orig_index: usize) -> Option<&BlockNeighborCache> {
        self.cache.get(orig_index).and_then(|c| c.as_ref())
    }

    // ref: heading_detection/neighbors.py::neighbor_above
    pub fn above(&self, oi: usize) -> Option<BlockId> {
        self.entry(oi).and_then(|c| c.above)
    }

    // ref: heading_detection/neighbors.py::body_neighbor_above
    pub fn body_above(&self, oi: usize) -> Option<BlockId> {
        self.entry(oi).and_then(|c| c.body_above)
    }

    // ref: heading_detection/neighbors.py::neighbor_right
    pub fn right(&self, oi: usize) -> Option<BlockId> {
        self.entry(oi).and_then(|c| c.right)
    }

    // ref: heading_detection/neighbors.py::closest_body_neighbor_above
    pub fn closest_body_above(&self, oi: usize) -> Option<BlockId> {
        self.entry(oi).and_then(|c| c.closest_body_above)
    }

    pub fn get(&self, oi: usize) -> Option<&BlockNeighborCache> {
        self.entry(oi)
    }
}
