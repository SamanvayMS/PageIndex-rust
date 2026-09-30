//! Gutter-gap candidates and scoring for column detection.
//!
//! ref: pageindex/flash/columns/gutters.py

use std::sync::LazyLock;

use pi_core::Rect;
use pi_pycompat::pymath::{max, min};
use regex::Regex;

use crate::clustering::LineId;
use crate::clustering::merge_rules::WS_CLASS;
use crate::model::char_stats::{info_weight, max_nan, min_nan};
use crate::model::numbering::{numbering_kind, numbering_value};
use crate::model::rects::Bounded;
use crate::model::span_line::{Line, Span, text_of_line};
use crate::stats::PageStats;

// ref: columns/gutters.py:70 `DOT_LEADER_RE` (`\Z` -> `\z`)
static DOT_LEADER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"(?:[.][{WS_CLASS}]*){{5,}}\z")).unwrap());

/// A line entering (`is_start`) or leaving the sweep at `position`.
/// ref: columns/gutters.py::SweepEvent
#[derive(Debug, Clone, Copy)]
pub struct SweepEvent {
    pub line: LineId,
    pub position: f64,
    pub is_start: bool,
}

/// Direction 0 = vertical sweep (row breaks), 1 = horizontal sweep (column breaks).
/// ref: columns/gutters.py::SplitCandidate
#[derive(Debug, Clone, Copy)]
pub struct SplitCandidate {
    pub start: f64,
    pub end: f64,
    pub direction: u8,
    pub score: f64,
}

/// ref: columns/gutters.py::ColumnDetectionContext
#[derive(Debug, Clone)]
pub struct ColumnDetectionContext<'a> {
    /// ref slot: `secondary_slot`
    pub stats: &'a PageStats,
    /// Recursion depth limit, floor(2 log2(#lines)). ref slot: `tertiary_slot`
    pub max_depth: i64,
    /// Minimum sub-rect width for a vertical sweep: page width / 6. ref slot: `state_slot`
    pub min_sweep_width: f64,
    /// Minimum gap for a row break. ref slot: `auxiliary_slot`
    pub min_row_gap: f64,
    /// Minimum gap for a column break: the average char width. ref slot: `option_slot`
    pub min_col_gap: f64,
    /// Minimum sub-rect height for a horizontal sweep: 1.5 x median font size.
    /// ref slot: `measure_slot`
    pub min_sweep_height: f64,
}

impl<'a> ColumnDetectionContext<'a> {
    // ref: columns/gutters.py::ColumnDetectionContext.__init__
    pub fn new(page_bbox: &Rect, stats: &'a PageStats, line_count: usize) -> Self {
        let fs = stats.median_font_size;
        ColumnDetectionContext {
            stats,
            max_depth: if line_count > 0 {
                (2.0 * (line_count as f64).log2()).floor() as i64
            } else {
                0
            },
            min_sweep_width: page_bbox.bbox_width() / 6.0,
            min_row_gap: max_nan(
                0.5 * fs,
                min_nan(1.1 * (stats.median_overlap_gap - fs), 3.0 * fs),
            ),
            min_col_gap: stats.avg_char_width,
            min_sweep_height: 1.5 * fs,
        }
    }
}

/// Mutable page state the scorer reads (numbering caches are filled lazily, as in the
/// reference).
pub struct LineStore<'a> {
    pub lines: &'a mut [Line],
    pub spans: &'a [Span],
}

// ref: columns/gutters.py::collect_gutter_candidates
#[allow(clippy::too_many_arguments)]
pub fn collect_gutter_candidates(
    ctx: &ColumnDetectionContext,
    root: &Rect,
    cur: &Rect,
    events: &[SweepEvent],
    direction: u8,
    extent: f64,
    min_gap: f64,
    out: &mut Vec<SplitCandidate>,
    store: &mut LineStore,
) {
    let mut active: i64 = 0;
    for i in 0..events.len().saturating_sub(1) {
        if events[i].is_start {
            active += 1;
        } else {
            active -= 1;
        }
        if active > 0 {
            continue;
        }
        if let Some(score) =
            score_gutter_gap(ctx, root, cur, events, direction, extent, min_gap, i, store)
        {
            out.push(SplitCandidate {
                start: events[i].position,
                end: events[i + 1].position,
                direction,
                score,
            });
        }
    }
}

