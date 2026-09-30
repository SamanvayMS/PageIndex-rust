//! Recursive column splitting and column index assignment.
//!
//! ref: pageindex/flash/columns/splitting.py

use pi_core::Rect;
use pi_pycompat::pymath::{max, min};
use pi_pycompat::pysort;

use super::gutters::{
    ColumnDetectionContext, LineStore, SplitCandidate, SweepEvent, collect_gutter_candidates,
};
use crate::clustering::{ColBounds, LineId};
use crate::model::char_stats::info_weight;
use crate::model::rects::{Bounded, EMPTY_RECT, rect_union, tuple_lt};

// ref: columns/splitting.py::assign_column_index
fn assign_column_index(events: &[SweepEvent], column: i32, store: &mut LineStore) {
    for ev in events {
        if ev.is_start {
            store.lines[ev.line].column = column;
        }
    }
}

// ref: columns/splitting.py::recursive_split
#[allow(clippy::too_many_arguments)]
pub fn recursive_split(
    ctx: &ColumnDetectionContext,
    h_events: &[SweepEvent],
    v_events: &[SweepEvent],
    root: &Rect,
    cur: &Rect,
    depth: i64,
    column_offset: i32,
    store: &mut LineStore,
) -> Vec<Rect> {
    if depth >= ctx.max_depth {
        assign_column_index(h_events, column_offset, store);
        return vec![*cur];
    }

    let mut cands: Vec<SplitCandidate> = Vec::new();
    if cur.bbox_height() >= ctx.min_sweep_height {
        collect_gutter_candidates(
            ctx,
            root,
            cur,
            h_events,
            1,
            cur.bbox_height(),
            ctx.min_col_gap,
            &mut cands,
            store,
        );
    }
    if cur.bbox_width() >= ctx.min_sweep_width {
        collect_gutter_candidates(
            ctx,
            root,
            cur,
            v_events,
            0,
            cur.bbox_width(),
            ctx.min_row_gap,
            &mut cands,
            store,
        );
    }

    if cands.is_empty() {
        if cur.bbox_width() < 0.8 * root.bbox_width() {
            assign_column_index(h_events, column_offset, store);
            return vec![*cur];
        }
        // Fallback: look for vertical gutters in the horizontal events.
        let mut weight = 0.0; // ref: key_value
        let mut active: i64 = 0; // ref: active_overlap_count
        let mut max_weight = 0.0; // ref: measure_item
        let mut max_active: i64 = 0; // ref: local
        let mut gap_indices: Vec<usize> = Vec::new();
        let mut i = 0usize;
        while i + 1 < h_events.len() {
            let pos = h_events[i].position;
            let is_start = h_events[i].is_start;
            let line = &store.lines[h_events[i].line];
            if pos > root.left + root.bbox_width() * 5.0 / 6.0 {
                break;
            }
            if is_start {
                active += 1;
                weight += info_weight(&line.char_stats);
            } else {
                active -= 1;
                weight -= info_weight(&line.char_stats);
            }
            max_active = max_active.max(active);
            max_weight = max(max_weight, weight);
            if is_start || active > 2 || pos < root.left + root.bbox_width() / 6.0 {
                i += 1;
                continue;
            }
            let prev_gap = gap_indices.last().copied();
            if let Some(p) = prev_gap
                && pos < h_events[p].position + root.bbox_width() / 10.0
            {
                *gap_indices.last_mut().unwrap() = i;
            } else if max_active >= 8 && max_weight >= 100.0 {
                gap_indices.push(i);
            }
            max_active = 0;
            max_weight = 0.0;
            i += 1;
        }

        if gap_indices.is_empty() || gap_indices.len() > 2 {
            assign_column_index(h_events, column_offset, store);
            return vec![*cur];
        }
        if max_active < 4 || max_weight < 50.0 {
            assign_column_index(h_events, column_offset, store);
            return vec![*cur];
        }
        let mut out: Vec<Rect> = Vec::new();
        let mut cursor = 0usize;
        for index in 0..=gap_indices.len() {
            let pos = if index < gap_indices.len() {
                gap_indices[index]
            } else {
                h_events.len()
            };
            for ev in &h_events[cursor..pos] {
                if ev.is_start {
                    store.lines[ev.line].column = column_offset + out.len() as i32;
                }
            }
            let end = if pos < h_events.len() {
                h_events[pos].position
            } else {
                cur.right_edge()
            };
            out.push(Rect::new(
                h_events[cursor].position,
                end,
                cur.top,
                cur.bottom_edge(),
            ));
            cursor = pos + 1;
        }
        return out;
    }

    let mut best: Option<SplitCandidate> = None;
    for c in &cands {
        if best.is_none_or(|b| b.score < c.score) {
            best = Some(*c);
        }
    }
    let best = best.expect("non-empty candidates");

    if best.direction == 0 {
        // Row break: split events into the parts above and below the gap.
        let (mut upper_left, mut upper_right) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut lower_left, mut lower_right) = (f64::INFINITY, f64::NEG_INFINITY);
        let mut upper_h = Vec::new();
        let mut lower_h = Vec::new();
        for ev in h_events {
            let line = &store.lines[ev.line];
            if line.top_edge() > best.start {
                upper_h.push(*ev);
                upper_left = min(upper_left, line.left_edge());
                upper_right = max(upper_right, line.right_edge());
            } else if line.bottom_edge() < best.end {
                lower_h.push(*ev);
                lower_left = min(lower_left, line.left_edge());
                lower_right = max(lower_right, line.right_edge());
            }
        }
        let mut upper_v = Vec::new();
        let mut lower_v = Vec::new();
        for ev in v_events {
            if ev.position > best.start {
                upper_v.push(*ev);
            } else if ev.position < best.end {
                lower_v.push(*ev);
            }
        }
        let upper = recursive_split(
            ctx,
            &upper_h,
            &upper_v,
            root,
            &Rect::new(upper_left, upper_right, cur.top, best.end),
            depth + 1,
            column_offset,
            store,
        );
        let lower = recursive_split(
            ctx,
            &lower_h,
            &lower_v,
            root,
            &Rect::new(lower_left, lower_right, best.start, cur.bottom_edge()),
            depth + 1,
            column_offset + upper.len() as i32,
            store,
        );
        let mut out = upper;
        out.extend(lower);
        return out;
    }

    // Column break: split events into left and right parts.
    let (mut left_top, mut left_bottom) = (f64::NEG_INFINITY, f64::INFINITY);
    let (mut right_top, mut right_bottom) = (f64::NEG_INFINITY, f64::INFINITY);
    let mut left_h = Vec::new();
    let mut right_h = Vec::new();
    let mut left_v = Vec::new();
    let mut right_v = Vec::new();
    for ev in h_events {
        if ev.position < best.end {
            left_h.push(*ev);
        } else if ev.position > best.start {
            right_h.push(*ev);
        }
    }
    for ev in v_events {
        let line = &store.lines[ev.line];
        if line.left_edge() < best.end {
            left_v.push(*ev);
            left_top = max(left_top, line.top_edge());
            left_bottom = min(left_bottom, line.bottom_edge());
        } else if line.right_edge() > best.start {
            right_v.push(*ev);
            right_top = max(right_top, line.top_edge());
            right_bottom = min(right_bottom, line.bottom_edge());
        }
    }
    let left = recursive_split(
        ctx,
        &left_h,
        &left_v,
        root,
        &Rect::new(cur.left, best.start, left_top, left_bottom),
        depth + 1,
        column_offset,
        store,
    );
    let right = recursive_split(
        ctx,
        &right_h,
        &right_v,
        root,
        &Rect::new(best.end, cur.right_edge(), right_top, right_bottom),
        depth + 1,
        column_offset + left.len() as i32,
        store,
    );
    let mut out = left;
    out.extend(right);
    out
}

