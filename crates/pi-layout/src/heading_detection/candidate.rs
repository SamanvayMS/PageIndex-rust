//! Heading candidates, outline nodes, ordering and style-cluster context.
//!
//! ref: pageindex/flash/outline_assembly/candidates.py, outline_assembly/style_context.py
//! (the parts `heading_detection.find_section_openers` uses).

use std::collections::HashMap;

use crate::model::block::{Block, BlockId, heading_score};
use crate::model::rects::{Bounded, cmp_reading_order, tuple_lt};
use crate::phases::{DocPage, Document};
use crate::stats::scripts::{ScriptHistogram, dominant_script_family, tally_scripts};
use crate::tokens::{TokenView, is_char_token};

/// ref: outline_assembly/candidates.py::HeadingCandidate
#[derive(Debug, Clone)]
pub struct HeadingCandidate {
    pub kind: u8,
    /// 0-based page position.
    pub page: usize,
    /// ref slot: `group_slot`
    pub block: BlockId,
    /// Closest body block above. ref slot: `tertiary_slot`
    pub anchor: Option<BlockId>,
    /// Integral values are ints in the reference (`int(...)`); others are floats.
    pub numbering: Vec<f64>,
    /// Prefix tokens. ref slot: `secondary_slot`
    pub prefix: Option<TokenView>,
    /// Title tokens. ref slot: `primary_slot`
    pub title: Option<TokenView>,
    pub has_numbering: bool,
    pub is_prominent: bool,
    /// Dominant script family of prefix + title. ref slot: `state_slot`
    pub script: u8,
    /// Viewport-normalized y of the block's top-left. ref slot: `auxiliary_slot`
    pub y_frac: f64,
}

/// ref: outline_assembly/candidates.py::OutlineNode
#[derive(Debug, Clone)]
pub struct OutlineNode {
    pub heading: HeadingCandidate,
    pub children: Vec<OutlineNode>,
}

// ref: outline_assembly/candidates.py::_viewport_y_fraction
pub fn viewport_y_fraction(vb: [f64; 4], rot: i64, x: f64, y: f64) -> f64 {
    let [x0, y0, x1, y1] = vb;
    let cx = (x1 + x0) / 2.0;
    let cy = (y1 + y0) / 2.0;
    let r = rot.rem_euclid(360);
    let (xs, ys, xsign): (f64, f64, i32) = match r {
        90 => (1.0, 0.0, 0),
        180 => (0.0, 1.0, -1),
        270 => (-1.0, 0.0, 0),
        _ => (0.0, -1.0, 1),
    };
    let (off, height) = if xsign == 0 {
        ((cx - x0).abs(), (x1 - x0).abs())
    } else {
        ((cy - y0).abs(), (y1 - y0).abs())
    };
    let vy = xs * x + ys * y + (off - xs * cx - ys * cy);
    vy / if height != 0.0 { height } else { 1.0 }
}

impl HeadingCandidate {
    // ref: outline_assembly/candidates.py::HeadingCandidate.__init__
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        doc: &Document,
        page: usize,
        kind: u8,
        block: BlockId,
        anchor: Option<BlockId>,
        numbering: Vec<f64>,
        prefix: Option<TokenView>,
        title: Option<TokenView>,
        has_numbering: bool,
        is_prominent: bool,
    ) -> Self {
        let mut acc = ScriptHistogram::default();
        for tv in [&prefix, &title].into_iter().flatten() {
            for t in tv.iter() {
                tally_scripts(&mut acc, &t.text);
            }
        }
        let p = &doc.pages[page];
        let b = &p.blocks[block];
        let y_frac = match p.layout.viewport_box {
            Some(vb) => viewport_y_fraction(vb, p.layout.rot as i64, b.left_edge(), b.top_edge()),
            None => {
                let h = p.layout.bounds.bbox_height();
                (p.layout.bounds.top - b.top_edge()) / if h != 0.0 { h } else { 1.0 }
            }
        };
        HeadingCandidate {
            kind,
            page,
            block,
            anchor,
            numbering,
            prefix,
            title,
            has_numbering,
            is_prominent,
            script: dominant_script_family(&acc),
            y_frac,
        }
    }

    pub fn block<'a>(&self, doc: &'a Document) -> &'a Block {
        &doc.pages[self.page].blocks[self.block]
    }

    pub fn doc_page<'a>(&self, doc: &'a Document) -> &'a DocPage {
        &doc.pages[self.page]
    }
}

