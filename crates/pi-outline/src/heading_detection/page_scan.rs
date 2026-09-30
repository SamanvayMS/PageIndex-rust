//! Whole-page heading scan, document-level candidate collection/filtering, section openers.
//!
//! ref: pageindex/flash/heading_detection/page_scan.py

use pi_layout::labels::caption_text::{advance_past_line, trie_matches_all};
use pi_layout::model::block::{BlockId, heading_score, is_caps_heavy};
use pi_layout::model::char_stats::{info_weight, is_upper_dominant, letter_count};
use pi_layout::model::rects::{
    Bounded, center_aligned, intervals_overlap, rect_contains, tuple_lt, x_aligned,
};
use pi_layout::model::span_line::style_key;
use pi_layout::phases::DocPage;
use pi_layout::tokens::{is_char_token, is_word_token, token_numeric_value};

use super::candidates::{
    PageScanState, make_body_heading_candidate, make_plain_candidate, push_candidate,
};
use super::detectors::{
    detect_chapter_appendix, detect_labeled_heading, detect_numbered_heading,
    has_competing_labeled_heading, passes_neighbor_check, try_classify_heading,
};
use super::keyword_tables::{INTRODUCTION_SECTION_TRIE, SECTION_KEYWORDS_TRIE};
use super::style_detectors::{detect_font_heading, detect_heading_with_body};
use super::text_checks::{
    has_substantive_content, is_cover_page, is_heading_continuation, matches_abstract,
    matches_references,
};
use crate::model::{Cand, Doc, Node, Num, new_node};
use crate::outline_assembly::style_context::{OutlineContext, has_conflict_in_context};

const INF: f64 = f64::INFINITY;