/// Detects column rectangles and sets each line's column index.
// ref: columns/splitting.py::detect_columns
pub fn detect_columns(
    ctx: &ColumnDetectionContext,
    ids: &[LineId],
    store: &mut LineStore,
) -> Vec<Rect> {
    let mut h_events = Vec::new();
    let mut v_events = Vec::new();
    let mut bbox = EMPTY_RECT;
    for &id in ids {
        let line = &store.lines[id];
        if line.bbox_width() <= 0.0 || line.bbox_height() <= 0.0 {
            continue;
        }
        bbox = rect_union(&bbox, &line.bbox);
        h_events.push(SweepEvent {
            line: id,
            position: line.left_edge(),
            is_start: true,
        });
        h_events.push(SweepEvent {
            line: id,
            position: line.right_edge(),
            is_start: false,
        });
        v_events.push(SweepEvent {
            line: id,
            position: line.bottom_edge(),
            is_start: true,
        });
        v_events.push(SweepEvent {
            line: id,
            position: line.top_edge(),
            is_start: false,
        });
    }
    let lines = &*store.lines;
    pysort::sort_by_key_lt(
        &mut h_events,
        |e| {
            [
                e.position,
                if e.is_start { 0.0 } else { 1.0 },
                lines[e.line].bbox_width(),
            ]
        },
        |a, b| tuple_lt(a, b),
    );
    pysort::sort_by_key_lt(
        &mut v_events,
        |e| {
            [
                e.position,
                if e.is_start { 0.0 } else { 1.0 },
                lines[e.line].bbox_height(),
            ]
        },
        |a, b| tuple_lt(a, b),
    );
    recursive_split(ctx, &h_events, &v_events, &bbox, &bbox, 0, 0, store)
}

// ref: columns/splitting.py::columns_to_x_bounds
pub fn columns_to_x_bounds(rects: &[Rect]) -> Vec<ColBounds> {
    rects
        .iter()
        .map(|r| ColBounds {
            left: r.left,
            right: r.right,
        })
        .collect()
}
