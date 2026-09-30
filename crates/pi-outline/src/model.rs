//! Heading candidates, outline nodes and document accessors shared by stages 06-08.
//!
//! ref: pageindex/flash/outline_assembly/candidates.py
//!
//! Candidates are immutable once built and are compared by identity in the reference
//! (`is`, dict keys), so they live behind `Rc` and identity is `Rc::ptr_eq`. Outline nodes gain
//! children after they are placed, so their child list is a `RefCell`. Blocks are addressed by
//! `(page position, BlockId)`; every block field that later stages mutate is a `Cell` in
//! `pi_layout`, so the whole pass runs on `&Document`.

use std::cell::RefCell;
use std::rc::Rc;

use pi_layout::model::block::{Block, BlockId};
use pi_layout::model::rects::{Bounded, cmp_reading_order};
use pi_layout::model::span_line::{Line, Span};
use pi_layout::phases::{DocPage, Document};
use pi_layout::stats::scripts::{ScriptHistogram, dominant_script_family, tally_scripts};
use pi_layout::tokens::TokenView;

/// A numbering entry: the reference stores Python ints, and a float only when
/// `token_to_number` meets a non-integral value, so integral values print as ints.
pub type Num = f64;

/// ref: outline_assembly/candidates.py::HeadingCandidate
#[derive(Debug)]
pub struct HeadingCandidate {
    /// ref: `type`
    pub kind: u8,
    /// 0-based page position in `doc.pages`. ref: `page`
    pub page: usize,
    /// ref slot: `group_slot`
    pub block: BlockId,
    /// Closest body block (same page). ref slot: `tertiary_slot`
    pub anchor: Option<BlockId>,
    pub numbering: Vec<Num>,
    /// ref slot: `secondary_slot`
    pub prefix: Option<TokenView>,
    /// ref slot: `primary_slot`
    pub title: Option<TokenView>,
    pub has_numbering: bool,
    pub is_prominent: bool,
    /// Dominant script family of prefix + title. ref slot: `state_slot`
    pub script: u8,
    /// ref slot: `auxiliary_slot`
    pub y_frac: f64,
}

pub type Cand = Rc<HeadingCandidate>;

/// Identity key of a candidate (the reference keys dicts by the object).
pub fn cand_id(c: &Cand) -> usize {
    Rc::as_ptr(c) as usize
}

/// ref: outline_assembly/candidates.py::OutlineNode
#[derive(Debug)]
pub struct OutlineNode {
    pub heading: Cand,
    /// ref: `child_nodes`
    pub children: RefCell<Vec<Node>>,
}

pub type Node = Rc<OutlineNode>;

pub fn new_node(heading: Cand) -> Node {
    Rc::new(OutlineNode {
        heading,
        children: RefCell::new(Vec::new()),
    })
}

/// Read-only accessors mirroring the reference attribute paths.
#[derive(Clone, Copy)]
pub struct Doc<'a> {
    pub doc: &'a Document,
}

impl<'a> Doc<'a> {
    pub fn new(doc: &'a Document) -> Self {
        Doc { doc }
    }

    pub fn pages(&self) -> &'a [DocPage] {
        &self.doc.pages
    }

    pub fn page(&self, p: usize) -> &'a DocPage {
        &self.doc.pages[p]
    }

    /// 1-based `page_index`.
    pub fn page_index(&self, p: usize) -> usize {
        self.doc.pages[p].index()
    }

    pub fn block(&self, p: usize, id: BlockId) -> &'a Block {
        &self.doc.pages[p].blocks[id]
    }

    pub fn line(&self, p: usize, lid: usize) -> &'a Line {
        &self.doc.pages[p].layout.lines[lid]
    }

    pub fn span(&self, p: usize, sid: usize) -> &'a Span {
        &self.doc.pages[p].layout.spans[sid]
    }

    /// `block.line()`: the first line.
    pub fn first_line(&self, p: usize, b: &Block) -> &'a Line {
        self.line(p, b.first_line())
    }

    /// `last_line_of(block)`.
    pub fn last_line(&self, p: usize, b: &Block) -> &'a Line {
        self.line(p, b.last_line())
    }

    /// `first_span_of(block)`.
    pub fn first_span(&self, p: usize, b: &Block) -> &'a Span {
        self.span(p, self.first_line(p, b).spans[0])
    }

    /// `last_span(last_line_of(block))`.
    pub fn last_span(&self, p: usize, b: &Block) -> &'a Span {
        let l = self.last_line(p, b);
        self.span(p, *l.spans.last().expect("line has spans"))
    }

    /// `page.output_slot[i]` with the reference's bounds guard.
    pub fn output_at(&self, p: usize, i: i64) -> Option<BlockId> {
        let out = &self.doc.pages[p].output;
        (0 <= i && (i as usize) < out.len()).then(|| out[i as usize])
    }

    /// `column_index_of(block)`: the first line's column, -1 without lines.
    // ref: stats/aggregates.py::column_index_of
    pub fn column_index(&self, p: usize, b: &Block) -> i32 {
        b.lines.first().map_or(-1, |&l| self.line(p, l).column)
    }

    pub fn cblock(&self, c: &HeadingCandidate) -> &'a Block {
        self.block(c.page, c.block)
    }

    pub fn cpage_index(&self, c: &HeadingCandidate) -> usize {
        self.page_index(c.page)
    }
}

