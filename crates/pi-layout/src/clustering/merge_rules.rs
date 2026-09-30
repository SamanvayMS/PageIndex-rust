//! Span continuation and line-merge predicates.
//!
//! ref: pageindex/flash/clustering/merge_rules.py

use std::sync::LazyLock;

use pi_pycompat::pymath;
use regex::Regex;

use crate::model::char_stats::{is_upper_dominant, letter_count};
use crate::model::numbering::numbering_kind;
use crate::model::rects::{Bounded, magnitude_ratio, same_y_extent};
use crate::model::span_line::{
    Line, Span, last_span, line_avg_char_width, raw_text_of_line, span_avg_char_width, text_of_line,
};

/// Package whitespace class body (ref: model/char_stats.py:118 `_UNICODE_WHITESPACE_CLASS`).
pub const WS_CLASS: &str = r"\t\n\x0B\x0C\r\x20\xA0\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}";

// ref: clustering/merge_rules.py:30 `TRAILING_DOT_LEADER_RE` (`\Z` -> `\z`)
pub static TRAILING_DOT_LEADER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"(?:[.][{WS_CLASS}]*){{4,}}\z")).unwrap());

/// Column x-bounds as produced by `columns_to_x_bounds` (a `{left, right}` dict).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColBounds {
    pub left: f64,
    pub right: f64,
}

// ref: clustering/merge_rules.py::span_continues_line
pub fn span_continues_line(line: &Line, span: &Span, spans: &[Span]) -> bool {
    if spans[last_span(line)].skew != span.skew {
        return false;
    }
    let line_cy = line.center_y();
    let span_cy = span.center_y();
    if (line_cy > span.top_edge() || line_cy < span.bottom_edge())
        && (span_cy > line.top_edge() || span_cy < line.bottom_edge())
    {
        return false;
    }
    let tolerance = pymath::min(
        5.0,
        pymath::max3(0.1, line_avg_char_width(line), span_avg_char_width(span)),
    );
    let mut wide = 2.0 * tolerance;
    if line.char_stats.last_cat == 5 {
        wide *= 2.0;
    }
    span.left_edge() > line.right_edge() - wide && span.left_edge() < line.right_edge() + tolerance
}

// ref: clustering/merge_rules.py::vertical_distance_in_line_heights
pub fn vertical_distance_in_line_heights(a: &Line, b: &Line) -> f64 {
    let acy = a.center_y();
    let bcy = b.center_y();
    if acy == bcy {
        return 0.0;
    }
    let denom = pymath::max(a.bbox_height(), b.bbox_height());
    if denom == 0.0 {
        let diff = acy - bcy;
        return if diff.is_nan() {
            f64::NAN
        } else {
            f64::INFINITY
        };
    }
    (acy - bcy).abs() / denom
}

/// Which neighbour [`pick_closer_neighbor`] chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Neighbor {
    Pred,
    Succ,
}

// ref: clustering/merge_rules.py::pick_closer_neighbor
pub fn pick_closer_neighbor(
    pred: Option<&Line>,
    succ: Option<&Line>,
    cand: &Line,
    tol: f64,
) -> Option<Neighbor> {
    if pred.is_none() && succ.is_none() {
        return None;
    }
    let d_pred = pred.map_or(f64::INFINITY, |l| {
        vertical_distance_in_line_heights(l, cand)
    });
    let d_succ = succ.map_or(f64::INFINITY, |l| {
        vertical_distance_in_line_heights(l, cand)
    });
    if d_pred >= tol && d_succ >= tol {
        return None;
    }
    if d_pred < d_succ {
        pred.map(|_| Neighbor::Pred)
    } else {
        succ.map(|_| Neighbor::Succ)
    }
}