/// Heading candidates found on the page.
// ref: heading_detection/page_scan.py::scan_page_headings
// `!(a >= b)` / `!(a < b)` are kept: they differ from `a < b` / `a >= b` on NaN.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
pub fn scan_page_headings(ps: &mut PageScanState) -> Vec<Cand> {
    let d = ps.d;
    let pg = ps.pg();
    let lay = &pg.layout;
    if is_cover_page(d.doc, pg) {
        return Vec::new();
    }
    ps.out.clear();
    ps.pushed.clear();
    let body = d.doc.stats.body_font_size;
    let st = &lay.stats;
    for &id in &pg.output {
        let b = ps.b(id);
        if b.char_count() == 0 || b.weighted_skew > 1.0 || b.kind.get() != 0 {
            continue;
        }
        if b.region_label.get() != 0 {
            continue;
        }
        let above_id = ps.neighbors.above(b);
        let above = ps.ob(above_id);
        if above.is_some_and(|a| rect_contains(&a.bbox, &b.bbox)) {
            continue;
        }
        if b.char_count() <= 1 && b.char_stats.first_cat != 4 {
            continue;
        }
        let wn = ps.neighbors.at(b.orig_index.get()).cloned();
        let has_da_above = wn.as_ref().is_some_and(|w| w.marked);
        let rows = b.line_count();
        if has_da_above
            && rows > 1
            && ((0.0 < b.bold_frac && b.bold_frac < 1.0)
                || style_key(d.first_span(ps.page, b)) != style_key(d.last_span(ps.page, b)))
            && let Some(lo) = detect_heading_with_body(ps, id)
        {
            push_candidate(ps, lo);
            continue;
        }
        let tokens = ps.tokens(id);
        let first_line_id = b.first_line();
        let first_line = &lay.lines[first_line_id];
        let flt = tokens.slice(0, advance_past_line(&tokens, first_line_id, 0));
        if has_da_above
            && !b.caption_claimed.get()
            && rows >= 3
            && first_line.bbox_width()
                <= 0.2
                    * pi_pycompat::pymath::min(
                        lay.lines[b.lines[1]].bbox_width(),
                        lay.lines[b.lines[2]].bbox_width(),
                    )
            && matches_abstract(&flt)
        {
            let c = make_body_heading_candidate(ps, 5, id, &flt);
            push_candidate(ps, c);
            continue;
        }
        if b.char_count() >= 200 {
            continue;
        }
        if b.char_count() >= 100
            && rows > 1
            && b.char_stats.category_counts[6] as i64
                - first_line.char_stats.category_counts[6] as i64
                > 1
        {
            continue;
        }
        let size = b.weighted_font_size;
        let page_width = lay.bounds.bbox_width();
        let lots_caps = b.char_stats.category_counts[2] as f64
            >= pi_pycompat::pymath::max(3.0, letter_count(&b.char_stats) as f64 / 2.0);
        if rows > 4 || (rows >= 3 && !(size >= 1.5 * st.median_font_size || lots_caps)) {
            continue;
        }
        if b.density_chars < 0.5 * d.doc.stats.p80_density {
            continue;
        }
        if letter_count(&b.char_stats) == 0 {
            continue;
        }
        if b.bold_frac < 0.1 && !lots_caps && size < body - 2.0 {
            continue;
        }
        if let Some(g) = detect_chapter_appendix(ps, id) {
            push_candidate(ps, g);
            continue;
        }
        if passes_neighbor_check(ps, id) {
            continue;
        }
        let top_gap = above.map_or(INF, |a| a.bottom_edge() - b.top_edge());
        let isolated = !b.caption_claimed.get()
            && rows <= 2
            && above.is_none_or(|a| {
                top_gap > 1.5 * size || (a.region_label.get() != 5 && a.region_label.get() != 11)
            });
        if isolated {
            if matches_references(&tokens) {
                let c = make_plain_candidate(ps, 7, id);
                push_candidate(ps, c);
                continue;
            }
            if has_da_above && trie_matches_all(&INTRODUCTION_SECTION_TRIE, &tokens) {
                let c = make_plain_candidate(ps, 11, id);
                push_candidate(ps, c);
                continue;
            }
        }
        let o = b.orig_index.get() as i64;
        let previous_block = ps.ob(ps.output_at(o - 1));
        let below = ps.ob(ps.output_at(o + 1));
        if !has_da_above
            && (!(heading_score(b) >= st.median_font_size + 1.5)
                || above.is_some_and(|a| a.kind.get() != 1)
                || previous_block.is_some_and(|p| p.kind.get() != 1)
                || below.is_some_and(|x| !(x.top_edge() < b.bottom_edge() - size)))
        {
            continue;
        }
        if b.bold_frac < 0.1
            && !lots_caps
            && d.first_span(ps.page, b).font_name == st.dominant_font
            && size < body - 0.5
        {
            continue;
        }
        let pred_id = ps.neighbors.right(b);
        let pred = ps.ob(pred_id);
        if pred.is_some_and(|p| p.weighted_skew > 1.0) {
            continue;
        }
        if top_gap < 0.0 {
            continue;
        }
        let pred_gap = pred.map_or(INF, |p| b.bottom_edge() - p.top_edge());
        if pred_gap < -0.9 * b.bbox_height() {
            continue;
        }
        let line_gap = st.median_overlap_gap - st.median_font_size;
        if top_gap < line_gap && size < st.median_font_size - 1.0 {
            continue;
        }
        let right_sib = wn.as_ref().and_then(|w| w.side);
        let left_sib = wn.as_ref().and_then(|w| w.side_peer);

        if let Some(hk) = detect_numbered_heading(ps, id, &tokens) {
            let col_idx = if b.lines.is_empty() {
                0
            } else {
                d.column_index(ps.page, b)
            };
            let column_rect = (0 <= col_idx && (col_idx as usize) < lay.columns.len())
                .then(|| &lay.columns[col_idx as usize]);
            let first_number: Num = hk.numbering.first().copied().unwrap_or(0.0);
            if size < body - 1.0
                && b.bbox_width() < 0.2 * page_width
                && let Some(cr) = column_rect
                && cr.bbox_width() < 0.2 * page_width
                && cr.bbox_height() > 1.5 * cr.bbox_width()
            {
                let mismatch = |x: Option<BlockId>| {
                    x.is_some_and(|x| {
                        ps.tokens(x)
                            .first()
                            .is_some_and(|f| f.kind == 1 && token_numeric_value(f) != first_number)
                    })
                };
                if mismatch(above_id) {
                    continue;
                }
                if mismatch(pred_id) {
                    continue;
                }
            }
            let ftv = tokens.token_at(0);
            let second = if tokens.length > 1 {
                tokens.token_at(1)
            } else {
                None
            };
            if hk.numbering.len() <= 1
                && (ftv.is_some_and(|t| t.boundary) || second.is_some_and(is_word_token))
            {
                if let Some(a) = above
                    && is_heading_continuation(pg, a, b, first_number)
                {
                    continue;
                }
                if let Some(rs) = right_sib
                    && above_id != Some(rs)
                    && !center_aligned(b, ps.b(rs), 1.0)
                    && {
                        // The reference dereferences `above` here unguarded.
                        let a = above.expect("reference raises on a missing above block");
                        a.line_count() < 10 || a.char_count() < 300
                    }
                    && is_heading_continuation(pg, ps.b(rs), b, first_number)
                {
                    continue;
                }
                if let Some(p) = pred
                    && is_heading_continuation(pg, p, b, first_number)
                {
                    continue;
                }
                if let Some(ls) = left_sib
                    && pred_id != Some(ls)
                    && !center_aligned(b, ps.b(ls), 1.0)
                    && {
                        let p = pred.expect("reference raises on a missing predecessor");
                        p.line_count() < 10 || p.char_count() < 300
                    }
                    && is_heading_continuation(pg, ps.b(ls), b, first_number)
                {
                    continue;
                }
            }
            let fs = d.first_span(ps.page, b);
            if b.top_edge() < lay.bounds.bbox_height() / 4.0
                && size <= st.median_font_size
                && fs.char_stats.first_cat == 1
                && fs.bbox_height() < size - 0.5
                && hk.numbering.len() <= 1
                && second.is_some_and(is_char_token)
            {
                continue;
            }
            push_candidate(ps, hk);
            continue;
        }
        if b.caption_claimed.get() {
            continue;
        }
        if b.char_count() >= 120 {
            continue;
        }
        if top_gap <= line_gap - 0.1 {
            continue;
        }
        if let Some(hs) = detect_labeled_heading(ps, id, &tokens) {
            if let Some(a) = above_id
                && has_competing_labeled_heading(ps, &hs, a)
            {
                continue;
            }
            if let Some(rs) = right_sib
                && above_id != Some(rs)
                && has_competing_labeled_heading(ps, &hs, rs)
            {
                continue;
            }
            if let Some(p) = pred_id
                && has_competing_labeled_heading(ps, &hs, p)
            {
                continue;
            }
            if let Some(ls) = left_sib
                && pred_id != Some(ls)
                && has_competing_labeled_heading(ps, &hs, ls)
            {
                continue;
            }
            push_candidate(ps, hs);
            continue;
        }
        if isolated {
            let caps_heavy =
                size + 2.0 * b.bold_frac >= body + 4.0 || is_upper_dominant(&b.char_stats);
            let aoo = ps.ob(ps.neighbors.closest_body(b));
            let type5 = caps_heavy
                || aoo.is_some_and(|ao| {
                    let p = pred.expect("closest body neighbor implies a predecessor");
                    b.bottom_edge() - ao.top_edge() < 3.0 * (b.bottom_edge() - p.top_edge())
                        || info_weight(&p.char_stats) >= 30.0
                });
            if type5 && matches_abstract(&tokens) {
                let c = make_plain_candidate(ps, 5, id);
                push_candidate(ps, c);
                continue;
            }
            let type6 = caps_heavy
                || above.is_some_and(|a| {
                    x_aligned(b, a, 1.0)
                        && (a.bbox_width() >= page_width / 6.0
                            || a.bold_frac > 0.9
                            || is_upper_dominant(&a.char_stats))
                })
                || pred.is_some_and(|p| {
                    x_aligned(b, p, 1.0)
                        && (p.bbox_width() >= page_width / 6.0
                            || p.bold_frac > 0.9
                            || is_upper_dominant(&p.char_stats)
                            || (b.italic_frac < 0.1 && p.italic_frac > 0.9))
                });
            if type6 && SECTION_KEYWORDS_TRIE.full_match(&tokens) {
                let c = make_plain_candidate(ps, 6, id);
                push_candidate(ps, c);
                continue;
            }
        }
        if info_weight(&b.char_stats) <= 3.0 {
            continue;
        }
        if has_substantive_content(pg, b, previous_block, below) {
            continue;
        }
        if let Some(c) = detect_font_heading(ps, id) {
            push_candidate(ps, c);
        }
    }
    ps.out.clone()
}

