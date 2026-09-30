//! Document-level state after stage 04: pages with their blocks, document statistics and the
//! recurring-text histogram.
//!
//! ref: pageindex/flash/main.py::DocumentState, phases/page_view.py::PageView (block fields)

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use pi_pycompat::pysort;

use super::PageLayout;
use crate::blocks::{BlockClusterContext, cluster_lines_into_blocks};
use crate::model::block::{Block, BlockId};
use crate::model::rects::{
    Bounded, center_aligned, left_aligned, right_aligned, tuple_lt, x_centers_close,
};
use crate::stats::{DocStats, compute_doc_stats};

/// A page with its blocks (the stage 04+ fields of the reference `PageView`).
#[derive(Debug)]
pub struct DocPage {
    pub layout: PageLayout,
    /// Block arena; `output` and `reading` hold ids into it.
    pub blocks: Vec<Block>,
    /// Blocks in original (clustering) order. ref slot: `output_slot`
    pub output: Vec<BlockId>,
    /// Blocks in reading order. ref slot: `secondary_slot`
    pub reading: Vec<BlockId>,
    /// ref slot: `measure_slot`
    pub has_caption: Cell<bool>,
    /// Title or references page. ref slot: `auxiliary_slot`
    pub title_or_refs: Cell<bool>,
    /// ref slot: `state_slot`
    pub has_body: Cell<bool>,
    /// ref slot: `style_slot`
    pub body_style_hashes: RefCell<HashSet<String>>,
}

impl DocPage {
    /// 1-based page number. ref: `page_index`
    pub fn index(&self) -> usize {
        self.layout.page as usize
    }

    pub fn block(&self, id: BlockId) -> &Block {
        &self.blocks[id]
    }
}

/// ref: main.py::DocumentState
#[derive(Debug)]
pub struct Document {
    /// ref slot: `primary_slot`
    pub pages: Vec<DocPage>,
    /// ref slot: `secondary_slot`
    pub stats: DocStats,
    /// Jenkins hash -> count. ref slot: `tertiary_slot`
    pub recurring: RefCell<HashMap<i32, u64>>,
}

/// Block reference across the document: (0-based page position, block id).
pub type BlockRef = (usize, BlockId);

// ref: phases/page_view.py::assign_reading_order
pub fn assign_reading_order(page: &mut DocPage, ids: Vec<BlockId>) {
    for (i, &id) in ids.iter().enumerate() {
        page.blocks[id].orig_index.set(i);
    }
    page.output = ids.clone();
    let mut reading = ids;
    {
        let (blocks, lines) = (&page.blocks, &page.layout.lines);
        pysort::sort_by_key_lt(
            &mut reading,
            |&id| {
                let b = &blocks[id];
                let col = b.lines.first().map_or(-1, |&l| lines[l].column);
                [
                    col as f64,
                    -b.top_edge(),
                    -b.bottom_edge(),
                    b.left_edge(),
                    b.right_edge(),
                ]
            },
            |p, q| tuple_lt(p, q),
        );
    }
    let bounds = page.layout.bounds;
    for idx in 0..reading.len() {
        let c = &page.blocks[reading[idx]];
        c.reading_order_index.set(idx);
        let r = reading.get(idx + 1).map(|&id| &page.blocks[id]);
        let iso = if c.center_aligned && x_centers_close(&bounds, c) {
            match r {
                None => true,
                Some(r) => {
                    r.top_edge() > c.bottom_edge()
                        || (!left_aligned(c, r, 1.0) && !right_aligned(c, r, 1.0))
                }
            }
        } else {
            match r {
                Some(r) if c.center_aligned => {
                    !left_aligned(c, r, 1.0)
                        && !right_aligned(c, r, 1.0)
                        && center_aligned(c, r, c.bbox_width() / 10.0)
                        && x_centers_close(&bounds, r)
                }
                _ => false,
            }
        };
        c.isolated_centered.set(iso);
    }
    page.reading = reading;
}

/// Stages 03-04 of `flash/main.py::extract_toc`: document statistics, then per page block
/// clustering and reading order.
pub fn build_document(layouts: Vec<PageLayout>) -> Document {
    let stats = compute_doc_stats(&layouts);
    let pages = layouts
        .into_iter()
        .map(|layout| {
            let list = {
                let ctx = BlockClusterContext {
                    doc_stats: &stats,
                    page_bbox: layout.bounds,
                    page_stats: &layout.stats,
                    page: &layout,
                    columns: &layout.columns,
                };
                cluster_lines_into_blocks(&ctx)
            };
            let mut page = DocPage {
                layout,
                blocks: list.arena,
                output: Vec::new(),
                reading: Vec::new(),
                has_caption: Cell::new(false),
                title_or_refs: Cell::new(false),
                has_body: Cell::new(false),
                body_style_hashes: RefCell::new(HashSet::new()),
            };
            assign_reading_order(&mut page, list.ids);
            page
        })
        .collect();
    Document {
        pages,
        stats,
        recurring: RefCell::new(HashMap::new()),
    }
}