/// Viewport-normalized y of a PDF user-space point, as pdfium's page transform maps it.
// ref: outline_assembly/candidates.py::_viewport_y_fraction
pub fn viewport_y_fraction(viewport_box: [f64; 4], rot: i64, user_x: f64, user_y: f64) -> f64 {
    let [x_min, y_min, x_max, y_max] = viewport_box;
    let cx = (x_max + x_min) / 2.0;
    let cy = (y_max + y_min) / 2.0;
    let mut rotation = rot.rem_euclid(360);
    if rotation < 0 {
        rotation += 360;
    }
    let (xs, ys, sign): (f64, f64, i32) = match rotation {
        90 => (1.0, 0.0, 0),
        180 => (0.0, 1.0, -1),
        270 => (-1.0, 0.0, 0),
        _ => (0.0, -1.0, 1),
    };
    let (off, height) = if sign == 0 {
        ((cx - x_min).abs(), (x_max - x_min).abs())
    } else {
        ((cy - y_min).abs(), (y_max - y_min).abs())
    };
    let vy = xs * user_x + ys * user_y + (off - xs * cx - ys * cy);
    vy / if height != 0.0 { height } else { 1.0 }
}

impl HeadingCandidate {
    // ref: outline_assembly/candidates.py::HeadingCandidate.__init__
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        d: Doc,
        kind: u8,
        page: usize,
        block: BlockId,
        anchor: Option<BlockId>,
        numbering: Vec<Num>,
        prefix: Option<TokenView>,
        title: Option<TokenView>,
        has_numbering: bool,
        is_prominent: bool,
    ) -> Cand {
        let mut acc = ScriptHistogram::default();
        for tv in [&prefix, &title].into_iter().flatten() {
            for t in tv.iter() {
                tally_scripts(&mut acc, &t.text);
            }
        }
        let script = dominant_script_family(&acc);
        let pg = d.page(page);
        let b = d.block(page, block);
        let y_frac = match pg.layout.viewport_box {
            Some(vb) => viewport_y_fraction(vb, pg.layout.rot as i64, b.left_edge(), b.top_edge()),
            None => {
                let h = pg.layout.bounds.bbox_height();
                let h = if h != 0.0 { h } else { 1.0 };
                (pg.layout.bounds.top_edge() - b.top_edge()) / h
            }
        };
        Rc::new(HeadingCandidate {
            kind,
            page,
            block,
            anchor,
            numbering,
            prefix,
            title,
            has_numbering,
            is_prominent,
            script,
            y_frac,
        })
    }
}

/// `str(n)` of a numbering entry (int when integral).
pub fn num_str(n: Num) -> String {
    if n.fract() == 0.0 && n.abs() < 1e16 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// ref: outline_assembly/candidates.py::heading_order_key
pub fn heading_order_key(d: Doc, c: &HeadingCandidate) -> [f64; 6] {
    let b = d.cblock(c);
    [
        d.cpage_index(c) as f64,
        d.column_index(c.page, b) as f64,
        -b.top_edge(),
        -b.bottom_edge(),
        b.left_edge(),
        b.right_edge(),
    ]
}

/// Column index first, then reading position (raw deltas; callers use the sign).
// ref: outline_assembly/candidates.py::_compare_block_order, cliques.py::compare_block_order
pub fn compare_block_order(d: Doc, p: usize, a: &Block, b: &Block) -> f64 {
    let (ca, cb) = (d.column_index(p, a), d.column_index(p, b));
    if ca != cb {
        return (ca - cb) as f64;
    }
    cmp_reading_order(a, b)
}

/// Order by page, then block reading position.
// ref: outline_assembly/candidates.py::compare_heading_order
pub fn compare_heading_order(d: Doc, a: &HeadingCandidate, b: &HeadingCandidate) -> f64 {
    let (pa, pb) = (d.cpage_index(a), d.cpage_index(b));
    if pa != pb {
        return pa as f64 - pb as f64;
    }
    compare_block_order(d, a.page, d.cblock(a), d.cblock(b))
}

/// `clamp` with NaN propagation. ref: heading_detection/text_checks.py::clamp
pub fn clamp(value: f64, lo: f64, hi: f64) -> f64 {
    let m = if hi < value { hi } else { value };
    if lo > m { lo } else { m }
}
