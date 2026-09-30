//! Per-page processing driver (stages 02-03).
//!
//! ref: pageindex/flash/phases/page_view.py

use pi_core::Rect;

use crate::clustering::{LineId, build_initial_lines, cluster_lines};
use crate::columns::{ColumnDetectionContext, LineStore, columns_to_x_bounds, detect_columns};
use crate::consts::{CLUSTER_TOL_FIRST, CLUSTER_TOL_SECOND};
use crate::model::span_line::{Line, Span};
use crate::phases::line_numbers::strip_line_numbers;
use crate::stats::{PageStats, compute_page_stats};

/// Per-page layout state after line building, column detection and line-number stripping
/// (the fields of the reference `PageView` that stages 02-03 fill).
#[derive(Debug, Clone)]
pub struct PageLayout {
    /// 1-based page number. ref: `page_index`
    pub page: u32,
    /// ref: `bounds`
    pub bounds: Rect,
    /// ref slot: `primary_slot`
    pub stats: PageStats,
    /// Column rectangles. ref slot: `tertiary_slot`
    pub columns: Vec<Rect>,
    /// Final lines in reading order. ref: `lines`
    pub lines: Vec<Line>,
    /// The page's spans in producer order (the reference's `page.text`); lines index into it.
    /// Carries the bold flags set by overstrike detection.
    pub spans: Vec<Span>,
    /// Unrotated view box `(x0, y0, x1, y1)` and `/Rotate`, set by the caller when known
    /// (`extract_toc` does); heading y-fractions use them. ref: `viewport_box`, `rot`
    pub viewport_box: Option<[f64; 4]>,
    pub rot: u16,
}

/// `page_bbox` as `flash/main.py::extract_toc` derives it from the view box and `/Rotate`.
// ref: main.py::extract_toc (lines 139-144)
pub fn page_bbox_from_viewbox(viewbox: [f64; 4], rotation: u16) -> Rect {
    let [x0, y0, x1, y1] = viewbox;
    let (w, h) = ((x1 - x0).abs(), (y1 - y0).abs());
    let (pw, ph) = if rotation % 180 == 90 { (h, w) } else { (w, h) };
    Rect::new(0.0, pw, ph, 0.0)
}

/// Runs the per-page pipeline on flat span input: spans -> lines -> cluster (0.75) -> page
/// stats -> columns -> cluster (0.5, column bounds) -> strip line numbers -> page stats.
/// Pure: no shared state, so pages can be processed in parallel.
// ref: phases/page_view.py::process_page
pub fn process_page(spans: &[pi_core::Span], page_num: u32, page_bbox: Rect) -> PageLayout {
    let mut spans: Vec<Span> = spans.iter().map(Span::from_core).collect();
    // 1) Initial lines.
    let mut arena: Vec<Line> = build_initial_lines(&mut spans, &page_bbox);
    let ids: Vec<LineId> = (0..arena.len()).collect();
    // 2) First clustering pass, no column info.
    let ids = cluster_lines(&mut arena, ids, CLUSTER_TOL_FIRST, &[], &mut spans);
    // 3) First-pass page stats.
    let stats = compute_page_stats(&page_bbox, ids.iter().map(|&i| &arena[i]), &spans);
    // 4) Columns; sets each line's column index.
    let columns = {
        let ctx = ColumnDetectionContext::new(&page_bbox, &stats, ids.len());
        let mut store = LineStore {
            lines: &mut arena,
            spans: &spans,
        };
        detect_columns(&ctx, &ids, &mut store)
    };
    // 5) Second clustering pass with column bounds.
    let cols = columns_to_x_bounds(&columns);
    let ids = cluster_lines(&mut arena, ids, CLUSTER_TOL_SECOND, &cols, &mut spans);
    // 6) Line-number column.
    let ids = strip_line_numbers(&page_bbox, ids, &mut arena, &spans);
    // 7) Stats on the cleaned lines.
    let stats = compute_page_stats(&page_bbox, ids.iter().map(|&i| &arena[i]), &spans);
    let lines = ids.iter().map(|&i| std::mem::take(&mut arena[i])).collect();
    PageLayout {
        page: page_num,
        bounds: page_bbox,
        stats,
        columns,
        lines,
        spans,
        viewport_box: None,
        rot: 0,
    }
}
