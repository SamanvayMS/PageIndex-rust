//! Line-to-block joining rules and the section-heading trie.
//!
//! ref: pageindex/flash/blocks/join_rules.py

// The numbering-pattern cascade keeps the reference's separate branches (same action, distinct
// conditions) so each maps to a line of the original.
#![allow(clippy::if_same_then_else)]

use std::sync::LazyLock;

use pi_core::Rect;
use pi_pycompat::pymath;

use crate::model::block::{Block, case_signal, dominant_style_of, first_span_of, last_span_of};
use crate::model::char_stats::{is_upper_dominant, letter_count, max_nan};
use crate::model::numbering::numbering_ro;
use crate::model::rects::{
    Bounded, EMPTY_RECT, center_aligned, left_aligned, magnitude_ratio, right_aligned,
    x_centers_close,
};
use crate::model::span_line::{Line, line_avg_char_width, style_key};
use crate::phases::PageLayout;
use crate::stats::{DocStats, PageStats};
use crate::tokens::Trie;

pub(crate) static DICTS: LazyLock<serde_json::Value> =
    LazyLock::new(|| serde_json::from_str(pi_data::DICTIONARIES_JSON).expect("dictionaries.json"));

/// `_DICTS.get(key, [])` as strings.
pub(crate) fn dict_list(key: &str) -> Vec<String> {
    DICTS
        .get(key)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Case-folded trie over dictionary lists.
pub(crate) fn dict_trie(keys: &[&str]) -> Trie {
    let mut all = Vec::new();
    for k in keys {
        all.extend(dict_list(k));
    }
    Trie::build(all, true, false)
}

/// ref: blocks/join_rules.py:42 `SECTION_HEADING_TRIE`
pub static SECTION_HEADING_TRIE: LazyLock<Trie> =
    LazyLock::new(|| dict_trie(&["section_keywords", "abstract_keywords", "references"]));

/// ref: blocks/join_rules.py::BlockClusterContext
pub struct BlockClusterContext<'a> {
    /// ref slot: `tertiary_slot`
    pub doc_stats: &'a DocStats,
    /// ref slot: `auxiliary_slot`
    pub page_bbox: Rect,
    /// ref slot: `primary_slot`
    pub page_stats: &'a PageStats,
    /// Lines (and spans) to cluster. ref slot: `secondary_slot`
    pub page: &'a PageLayout,
    /// Column rectangles. ref slot: `state_slot`
    pub columns: &'a [Rect],
}

