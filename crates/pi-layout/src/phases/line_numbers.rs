//! Line-number column detection and stripping.
//!
//! ref: pageindex/flash/phases/line_numbers.py

use pi_core::Rect;
use pi_pycompat::pymath::{max, min};

use crate::clustering::LineId;
use crate::consts::*;
use crate::model::char_stats::info_weight;
use crate::model::numbering::to_number;
use crate::model::rects::Bounded;
use crate::model::span_line::{Line, Span, append_span, line_avg_char_width};

/// ref: phases/line_numbers.py::LineNumberCluster
#[derive(Debug, Clone)]
pub struct LineNumberCluster {
    pub lines: Vec<LineId>,
    pub left: f64,
    /// Average char width of the seed line. ref slot: `secondary_slot`
    pub char_width: f64,
    /// Last line number seen. ref slot: `primary_slot`
    pub last_number: f64,
    pub is_valid_sequence: bool,
}

// ref: phases/line_numbers.py::init_line_number_cluster
pub fn init_line_number_cluster(id: LineId, line: &Line, spans: &[Span]) -> LineNumberCluster {
    let n = to_number(&spans[line.spans[0]].trimmed_text);
    let valid = n > 0.0 && n < LINE_NUMBER_MAX && !n.is_nan() && n == n.floor();
    LineNumberCluster {
        lines: vec![id],
        left: line.left_edge(),
        char_width: line_avg_char_width(line),
        last_number: n,
        is_valid_sequence: valid,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Nearest {
    Left,
    Right,
}

// ref: phases/line_numbers.py::nearest_cluster
fn nearest_cluster(
    c: &LineNumberCluster,
    left: Option<&LineNumberCluster>,
    right: Option<&LineNumberCluster>,
) -> Option<Nearest> {
    let d_left = left.map_or(f64::INFINITY, |o| c.left - o.left);
    let d_right = right.map_or(f64::INFINITY, |o| o.left - c.left);
    let tol = 2.0 * c.char_width;
    if d_left > tol && d_right > tol {
        return None;
    }
    if d_left < d_right {
        left.map(|_| Nearest::Left)
    } else {
        right.map(|_| Nearest::Right)
    }
}

// ref: phases/line_numbers.py::validate_line_number_cluster
fn validate_line_number_cluster(
    rect: &Rect,
    lines: &[&Line],
    cluster: &LineNumberCluster,
    arena: &[Line],
    spans: &[Span],
) -> bool {
    if cluster.lines.len() < LINE_NUMBER_MIN_LINES {
        return false;
    }
    if cluster.left < LINE_NUMBER_EDGE_FRAC * rect.bbox_width() {
        return true;
    }
    let mut empty_line_count = 0usize;
    let mut flag = false;
    let mut top = f64::NEG_INFINITY;
    let mut bot = f64::INFINITY;
    let mut min_gap = f64::INFINITY;
    let mut max_gap = f64::NEG_INFINITY;
    let mut prev: Option<&Line> = None;
    for &id in &cluster.lines {
        let l = &arena[id];
        if l.char_count() as i64 - l.char_stats.category_counts[1] as i64 <= 0 {
            empty_line_count += 1;
        }
        if let Some(first) = l.first_letter_span
            && spans[first].char_stats.first_cat == 3
        {
            flag = true;
        }
        top = max(top, l.top_edge());
        bot = min(bot, l.bottom_edge());
        if let Some(p) = prev {
            let gap = p.bottom_edge() - l.bottom_edge();
            min_gap = min(min_gap, gap);
            max_gap = max(max_gap, gap);
        }
        prev = Some(l);
    }
    let gap_ratio = if min_gap != 0.0 {
        max_gap / min_gap
    } else if max_gap != 0.0 {
        f64::INFINITY.copysign(max_gap)
    } else {
        f64::NAN
    };
    if (empty_line_count as f64) < cluster.lines.len() as f64 / 2.0
        && (!flag || gap_ratio > LINE_NUMBER_GAP_RATIO)
    {
        return false;
    }
    let mut total = 0.0;
    let mut covered = 0.0;
    for l in lines {
        let w = info_weight(&l.char_stats);
        total += w;
        if l.bottom_edge() < top && l.top_edge() > bot {
            covered += w;
        }
    }
    covered >= LINE_NUMBER_COVERAGE * total
}

/// Detects a column of line numbers and strips it (dropping each affected line's first span).
/// Returns the input ids unchanged when no line-numbering pattern is found.
// ref: phases/line_numbers.py::strip_line_numbers
// `!(a < b)` is bisect_right's test and must stay NaN-faithful.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
pub fn strip_line_numbers(
    rect: &Rect,
    ids: Vec<LineId>,
    arena: &mut Vec<Line>,
    spans: &[Span],
) -> Vec<LineId> {
    // SortedKeyList keyed by `left` (insertion at bisect_right).
    let mut tree: Vec<LineNumberCluster> = Vec::new();
    for &id in &ids {
        let line = &arena[id];
        if line.spans.is_empty() || spans[line.spans[0]].trimmed_text.is_empty() {
            continue;
        }
        if line.left_edge() > LINE_NUMBER_MAX_LEFT_FRAC * rect.bbox_width() {
            continue;
        }
        let cand = init_line_number_cluster(id, line, spans);
        if !cand.is_valid_sequence {
            continue;
        }
        let idx_succ = tree.partition_point(|c| c.left < cand.left);
        let idx_pred = tree.partition_point(|c| !(cand.left < c.left));
        let succ = tree.get(idx_succ);
        let pred = if idx_pred > 0 {
            tree.get(idx_pred - 1)
        } else {
            None
        };
        match nearest_cluster(&cand, pred, succ) {
            Some(which) => {
                let m = match which {
                    Nearest::Left => &mut tree[idx_pred - 1],
                    Nearest::Right => &mut tree[idx_succ],
                };
                if m.is_valid_sequence {
                    m.is_valid_sequence = cand.last_number == m.last_number + 1.0;
                }
                m.lines.push(id);
                m.last_number = cand.last_number;
            }
            None => {
                let at = tree.partition_point(|c| !(cand.left < c.left));
                tree.insert(at, cand);
            }
        }
    }

    let mut best: Option<&LineNumberCluster> = None;
    for c in &tree {
        if c.is_valid_sequence && best.is_none_or(|b| c.lines.len() > b.lines.len()) {
            best = Some(c);
        }
    }
    let Some(best) = best else { return ids };
    let page_lines: Vec<&Line> = ids.iter().map(|&i| &arena[i]).collect();
    if !validate_line_number_cluster(rect, &page_lines, best, arena, spans) {
        return ids;
    }

    let affected: std::collections::HashSet<LineId> = best.lines.iter().copied().collect();
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if !affected.contains(&id) {
            out.push(id);
            continue;
        }
        let src = &arena[id];
        let first = src.spans[0];
        let mut new_line = Line::new();
        for &sid in &src.spans {
            if sid == first {
                continue;
            }
            append_span(&mut new_line, sid, spans);
        }
        if new_line.char_count() == 0 {
            continue;
        }
        new_line.column = src.column;
        arena.push(new_line);
        out.push(arena.len() - 1);
    }
    out
}