/// Document-level state aggregating per-page candidates.
/// ref: heading_detection/page_scan.py::DocCandidateCollector
pub struct DocCandidateCollector<'a> {
    /// Section openers. ref slot: `measure_slot`
    labeled: &'a [Node],
    /// Next labeled entry to consume. ref slot: `option_slot`
    next_labeled: usize,
    /// A page has been filtered. ref slot: `auxiliary_slot`
    seen_page: bool,
    /// Max first number of numbered headings. ref slot: `primary_slot`
    max_number: Num,
    /// A letter sequence starting at 1 was seen. ref slot: `secondary_slot`
    letter_started: bool,
    /// A Roman sequence starting at 1 was seen. ref slot: `tertiary_slot`
    roman_started: bool,
    /// ref slot: `state_slot`
    pub accepted: Vec<Cand>,
}

/// Per-page filter: noisy pages, title overlap, page headers, numbering continuity.
// ref: heading_detection/page_scan.py::filter_page_candidates
pub fn filter_page_candidates(
    d: Doc,
    dc: &mut DocCandidateCollector,
    p: usize,
    mut cands: Vec<Cand>,
) {
    let page: &DocPage = d.page(p);
    let pi = page.index();
    while dc.next_labeled < dc.labeled.len() {
        let c = &dc.labeled[dc.next_labeled].heading;
        if d.cpage_index(c) > pi {
            break;
        }
        if c.kind == 2 {
            if !dc.roman_started {
                dc.roman_started = c.numbering.first() == Some(&1.0);
            }
        } else if c.kind == 4 {
            if !dc.letter_started {
                dc.letter_started = c.numbering.first() == Some(&1.0);
            }
        } else if c.kind == 1 && !c.numbering.is_empty() {
            dc.max_number = pi_pycompat::pymath::max(dc.max_number, c.numbering[0]);
        }
        dc.next_labeled += 1;
    }
    let count = cands.len();
    if count >= 20 {
        return;
    }
    pi_pycompat::pysort::sort_by_key_lt(
        &mut cands,
        |c| {
            let b = d.cblock(c);
            let col = b.lines.first().map_or(-1, |&l| page.layout.lines[l].column);
            [
                col as f64,
                -b.top_edge(),
                -b.bottom_edge(),
                b.left_edge(),
                b.right_edge(),
            ]
        },
        |a, b| tuple_lt(a, b),
    );
    let mut accepted: Vec<Cand> = Vec::new();
    let title = if !dc.seen_page && page.title_or_refs.get() {
        page.output
            .iter()
            .map(|&i| &page.blocks[i])
            .find(|b| b.kind.get() == 3)
    } else {
        None
    };
    let mut min_first = INF;
    let mut total_bottom = INF;
    let mut singles = 0;
    for c in &cands {
        let b = d.cblock(c);
        if total_bottom == INF && c.kind == 5 {
            total_bottom = b.top_edge() + b.weighted_font_size;
        }
        if c.kind == 1 && !c.numbering.is_empty() {
            min_first = pi_pycompat::pymath::min(min_first, c.numbering[0]);
            if c.numbering.len() == 1 {
                singles += 1;
            }
        }
    }
    let mut max_first: Num = 0.0;
    for index in 0..count {
        let ac = &cands[index];
        let ab = d.cblock(ac);
        let next = cands.get(index + 1);
        if !dc.seen_page && ac.kind != 11 {
            let bottom = ab.top_edge();
            if let Some(t) = title
                && bottom > t.top_edge()
                && intervals_overlap(
                    ab.left_edge(),
                    ab.right_edge(),
                    t.left_edge(),
                    t.right_edge(),
                )
            {
                continue;
            }
            if bottom > total_bottom {
                continue;
            }
        }
        if ac.kind == 0
            && let Some(nx) = next
            && nx.kind == 5
        {
            let nb = d.cblock(nx);
            if ab.left_edge() <= nb.right_edge()
                && ab.right_edge() >= nb.left_edge()
                && ab.bottom_edge() - nb.top_edge() < 2.0 * ab.bbox_height()
                && pi_layout::tokens::tokenize_block(ab, &page.layout).length > 1
            {
                continue;
            }
        }
        if ac.kind == 2 {
            if ac.numbering.first() == Some(&1.0) {
                dc.roman_started = true;
            } else if !dc.roman_started {
                continue;
            }
            accepted.push(ac.clone());
            continue;
        }
        if ac.kind == 4 {
            if ac.numbering.first() == Some(&1.0) {
                dc.letter_started = true;
            } else if !dc.letter_started {
                continue;
            }
            accepted.push(ac.clone());
            continue;
        }
        if ac.kind != 1 {
            accepted.push(ac.clone());
            continue;
        }
        if singles >= 5 {
            continue;
        }
        let first = ac.numbering.first().copied().unwrap_or(0.0);
        if ab.bold_frac < 0.9
            && !is_caps_heavy(&ab.char_stats)
            && ab.weighted_font_size < page.layout.stats.median_font_size + 1.0
        {
            if dc.max_number <= 0.0 && min_first > 1.0 && ac.numbering.len() <= 1 {
                continue;
            }
            if first > 3.0 * pi as f64 {
                continue;
            }
        }
        max_first = pi_pycompat::pymath::max(max_first, first);
        accepted.push(ac.clone());
    }
    dc.accepted.extend(accepted);
    dc.seen_page = true;
    dc.max_number = pi_pycompat::pymath::max(dc.max_number, max_first);
}