/// `candidate_line` joins `block`? `next_line` is the line after the candidate in page order
/// (the reference names it `previous_line`).
// ref: blocks/join_rules.py::should_join_line_to_block
pub fn should_join_line_to_block(
    ctx: &BlockClusterContext,
    block: &Block,
    cand: &Line,
    next_line: Option<&Line>,
    first_candidate: &Block,
    is_first_candidate: bool,
) -> bool {
    let page = ctx.page;
    let spans = &page.spans;
    // Step 1: skew.
    if (block.weighted_skew - cand.skew_frac).abs() > 1.0 {
        return false;
    }
    // Step 2: size + alignment gates.
    let fs_delta = cand.avg_font_size - block.weighted_font_size;
    let left_edges_aligned = left_aligned(block, cand, 1.0);
    let first_line = &page.lines[block.first_line()];
    let mut both_edges_aligned = left_edges_aligned
        || (block.line_count() == 1
            && left_aligned(block, cand, 8.0 * line_avg_char_width(first_line)));
    let right_edges_aligned = right_aligned(block, cand, 2.0);
    both_edges_aligned = both_edges_aligned && right_edges_aligned;
    let page_fs = ctx.page_stats.median_font_size;
    let doc_fs = ctx.doc_stats.body_font_size;
    let page_body_delta = pymath::min(
        cand.avg_font_size - page_fs,
        block.weighted_font_size - page_fs,
    );
    let doc_body_delta = pymath::min(
        cand.avg_font_size - doc_fs,
        block.weighted_font_size - doc_fs,
    );
    let bls = &spans[last_span_of(block, page)];
    let lfs = &spans[cand.spans[0]];
    if fs_delta.abs() > page_body_delta
        && fs_delta.abs() > doc_body_delta - 2.0
        && !(style_key(bls) == style_key(lfs) && bls.char_count() > 1 && lfs.char_count() > 1)
        && (fs_delta > 2.0
            || (fs_delta > 1.0 && !both_edges_aligned)
            || fs_delta < -5.0
            || (fs_delta < -2.0 && cand.char_count() >= 5)
            || (fs_delta < -1.0 && cand.char_count() >= 20 && !both_edges_aligned))
    {
        return false;
    }

    // Step 3: font / bold mismatch.
    let bll = &page.lines[block.last_line()];
    let width_ratio = magnitude_ratio(block.bbox_width(), cand.bbox_width());
    let bold_mismatch = bls.bold != lfs.bold;
    let font_mismatch =
        bls.font_name != lfs.font_name && dominant_style_of(block) != style_key(lfs);
    if font_mismatch || bold_mismatch {
        if bold_mismatch && width_ratio > 2.0 {
            return false;
        }
        if (bll.char_stats.first_cat == 1 || bll.char_stats.first_cat == 2)
            && (cand.char_stats.first_cat == 2 || width_ratio > 4.0)
        {
            return false;
        }
        if bll.char_stats.last_cat == 6 || block.bbox_width() > 1.5 * bll.bbox_width() {
            return false;
        }
    }
    if block.bold_frac > 0.9 && cand.bold_frac < 0.8 && width_ratio > 2.0 {
        return false;
    }

    // Step 4: spatial gates.
    let centers_aligned = center_aligned(block, cand, 1.0);
    if !centers_aligned {
        let vgap = block.bottom_edge() - cand.top_edge();
        let hoff = cand.left_edge() - block.left_edge();
        if (vgap > -1.0 && hoff > 0.33 * block.bbox_width()) || hoff > 0.98 * block.bbox_width() {
            return false;
        }
        if cand.center_x() < block.left_edge() {
            return false;
        }
    }

    // Step 5: tolerance base.
    let bottom_gap = block.bottom_edge() - cand.bottom_edge();
    let mut tol = (max_nan(
        1.3 * (block.top_edge() - block.bottom_edge()) / block.line_count() as f64,
        ctx.page_stats.median_overlap_gap,
    ) + 1.3 * block.weighted_font_size)
        / 2.0;

    // Step 6: case-flip "hanging indent".
    let bcs = case_signal(&block.char_stats);
    let lcs = case_signal(&cand.char_stats);
    let case_flip = ((bcs == 1 && lcs == -1) || (lcs == 1 && bcs == -1))
        && letter_count(&cand.char_stats) >= 3
        && (is_upper_dominant(&block.char_stats) != is_upper_dominant(&lfs.char_stats)
            || letter_count(&lfs.char_stats) < 3)
        && (is_upper_dominant(&bls.char_stats) != is_upper_dominant(&cand.char_stats)
            || letter_count(&bls.char_stats) < 3);
    let last_line_wide = if block.bbox_width() != 0.0 {
        bll.bbox_width() / block.bbox_width() > 0.9
    } else {
        bll.bbox_width() > 0.0
    };
    if !font_mismatch
        && !bold_mismatch
        && !case_flip
        && (width_ratio <= 1.2 || left_aligned(bll, cand, 0.1))
        && last_line_wide
    {
        tol *= 1.3;
    }
    if page_body_delta > 0.5 * page_fs && !case_flip {
        tol *= 2.0;
    }

    // Step 7: column alignment.
    let col = cand.column;
    let column_rect = if col >= 0 && (col as usize) < ctx.columns.len() {
        ctx.columns[col as usize]
    } else {
        EMPTY_RECT
    };
    let line_left_col = left_aligned(cand, &column_rect, 4.5);
    let line_right_col = right_aligned(cand, &column_rect, 4.5);
    let block_left_col = left_aligned(block, &column_rect, 4.5);
    let block_right_col = right_aligned(block, &column_rect, 4.5);
    let block_column_justified = block_left_col == block_right_col
        && block.center_aligned
        && x_centers_close(&ctx.page_bbox, block);
    let line_column_centered = line_left_col == line_right_col
        && (x_centers_close(&ctx.page_bbox, cand) || (block_column_justified && centers_aligned));

    // Step 8: alignment multipliers.
    if block_column_justified
        && line_column_centered
        && block.bbox_width() > 0.5 * cand.bbox_width()
        && next_line.is_none_or(|n| cand.bottom_edge() - n.bottom_edge() >= bottom_gap)
        && !font_mismatch
    {
        tol *= 1.3;
        if let Some(n) = next_line
            && ((block.bold_frac > n.bold_frac && cand.bold_frac > n.bold_frac)
                || (block.weighted_font_size > n.bbox_height() + 1.0
                    && cand.bbox_height() > n.bbox_height() + 1.0))
        {
            tol = pymath::max(tol, cand.bottom_edge() - n.top_edge());
        }
    } else if block_right_col && line_left_col {
        tol *= if block.line_count() <= 1 { 1.3 } else { 1.2 };
    } else if block_left_col && line_left_col {
        tol *= 1.1;
    } else if block_right_col {
        if block.line_count() <= 1 {
            tol *= 1.1;
        }
        if cand.char_stats.first_cat == 3 {
            tol *= 1.1;
        }
        if block.line_count() <= 1 && cand.char_stats.first_cat == 3 {
            tol *= 1.1;
        }
        if cand.left_edge() > block.left_edge()
            && cand.left_edge() <= block.left_edge() + 0.1 * block.bbox_width()
            && (block.line_count() <= 1 || left_aligned(cand, bll, 1.0))
        {
            tol *= 1.2;
        }
    } else if cand.bbox_width() < 0.9 * bll.bbox_width() && center_aligned(block, cand, 1.0) {
        tol *= 1.1;
    }

    if left_edges_aligned
        && cand.bbox_width() < 0.5 * block.bbox_width()
        && block.char_stats.last_cat != 6
        && cand.char_stats.last_cat == 6
    {
        tol *= 1.3;
    }

    // Step 9: numbering patterns.
    let (block_kind, _) = numbering_ro(first_line, spans);
    let block_has_numbering = block_kind != 0
        && spans[first_span_of(block, page)].bbox_height() >= 0.8 * block.weighted_font_size;
    let block_digit = block_has_numbering && block_kind == 1;
    let (line_kind, _) = numbering_ro(cand, spans);
    let line_has_numbering =
        line_kind != 0 && spans[cand.spans[0]].bbox_height() >= 0.8 * cand.avg_font_size;
    let line_digit = line_has_numbering && line_kind == 1;

    if block_digit && !line_digit && fs_delta <= -0.5 {
        tol /= 2.0;
    } else if (block_digit && (bold_mismatch || fs_delta <= -0.5))
        || (line_digit && (bold_mismatch || fs_delta >= 0.5))
    {
        tol /= 1.5;
    } else if block_digit
        && cand.left_edge() >= block.left_edge()
        && 0.9 * cand.bbox_width() > block.bbox_width()
    {
        tol /= 1.5;
    } else if block_has_numbering
        && cand.left_edge() >= block.left_edge()
        && 0.9 * cand.bbox_width() > block.bbox_width()
    {
        tol /= 1.3;
    } else if ((block_digit && cand.char_stats.first_cat != 3) || line_digit) && font_mismatch {
        tol /= 1.3;
    } else if block_digit && left_edges_aligned && cand.char_stats.first_cat == 2 {
        tol /= 1.3;
    } else if (block_has_numbering
        && (font_mismatch
            || bold_mismatch
            || fs_delta <= -0.5
            || (left_edges_aligned && cand.char_stats.first_cat == 2)))
        || (line_has_numbering && (font_mismatch || bold_mismatch || fs_delta >= 0.5))
    {
        tol /= 1.1;
    }
    if block_has_numbering && line_has_numbering {
        tol /= 1.3;
    }

    // Step 10: hanging indent + neighbour patches.
    if block_kind == 1
        && line_kind != 1
        && !left_edges_aligned
        && let Some(fl) = first_line.first_letter_span
        && left_aligned(&spans[fl], cand, 1.0)
    {
        tol *= 2.0;
    }
    if case_flip {
        tol /= 1.1;
        if block.line_count() == 1 || !left_edges_aligned {
            let divisor = if width_ratio > 3.0 {
                3.0
            } else if width_ratio > 1.5 {
                1.5
            } else {
                1.0
            };
            tol /= divisor;
        }
        if (is_upper_dominant(&block.char_stats) && block_has_numbering)
            || (is_upper_dominant(&cand.char_stats) && line_has_numbering)
        {
            tol /= 2.0;
        }
        if font_mismatch || bold_mismatch {
            tol /= 1.5;
        }
    }
    if !is_first_candidate
        && bottom_gap > 1.1 * (first_candidate.bottom_edge() - cand.bottom_edge())
    {
        tol /= 2.0;
    }
    bottom_gap <= tol
}
