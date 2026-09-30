//! Builds initial lines and clusters them into merged lines.
//!
//! ref: pageindex/flash/clustering/build.py

use pi_core::Rect;
use pi_pycompat::{pymath, pysort};

use super::merge_rules::{
    ColBounds, Neighbor, pick_closer_neighbor, should_merge_lines, span_continues_line,
};
use crate::consts::*;
use crate::model::rects::{
    Bounded, left_edge_key, reading_order_key, same_x_extent, same_y_extent, tuple_eq, tuple_lt,
};
use crate::model::span_line::{Line, Span, SpanId, append_span, last_span};

/// Index of a line in a page's line arena.
pub type LineId = usize;

// ref: clustering/build.py::_skip_mark_only
fn skip_mark_only(span: &Span, page_area: f64) -> bool {
    (span.char_count() as i64 - span.char_stats.category_counts[5] as i64) > 1
        && span.area() < page_area * TINY_MARK_AREA_FRAC
}

/// Groups the page's spans (in producer order) into initial lines. Marks overstruck spans bold
/// in the arena (`spans`), as the reference mutates its span objects.
// ref: clustering/build.py::build_initial_lines
pub fn build_initial_lines(spans: &mut [Span], page_bbox: &Rect) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut pending_line = Line::new();
    let mut pending_span: Option<SpanId> = None;
    let page_area = page_bbox.area();

    for sid in 0..spans.len() {
        let span = &spans[sid];
        if span.text.is_empty() || skip_mark_only(span, page_area) {
            continue;
        }
        let Some(pid) = pending_span else {
            pending_span = Some(sid);
            continue;
        };
        let p = &spans[pid];
        if p.char_count() > 0
            && p.trimmed_text == span.trimmed_text
            && same_x_extent(p, span, OVERSTRIKE_TOL_FRAC * p.bbox_width())
            && same_y_extent(p, span, OVERSTRIKE_TOL_FRAC * p.bbox_height())
        {
            spans[pid].bold = true;
            continue;
        }
        if !(pending_line.spans.is_empty()
            || span_continues_line(&pending_line, &spans[pid], spans))
        {
            lines.push(std::mem::take(&mut pending_line));
        }
        append_span(&mut pending_line, pid, spans);
        pending_span = Some(sid);
    }

    if let Some(pid) = pending_span {
        if !(pending_line.spans.is_empty()
            || span_continues_line(&pending_line, &spans[pid], spans))
        {
            lines.push(std::mem::take(&mut pending_line));
        }
        append_span(&mut pending_line, pid, spans);
        lines.push(pending_line);
    }
    lines
}

// ref: clustering/build.py::_is_label_stack
fn is_label_stack(line: &Line, other: &Line, body_ma: f64) -> bool {
    if body_ma <= 0.0 {
        return false;
    }
    let overlap = pymath::min(line.right_edge(), other.right_edge())
        - pymath::max(line.left_edge(), other.left_edge());
    let frac = overlap / pymath::max(1e-6, pymath::min(line.bbox_width(), other.bbox_width()));
    let v_overlap = pymath::min(line.top_edge(), other.top_edge())
        - pymath::max(line.bottom_edge(), other.bottom_edge());
    let v_frac =
        v_overlap / pymath::max(1e-6, pymath::min(line.bbox_height(), other.bbox_height()));
    if !(frac > 0.5 && v_frac < 0.5) {
        return false;
    }
    let (upper, lower) = if line.center_y() > other.center_y() {
        (line, other)
    } else {
        (other, line)
    };
    upper.avg_font_size >= LABEL_STACK_BODY_RATIO * body_ma
        && upper.avg_font_size >= LABEL_STACK_LOWER_RATIO * lower.avg_font_size
}

/// The reference's `SortedKeyList(key=reading_order_key)` restricted to the operations
/// `cluster_lines` uses. Keys are unique (see `set_add`), so a sorted `Vec` with the same
/// bisect semantics is equivalent.
struct LineTree {
    ids: Vec<LineId>,
}

impl LineTree {
    fn key(arena: &[Line], id: LineId) -> [f64; 4] {
        reading_order_key(&arena[id])
    }

    fn bisect_left(&self, arena: &[Line], k: &[f64; 4]) -> usize {
        self.ids
            .partition_point(|&x| tuple_lt(&Self::key(arena, x), k))
    }

    fn bisect_right(&self, arena: &[Line], k: &[f64; 4]) -> usize {
        self.ids
            .partition_point(|&x| !tuple_lt(k, &Self::key(arena, x)))
    }

    /// ref: clustering/build.py::_set_add — a line whose key equals an existing one is dropped.
    fn set_add(&mut self, arena: &[Line], id: LineId) {
        let k = Self::key(arena, id);
        let idx = self.bisect_left(arena, &k);
        if idx < self.ids.len() && tuple_eq(&Self::key(arena, self.ids[idx]), &k) {
            return;
        }
        self.ids.insert(idx, id);
    }
}