// ref: clustering/merge_rules.py::should_merge_lines
pub fn should_merge_lines(
    line: &mut Line,
    other: &Line,
    cols: &[ColBounds],
    spans: &[Span],
) -> bool {
    if line.char_count() > 0
        && other.char_count() > 0
        && magnitude_ratio(line.max_span_height, other.max_span_height) > 2.0
        && (line.max_span_height - other.max_span_height).abs() > 10.0
    {
        return false;
    }

    let acw_line = line_avg_char_width(line);
    let acw_other = line_avg_char_width(other);
    let line_proj = if acw_line != 0.0 {
        1.0 / acw_line
    } else {
        f64::INFINITY
    };
    let other_proj = if acw_other != 0.0 {
        1.0 / acw_other
    } else {
        f64::INFINITY
    };
    let harmonic_char_width = 2.0 / (line_proj + other_proj);
    let horizontal_gap = other.left_edge() - line.right_edge();
    let mut gap_factor = 2.0;

    // "italic" mismatch reads the bold fraction, as the reference does.
    if (line.bold_frac > 0.0) != (other.bold_frac > 0.0) {
        gap_factor /= 1.5;
    }

    if line.char_stats.last_cat == 4
        || line.char_stats.category_counts[4] as f64 > line.char_count() as f64 / 2.0
    {
        gap_factor /= 2.0;
    }

    if line.char_stats.last_cat == 6 {
        let all_digits = other.char_stats.total_chars > 0
            && other.char_stats.total_chars == other.char_stats.category_counts[1];
        if all_digits && TRAILING_DOT_LEADER_RE.is_match(&raw_text_of_line(line, spans)) {
            gap_factor *= 3.0;
        }
    }

    let (col_left, col_right) =
        if !cols.is_empty() && 0 <= line.column && (line.column as usize) < cols.len() {
            let c = cols[line.column as usize];
            (c.left, c.right)
        } else {
            (f64::INFINITY, f64::NEG_INFINITY)
        };

    let inside_col = line.left_edge() >= col_left
        && line.right_edge() <= col_right
        && other.left_edge() >= col_left
        && other.right_edge() <= col_right;
    if (line.char_count() < 40 || inside_col)
        && (same_y_extent(line, other, 0.1) || same_y_extent(&spans[last_span(line)], other, 0.1))
    {
        gap_factor *= 1.5;
    }
    if line.char_count() < 40 && inside_col {
        gap_factor *= 2.0;
    }

    let other_column = if 0 <= other.column && (other.column as usize) < cols.len() {
        Some(cols[other.column as usize])
    } else {
        None
    };
    if cols.is_empty()
        || ((line.right_edge() - col_right).abs() < 5.0
            && (line.column as i64 >= cols.len() as i64 - 1
                || other_column.is_none()
                || (other.left_edge() - other_column.map_or(f64::INFINITY, |c| c.left)).abs()
                    < 5.0))
    {
        gap_factor /= 2.0;
    }

    if line.char_count() <= 8
        && line.bbox_width() <= 10.0 * line.avg_font_size
        && numbering_kind(line, spans) != 0
        && letter_count(&other.char_stats) > 0
    {
        gap_factor *= if inside_col { 3.0 } else { 2.0 };
    }

    if line.char_count() <= 10 {
        let text = text_of_line(line, spans);
        // Bracketed short line, or an uppercase-initial sentence inside a column.
        if (text.starts_with('[') && text.ends_with(']'))
            || (inside_col && line.char_stats.first_cat == 2 && text.ends_with('.'))
        {
            gap_factor *= 2.0;
        }
    }

    if inside_col && is_upper_dominant(&line.char_stats) && is_upper_dominant(&other.char_stats) {
        gap_factor *= 1.5;
    }

    horizontal_gap <= gap_factor * harmonic_char_width
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_leader() {
        assert!(TRAILING_DOT_LEADER_RE.is_match("Intro . . . ."));
        assert!(TRAILING_DOT_LEADER_RE.is_match("Intro....\u{3000}"));
        assert!(!TRAILING_DOT_LEADER_RE.is_match("Intro..."));
        assert!(!TRAILING_DOT_LEADER_RE.is_match("Intro....x"));
        assert!(!TRAILING_DOT_LEADER_RE.is_match("Intro...\n"));
    }
}