/// Scores the gap between `events[at]` and `events[at + 1]`, or `None` when not viable.
// ref: columns/gutters.py::_score_gutter_gap
#[allow(clippy::too_many_arguments)]
fn score_gutter_gap(
    ctx: &ColumnDetectionContext,
    root: &Rect,
    cur: &Rect,
    events: &[SweepEvent],
    direction: u8,
    extent: f64,
    min_gap: f64,
    at: usize,
    store: &mut LineStore,
) -> Option<f64> {
    let spans = store.spans;
    let gap_start = events[at].position; // ref: key_value
    let gap_end = events[at + 1].position; // ref: score_value
    let gap = gap_end - gap_start; // ref: item_value
    if gap < min_gap {
        return None;
    }

    // --- backward pass: lines that close before this gap ---
    let mut n_before: i64 = 0; // ref: measure_item
    let mut before_max_width = 0.0; // ref: preceding_max_width
    let mut weight_before = 0.0; // ref: secondary_item
    let mut max_font_before = 0.0; // ref: reference_item
    let mut weight_at_max_font_before = 0.0; // ref: distance_accumulator
    let mut n_before_at_edge: i64 = 0; // ref: width_value
    let mut before_edge_max_width = 0.0; // ref: wide_accumulator
    let mut min_edge = f64::INFINITY;
    let mut before_max_trailing = f64::NEG_INFINITY; // ref: preceding_max_trailing_edge
    let mut edge_min_leading = f64::INFINITY; // ref: min_edge_position
    let mut edge_max_trailing = f64::NEG_INFINITY; // ref: max_edge
    let mut numbered_before: i64 = 0; // ref: event_count
    let mut min_left_of_one = f64::INFINITY; // ref: lower_accumulator
    let mut short_numbered_before: i64 = 0; // ref: candidate_item
    let mut bracket_or_caps_before: i64 = 0; // ref: group_value
    let mut max_char_count: u32 = 0;
    let mut dot_leaders_before: i64 = 0; // ref: state_item

    let mut k = at as i64;
    while k >= 0 {
        let ev = events[k as usize];
        if ev.position < gap_start - gap {
            break;
        }
        if ev.is_start {
            k -= 1;
            continue;
        }
        let line = &mut store.lines[ev.line];
        n_before += 1;
        before_max_width = max(before_max_width, line.bbox_width());
        let (leading, trailing) = if direction == 1 {
            (line.bottom_edge(), line.top_edge())
        } else {
            (line.left_edge(), line.right_edge())
        };
        min_edge = min(min_edge, leading);
        before_max_trailing = max(before_max_trailing, trailing);
        let w = info_weight(&line.char_stats);
        weight_before += w;
        if line.avg_font_size > max_font_before {
            max_font_before = line.avg_font_size;
            weight_at_max_font_before = w;
        } else if line.avg_font_size == max_font_before {
            weight_at_max_font_before += w;
        }
        if gap_start - ev.position < 1.0 {
            n_before_at_edge += 1;
            before_edge_max_width = max(before_edge_max_width, line.bbox_width());
            edge_min_leading = min(edge_min_leading, leading);
            edge_max_trailing = max(edge_max_trailing, trailing);
        }
        if numbering_kind(line, spans) == 1 {
            numbered_before += 1;
            if numbering_value(line, spans) == 1.0 {
                min_left_of_one = min(min_left_of_one, line.left_edge());
            }
        }
        if direction == 1 {
            if line.char_count() <= 5 && numbering_kind(line, spans) != 0 {
                short_numbered_before += 1;
            }
            if line.char_count() <= 10 {
                let text = text_of_line(line, spans);
                if (text.starts_with('[') && text.ends_with(']'))
                    || (line.char_stats.first_cat == 2 && text.ends_with('.'))
                {
                    bracket_or_caps_before += 1;
                }
            }
            if DOT_LEADER_RE.is_match(&text_of_line(line, spans)) {
                dot_leaders_before += 1;
            }
        }
        max_char_count = max_char_count.max(line.char_count());
        k -= 1;
    }

    let nb = n_before as f64;
    if short_numbered_before >= n_before
        || short_numbered_before as f64 >= max(2.0, nb / 2.0)
        || bracket_or_caps_before >= n_before
        || dot_leaders_before as f64 >= max(2.0, nb / 2.0)
        || (direction == 1 && max_char_count <= 1)
    {
        return None;
    }

    // --- forward pass: lines that open after this gap ---
    let mut after_max_width = 0.0; // ref: next_gap
    let mut n_after: i64 = 0; // ref: following_line_count
    let mut after_min_leading = f64::INFINITY; // ref: other_gap
    let mut after_max_trailing = f64::NEG_INFINITY; // ref: following_max_trailing_edge
    let mut weight_at_max_font_after = 0.0; // ref: page_gap
    let mut max_font_after = 0.0; // ref: following_max_font_size
    let mut weight_after = 0.0; // ref: after
    let mut n_after_at_edge: i64 = 0; // ref: quantity
    let mut max_number_after = 0.0; // ref: numbering_score
    let mut numbered_after: i64 = 0; // ref: following_numbering_count
    let mut after_edge_max_width = 0.0; // ref: following_edge_max_width
    let mut max_top_numbered_after = 0.0; // ref: right_gap
    let mut k = at + 1;
    while k < events.len() {
        let ev = events[k];
        if ev.position > gap_end + gap {
            break;
        }
        if !ev.is_start {
            k += 1;
            continue;
        }
        let line = &mut store.lines[ev.line];
        n_after += 1;
        after_max_width = max(after_max_width, line.bbox_width());
        let (leading, trailing) = if direction == 1 {
            (line.bottom_edge(), line.top_edge())
        } else {
            (line.left_edge(), line.right_edge())
        };
        after_min_leading = min(after_min_leading, leading);
        after_max_trailing = max(after_max_trailing, trailing);
        let w = info_weight(&line.char_stats);
        weight_after += w;
        if line.avg_font_size > max_font_after {
            max_font_after = line.avg_font_size;
            weight_at_max_font_after = w;
        } else if line.avg_font_size == max_font_after {
            weight_at_max_font_after += w;
        }
        if ev.position - gap_end < 1.0 {
            n_after_at_edge += 1;
            after_edge_max_width = max(after_edge_max_width, line.bbox_width());
        }
        if numbering_kind(line, spans) == 1 {
            numbered_after += 1;
            max_number_after = max_nan(max_number_after, numbering_value(line, spans));
            max_top_numbered_after = max_nan(max_top_numbered_after, line.top_edge());
        }
        k += 1;
    }

    if n_before <= 0 || n_after <= 0 {
        return None;
    }

    let root_width = root.bbox_width();
    let height = root.bbox_height();
    let root_center_x = root.center_x();

    if direction == 1 {
        if n_before_at_edge <= 1
            && n_after_at_edge <= 1
            && !(cur.top_edge() < root.bottom_edge() + 0.3 * height
                && min_left_of_one < f64::INFINITY
                && max_number_after <= 4.0)
        {
            return None;
        }
        if (weight_before <= 100.0 && weight_after <= 100.0)
            && (gap_start < root.left + 0.2 * root_width || gap_end > root.left + 0.8 * root_width)
        {
            return None;
        }
    }

    let mid = (max_font_before + max_font_after) / 2.0;
    let body_fs = ctx.stats.median_font_size;

    if direction == 0
        && weight_at_max_font_before >= 0.8 * weight_before
        && weight_at_max_font_after >= 0.8 * weight_after
        && (((max_font_before - max_font_after).abs() < 0.1
            && max_font_before >= body_fs + 0.5
            && max_font_after >= body_fs + 0.5
            && gap < max(1.3 * mid, min_gap * 2.0))
            || (max_font_before >= body_fs + 2.0
                && max_font_after >= body_fs + 2.0
                && gap < max(1.5 * mid, min_gap * 3.0)))
    {
        return None;
    }

    let mut score = extent * extent * gap; // ref: line_value

    if direction == 1 {
        score *= n_before_at_edge.min(n_after_at_edge) as f64;
        if weight_before <= 50.0 && n_before <= 1 {
            score /= 100.0;
        }
        let cur_height = cur.bbox_height();
        let threshold = cur.top_edge() - 0.2 * cur_height;
        if numbered_after >= 3 && max_number_after >= 6.0 && max_top_numbered_after < threshold {
            let denom = max_number_after - numbered_after as f64;
            let inv = if denom != 0.0 {
                1.0 / denom
            } else {
                f64::INFINITY
            };
            let mut factor = max(0.3, min(1.0, inv));
            factor *= factor;
            score *= factor;
        } else if cur_height > height / 2.0 {
            let mut factor = cur_height / height;
            factor *= factor;
            score *= 1.0 + factor;
        }
        score *= max(
            1.0,
            2.0 - (root_center_x - (gap_start + gap_end) / 2.0).abs() / root_width * 10.0,
        );
    }

    if direction == 0 {
        let cur_width = cur.bbox_width();
        score *= max(before_edge_max_width, after_edge_max_width) / cur_width
            * (max(before_max_width, after_max_width) / cur_width);
        let min_value = min(min_edge, after_min_leading);
        let max_value = max(before_max_trailing, after_max_trailing);
        if min_value < root_center_x && max_value > root_center_x {
            let left_dist = root_center_x - min_value;
            let right_dist = max_value - root_center_x;
            score *= 1.0 + min(left_dist, right_dist) / max(left_dist, right_dist);
        }
        let edge_top = root.bottom + 0.2 * height;
        let edge_bot = root.bottom + 0.8 * height;
        if gap_start < edge_top || gap_end > edge_bot {
            score *= 4.0;
            if (gap_start < edge_top
                && numbered_before >= 1
                && before_edge_max_width < root_width / 4.0)
                || (gap_end > edge_bot
                    && numbered_after >= 1
                    && after_edge_max_width < root_width / 4.0)
            {
                score *= 9.0;
            }
        }
        if 2 * n_before_at_edge >= at as i64 && max_font_before > max_font_after + 0.5 {
            score *= 100.0;
        }
        if min_left_of_one < f64::INFINITY {
            if min_left_of_one < root_center_x {
                score *= 100.0;
            }
        } else if numbered_before >= 2 || numbered_after >= 2 {
            score /= 4.0;
        }
        let size = max(max_font_before, max_font_after);
        if extent >= 0.99 * root_width
            && edge_min_leading < root_center_x
            && edge_max_trailing > root_center_x
            && max_font_before >= max_font_after + 0.5
            && gap > size
        {
            score *= gap / size;
        }
    }

    if weight_before < 1.0 || weight_after < 1.0 {
        score *= 10.0;
    }

    Some(max_nan(0.0, score))
}
