//! Page scan state and heading-candidate constructors.
//!
//! ref: pageindex/flash/heading_detection/candidates.py

use std::collections::HashSet;

use pi_layout::labels::caption_text::extract_structural_number;
use pi_layout::model::block::{Block, BlockId};
use pi_layout::model::rects::Bounded;
use pi_layout::model::span_line::line_avg_char_width;
use pi_layout::phases::DocPage;
use pi_layout::tokens::{
    TokenView, is_trimmable_token, token_numeric_value, tokenize_block, trim_trailing_punct,
};

use super::neighbors::PageNeighborMap;
use super::text_checks::matches_references;
use crate::model::{Cand, Doc, HeadingCandidate, Num};

/// Per-page heading scan state.
/// ref: heading_detection/candidates.py::PageScanState
pub struct PageScanState<'a> {
    /// ref slot: `secondary_slot`
    pub d: Doc<'a>,
    /// Page position. ref slot: `primary_slot`
    pub page: usize,
    /// `doc.pages[page_index - 2]` when `page_index >= 2`. ref slot: `state_slot`
    pub prev: Option<usize>,
    /// ref slot: `tertiary_slot`
    pub neighbors: PageNeighborMap,
    /// ref slot: `option_slot`
    pub out: Vec<Cand>,
    /// Blocks already pushed. ref slot: `measure_slot`
    pub pushed: HashSet<BlockId>,
}

impl<'a> PageScanState<'a> {
    // ref: heading_detection/candidates.py::PageScanState.__init__
    pub fn new(d: Doc<'a>, page: usize) -> Self {
        let pi = d.page_index(page);
        // `doc.primary_slot[page_index - 2]`: a list position, assuming pages are in order.
        let prev = (pi >= 2).then(|| pi - 2);
        PageScanState {
            d,
            page,
            prev,
            neighbors: PageNeighborMap::new(d.page(page)),
            out: Vec::new(),
            pushed: HashSet::new(),
        }
    }

    pub fn pg(&self) -> &'a DocPage {
        self.d.page(self.page)
    }

    pub fn b(&self, id: BlockId) -> &'a Block {
        self.d.block(self.page, id)
    }

    pub fn ob(&self, id: Option<BlockId>) -> Option<&'a Block> {
        id.map(|i| self.b(i))
    }

    pub fn tokens(&self, id: BlockId) -> TokenView {
        tokenize_block(self.b(id), &self.pg().layout)
    }

    /// Page block by `orig_index` (`page_scan.auxiliary_slot[i]` with bounds guard).
    pub fn output_at(&self, i: i64) -> Option<BlockId> {
        self.d.output_at(self.page, i)
    }
}

// ref: heading_detection/candidates.py::push_candidate
pub fn push_candidate(ps: &mut PageScanState, c: Cand) {
    ps.pushed.insert(c.block);
    ps.out.push(c);
}

/// Build a candidate, applying the spatial promotion of the structural-numbering flag.
// ref: heading_detection/candidates.py::make_heading_candidate
pub fn make_heading_candidate(
    ps: &PageScanState,
    kind: u8,
    id: BlockId,
    numbering: Vec<Num>,
    prefix: Option<TokenView>,
    title: Option<TokenView>,
    mut has_numbering: bool,
) -> Cand {
    let b = ps.b(id);
    let right = ps.ob(ps.neighbors.right(b));
    if !has_numbering
        && let Some(rn) = right
        && b.italic_frac > 0.9
        && let Some(t) = &title
        && let Some(last) = t.last()
        && is_trimmable_token(last)
    {
        let mh = ps.d.last_line(ps.page, b);
        if mh.bbox_width() > 0.7 * rn.bbox_width()
            && (mh.right_edge() - rn.right_edge()).abs() < 2.0 * line_avg_char_width(mh)
        {
            has_numbering = true;
        }
    }
    let prominent =
        kind == 7 || (!numbering.is_empty() && title.as_ref().is_some_and(matches_references));
    HeadingCandidate::new(
        ps.d,
        kind,
        ps.page,
        id,
        ps.neighbors.closest_body(b),
        numbering,
        prefix,
        title,
        has_numbering,
        prominent,
    )
}

/// Type-only candidate over the full block text.
// ref: heading_detection/candidates.py::make_plain_candidate
pub fn make_plain_candidate(ps: &PageScanState, kind: u8, id: BlockId) -> Cand {
    make_heading_candidate(ps, kind, id, Vec::new(), None, Some(ps.tokens(id)), false)
}

// ref: heading_detection/candidates.py::make_body_heading_candidate
pub fn make_body_heading_candidate(
    ps: &PageScanState,
    kind: u8,
    id: BlockId,
    tokens: &TokenView,
) -> Cand {
    make_heading_candidate(
        ps,
        kind,
        id,
        Vec::new(),
        None,
        Some(trim_trailing_punct(tokens)),
        true,
    )
}

/// Numbered-heading candidate behind the reject-guard chain.
// ref: heading_detection/candidates.py::make_numbered_candidate
pub fn make_numbered_candidate(
    ps: &PageScanState,
    id: BlockId,
    numbering: Vec<Num>,
    tokens: TokenView,
    title: TokenView,
) -> Option<Cand> {
    let b = ps.b(id);
    let lay = &ps.pg().layout;
    if title.length <= 0 {
        return None;
    }
    if title.length == 1
        && let Some(ft) = title.first()
        && ft.first_cat != 2
        && ft.first_cat != 4
        && ft.last_cat != 2
        && !b.isolated_centered.get()
    {
        return None;
    }
    if numbering.len() == 1
        && numbering[0] == 1.0
        && b.top_edge() < 0.3 * lay.bounds.bbox_height()
        && ps.neighbors.right(b).is_none()
        && let Some(lt) = title.last()
        && let Some(a) = lt.anchors.last()
        && a.line == Some(b.last_line())
    {
        return None;
    }
    let mut reject = false;
    if b.line_count() > 1 {
        let second_line = &lay.lines[b.lines[1]];
        if let (Some(fnt), Some(ftt)) = (tokens.first(), title.first()) {
            let left = lay.spans[fnt.first_anchor_span().expect("anchor span")].left_edge();
            let title_left = lay.spans[ftt.first_anchor_span().expect("anchor span")].left_edge();
            if !(left < title_left && second_line.left_edge() > (left + title_left) / 2.0) {
                let tt = tokenize_block(b, lay);
                let tt = tt.from(di_count(&tt, b.first_line()));
                match extract_structural_number(&tt, true) {
                    None => reject = false,
                    Some(tt) if tt.length <= 0 => reject = false,
                    Some(_) if b.caption_claimed.get() => reject = true,
                    Some(tt) => {
                        if numbering.len() == 1 && tt.length <= 2 {
                            reject = match tt.token_at(0) {
                                Some(f) => {
                                    let v = token_numeric_value(f);
                                    !v.is_nan() && v == numbering[0] + 1.0
                                }
                                None => false,
                            };
                        }
                    }
                }
            }
        }
    }
    if reject {
        return None;
    }
    Some(make_heading_candidate(
        ps,
        1,
        id,
        numbering,
        Some(tokens),
        Some(title),
        false,
    ))
}

/// Count the leading tokens that belong to `line`.
// ref: heading_detection/candidates.py::_di_count
pub fn di_count(tokens: &TokenView, line: usize) -> i64 {
    let mut n = 0;
    for i in 0..tokens.length {
        match tokens.token_at(i) {
            Some(t) if t.line() == Some(line) => n += 1,
            _ => break,
        }
    }
    n
}