/// Python `str()` of a numbering value (ints print without a fraction).
pub fn format_number(v: f64) -> String {
    if v.is_finite() && v == v.trunc() && v.abs() < 1e16 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

fn column_of(doc: &Document, c: &HeadingCandidate) -> i32 {
    let p = &doc.pages[c.page];
    p.blocks[c.block]
        .lines
        .first()
        .map_or(-1, |&l| p.layout.lines[l].column)
}

// ref: outline_assembly/candidates.py::compare_heading_order (+ _compare_block_order)
pub fn compare_heading_order(doc: &Document, a: &HeadingCandidate, b: &HeadingCandidate) -> f64 {
    let (pa, pb) = (doc.pages[a.page].index(), doc.pages[b.page].index());
    if pa != pb {
        return pa as f64 - pb as f64;
    }
    let (ca, cb) = (column_of(doc, a), column_of(doc, b));
    if ca != cb {
        return (ca - cb) as f64;
    }
    cmp_reading_order(a.block(doc), b.block(doc))
}

// ref: outline_assembly/candidates.py::heading_order_key
pub fn heading_order_key(doc: &Document, c: &HeadingCandidate) -> [f64; 6] {
    let b = c.block(doc);
    [
        doc.pages[c.page].index() as f64,
        column_of(doc, c) as f64,
        -b.top_edge(),
        -b.bottom_edge(),
        b.left_edge(),
        b.right_edge(),
    ]
}

// ref: outline_assembly/candidates.py::heading_signature
pub fn heading_signature(c: &HeadingCandidate) -> String {
    if !c.numbering.is_empty() {
        let nums: Vec<String> = c.numbering.iter().map(|&n| format_number(n)).collect();
        return format!("{}|{}", c.kind, nums.join(","));
    }
    let mut s = format!("{}|", c.kind);
    if let Some(t) = &c.title {
        for tok in t.iter() {
            if is_char_token(tok) {
                s.push_str(&pi_pycompat::unicode::lower(&tok.text));
            }
        }
    }
    s
}

/// A style bucket of candidates (indices into the caller's candidate list).
/// ref: outline_assembly/style_context.py::StyleCluster
#[derive(Debug, Default)]
pub struct StyleCluster {
    /// Signature -> candidate. ref slot: `state_slot`
    by_signature: HashMap<String, usize>,
    /// Sorted by (heading_score, heading_order_key), set semantics. ref slot: `secondary_slot`
    sorted: Vec<usize>,
    /// Min / max by heading order. ref slots: `primary_slot` / `tertiary_slot`
    min: Option<usize>,
    max: Option<usize>,
}

fn cluster_key(doc: &Document, c: &HeadingCandidate) -> [f64; 7] {
    let k = heading_order_key(doc, c);
    [
        heading_score(c.block(doc)),
        k[0],
        k[1],
        k[2],
        k[3],
        k[4],
        k[5],
    ]
}

impl StyleCluster {
    pub fn size(&self) -> usize {
        self.sorted.len()
    }

    // ref: outline_assembly/style_context.py::StyleCluster.add
    pub fn add(&mut self, doc: &Document, cands: &[HeadingCandidate], i: usize) {
        let c = &cands[i];
        self.by_signature.insert(heading_signature(c), i);
        let k = cluster_key(doc, c);
        let at = self
            .sorted
            .partition_point(|&j| tuple_lt(&cluster_key(doc, &cands[j]), &k));
        let equal = at < self.sorted.len()
            && crate::model::rects::tuple_eq(&cluster_key(doc, &cands[self.sorted[at]]), &k);
        if !equal {
            self.sorted.insert(at, i);
        }
        if self
            .min
            .is_none_or(|m| compare_heading_order(doc, c, &cands[m]) < 0.0)
        {
            self.min = Some(i);
        }
        if self
            .max
            .is_none_or(|m| compare_heading_order(doc, c, &cands[m]) > 0.0)
        {
            self.max = Some(i);
        }
    }

    // ref: outline_assembly/style_context.py::StyleCluster.has_nearby_duplicate
    pub fn has_nearby_duplicate(
        &self,
        doc: &Document,
        cands: &[HeadingCandidate],
        c: &HeadingCandidate,
    ) -> bool {
        self.by_signature
            .get(&heading_signature(c))
            .is_some_and(|&e| {
                (doc.pages[c.page].index() as i64 - doc.pages[cands[e].page].index() as i64).abs()
                    < 20
            })
    }

    // ref: outline_assembly/candidates.py::is_in_oo_range
    pub fn in_order_range(
        &self,
        doc: &Document,
        cands: &[HeadingCandidate],
        c: &HeadingCandidate,
    ) -> bool {
        match (self.min, self.max) {
            (Some(lo), Some(hi)) => {
                compare_heading_order(doc, c, &cands[lo]) >= 0.0
                    && compare_heading_order(doc, c, &cands[hi]) <= 0.0
            }
            _ => false,
        }
    }
}

/// ref: outline_assembly/style_context.py::OutlineContext
#[derive(Debug, Default)]
pub struct OutlineContext {
    /// type 10. ref slot: `secondary_slot`
    pub appendix: StyleCluster,
    /// type 8. ref slot: `primary_slot`
    pub chapter: StyleCluster,
    /// numbered. ref slot: `auxiliary_slot`
    pub numbered: StyleCluster,
    /// everything else. ref slot: `tertiary_slot`
    pub other: StyleCluster,
}

impl OutlineContext {
    pub fn new(doc: &Document, cands: &[HeadingCandidate], members: &[usize]) -> Self {
        let mut ctx = OutlineContext::default();
        for &i in members {
            ctx.add(doc, cands, i);
        }
        ctx
    }

    // ref: outline_assembly/style_context.py::pick_style_bucket
    fn bucket(&self, c: &HeadingCandidate) -> &StyleCluster {
        if c.kind == 10 {
            &self.appendix
        } else if c.kind == 8 {
            &self.chapter
        } else if !c.numbering.is_empty() {
            &self.numbered
        } else {
            &self.other
        }
    }

    pub fn add(&mut self, doc: &Document, cands: &[HeadingCandidate], i: usize) {
        let c = &cands[i];
        let b = if c.kind == 10 {
            &mut self.appendix
        } else if c.kind == 8 {
            &mut self.chapter
        } else if !c.numbering.is_empty() {
            &mut self.numbered
        } else {
            &mut self.other
        };
        b.add(doc, cands, i);
    }

    pub fn has_nearby_duplicate(
        &self,
        doc: &Document,
        cands: &[HeadingCandidate],
        c: &HeadingCandidate,
    ) -> bool {
        self.bucket(c).has_nearby_duplicate(doc, cands, c)
    }

    // ref: outline_assembly/style_context.py::has_conflict_in_context
    pub fn has_conflict(
        &self,
        doc: &Document,
        cands: &[HeadingCandidate],
        c: &HeadingCandidate,
    ) -> bool {
        (c.kind != 10 && self.appendix.in_order_range(doc, cands, c))
            || (c.kind != 8 && self.chapter.in_order_range(doc, cands, c))
            || (c.numbering.is_empty() && self.numbered.in_order_range(doc, cands, c))
            || (c.kind != 8 && !c.numbering.is_empty() && self.chapter.size() > 0)
    }
}