/// Sparse early page that behaves like a cover.
// ref: title/scoring.py::is_cover_like_page
pub fn is_cover_like_page(d: Doc, page: &DocPage) -> bool {
    if page.has_caption.get() {
        return false;
    }
    let threshold = 0.5 * pi_pycompat::pymath::min(d.doc.stats.median_page_weight, 5e3);
    let w = page.layout.stats.total_line_weight;
    if page.index() <= 1 && w < threshold {
        return true;
    }
    let early_limit = 1.0 + pi_pycompat::pymath::min(15.0, d.pages().len() as f64 / 5.0);
    (page.index() as f64) < early_limit && w < 0.8 * threshold
}

/// Per-page heading detection across the document.
// ref: heading_detection/page_scan.py::build_doc_heading_candidates
pub fn build_doc_heading_candidates(d: Doc, labeled: &[Node]) -> Vec<Cand> {
    let mut dc = DocCandidateCollector {
        labeled,
        next_labeled: 0,
        seen_page: false,
        max_number: 0.0,
        letter_started: false,
        roman_started: false,
        accepted: Vec::new(),
    };
    let mut saw_body = false;
    for p in 0..d.pages().len() {
        if !saw_body && is_cover_like_page(d, d.page(p)) {
            continue;
        }
        saw_body = true;
        let mut ps = PageScanState::new(d, p);
        let cands = scan_page_headings(&mut ps);
        filter_page_candidates(d, &mut dc, p, cands);
    }
    dc.accepted
}