fn pair_mut(arena: &mut [Line], a: LineId, b: LineId) -> (&mut Line, &mut Line) {
    assert_ne!(a, b);
    if a < b {
        let (x, y) = arena.split_at_mut(b);
        (&mut x[a], &mut y[0])
    } else {
        let (x, y) = arena.split_at_mut(a);
        (&mut y[0], &mut x[b])
    }
}

/// Stable sort of line ids by `key`, with Python tuple ordering.
pub(crate) fn sort_ids_by(arena: &[Line], ids: &mut Vec<LineId>, key: fn(&Line) -> [f64; 4]) {
    pysort::sort_by_key_lt(ids, |&id| key(&arena[id]), |a, b| tuple_lt(a, b));
}

/// Merges nearby compatible lines. `ids` are the current lines (in the arena); new lines are
/// pushed to the arena. Returns the merged line ids in reading order.
// ref: clustering/build.py::cluster_lines
pub fn cluster_lines(
    arena: &mut Vec<Line>,
    mut ids: Vec<LineId>,
    tol: f64,
    cols: &[ColBounds],
    spans: &mut [Span],
) -> Vec<LineId> {
    sort_ids_by(arena, &mut ids, left_edge_key);

    let mut sizes: Vec<f64> = ids
        .iter()
        .flat_map(|&id| arena[id].spans.iter().map(|&s| spans[s].font_size))
        .filter(|&fs| fs > 0.0)
        .collect();
    pysort::sort_by_lt(&mut sizes, |a, b| a < b);
    let body_ma = if sizes.is_empty() {
        0.0
    } else {
        sizes[sizes.len() / 2]
    };

    let mut tree = LineTree { ids: Vec::new() };
    let mut merged: Vec<LineId> = Vec::new();

    for cand in ids {
        if spans[last_span(&arena[cand])].skew > ROTATED_SKEW {
            merged.push(cand);
            continue;
        }
        let k = LineTree::key(arena, cand);
        let idx_succ = tree.bisect_left(arena, &k);
        let succ = tree.ids.get(idx_succ).copied();
        let idx_pred = tree.bisect_right(arena, &k);
        let pred = if idx_pred > 0 {
            Some(tree.ids[idx_pred - 1])
        } else {
            None
        };

        let choice = pick_closer_neighbor(
            pred.map(|p| &arena[p]),
            succ.map(|s| &arena[s]),
            &arena[cand],
            tol,
        );
        let (neighbor, pos) = match choice {
            None => {
                tree.set_add(arena, cand);
                continue;
            }
            Some(Neighbor::Pred) => (pred.unwrap(), idx_pred - 1),
            Some(Neighbor::Succ) => (succ.unwrap(), idx_succ),
        };
        tree.ids.remove(pos);
        let nls = last_span(&arena[neighbor]);

        let (nl, cl) = (&arena[neighbor], &arena[cand]);
        let s_nls = &spans[nls];
        // Subscript / overstrike: a single-span candidate duplicating the neighbour's last span.
        let overstrike = cl.spans.len() == 1
            && nl.spans.len() <= OVERSTRIKE_MAX_SPANS
            && s_nls.char_count() > 0
            && s_nls.trimmed_text == spans[cl.spans[0]].trimmed_text
            && same_x_extent(s_nls, cl, OVERSTRIKE_TOL_FRAC * s_nls.bbox_width())
            && same_y_extent(s_nls, cl, OVERSTRIKE_TOL_FRAC * s_nls.bbox_height());
        if overstrike {
            if (s_nls.left_edge() - cl.left_edge()).abs() < EXACT_DUP_TOL
                && (s_nls.top_edge() - cl.top_edge()).abs() < EXACT_DUP_TOL
            {
                tree.set_add(arena, neighbor);
                continue;
            }
            let mut new_line = Line::new();
            spans[nls].bold = true;
            for sid in arena[neighbor].spans.clone() {
                append_span(&mut new_line, sid, spans);
            }
            arena.push(new_line);
            tree.set_add(arena, arena.len() - 1);
            continue;
        }
        let merge = {
            let (nl, cl) = pair_mut(arena, neighbor, cand);
            should_merge_lines(nl, cl, cols, spans)
        };
        if merge {
            let (nl, cl) = (&arena[neighbor], &arena[cand]);
            if is_label_stack(nl, cl, body_ma) && cl.center_y() > nl.center_y() {
                let mut m = Line::new();
                for sid in cl.spans.iter().chain(nl.spans.iter()) {
                    append_span(&mut m, *sid, spans);
                }
                arena.push(m);
                tree.set_add(arena, arena.len() - 1);
            } else {
                let add = cl.spans.clone();
                for sid in add {
                    append_span(&mut arena[neighbor], sid, spans);
                }
                tree.set_add(arena, neighbor);
            }
        } else {
            merged.push(neighbor);
            tree.set_add(arena, cand);
        }
    }

    merged.extend(tree.ids);
    sort_ids_by(arena, &mut merged, reading_order_key);
    merged
}