/// First valid heading on each page after the title page, then clique-filtered. Retypes the
/// accepted blocks to 7 (and marks them used) and nearby duplicates to 12.
// ref: heading_detection/page_scan.py::find_section_openers
pub fn find_section_openers(d: Doc, start_page_idx: usize) -> Vec<Node> {
    let mut items: Vec<Cand> = Vec::new();
    for p in start_page_idx..d.pages().len() {
        let ps = PageScanState::new(d, p);
        let pg = ps.pg();
        let mut current = None;
        if !pg.title_or_refs.get() {
            for &id in &pg.output {
                let b = ps.b(id);
                if b.char_count() == 0 || b.weighted_skew > 1.0 || b.kind.get() != 0 {
                    continue;
                }
                if b.is_body_paragraph.get() || b.top_edge() < 0.5 * pg.layout.bounds.bbox_height()
                {
                    break;
                }
                if b.caption_label.get() != 0 {
                    break;
                }
                if let Some(c) = try_classify_heading(&ps, id) {
                    current = Some(c);
                    break;
                }
                if b.line_count() > 2 {
                    break;
                }
            }
        }
        if let Some(c) = current {
            items.push(c);
        }
    }
    if items.len() <= 1 {
        return Vec::new();
    }
    let bundle = OutlineContext::new(d, &items);
    let mut accepted = OutlineContext::new(d, &[]);
    let mut out = Vec::new();
    for c in &items {
        let b = d.cblock(c);
        if accepted.has_nearby_duplicate(d, c) {
            b.kind.set(12);
            continue;
        }
        if has_conflict_in_context(d, &bundle, c) {
            continue;
        }
        accepted.add(d, c);
        out.push(new_node(c.clone()));
        b.kind.set(7);
        b.used_as_heading.set(true);
    }
    out
}
