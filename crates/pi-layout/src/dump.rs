//! Parity serialization of stages 02-03, mirroring `parity/dump_reference.py` (`line_d`,
//! `page_stats_d`, the `02_lines` / `03_columns` payloads) and its file wrapper.

use pi_core::{PageSpans, Rect};
use serde_json::{Value, json};

use crate::model::span_line::peek_text_of_line;
use crate::phases::{PageLayout, page_bbox_from_viewbox, process_page};

/// `dump_reference.py::num`: non-finite floats become "inf" / "-inf" / "nan".
pub fn num(x: f64) -> Value {
    if x.is_nan() {
        json!("nan")
    } else if x.is_infinite() {
        json!(if x > 0.0 { "inf" } else { "-inf" })
    } else {
        json!(x)
    }
}

/// `dump_reference.py::rect`: `[left, right, top, bottom]`.
pub fn rect(r: &Rect) -> Value {
    json!([num(r.left), num(r.right), num(r.top), num(r.bottom)])
}

/// One page of `02_lines` (`{"page", "page_bbox", "stats", "lines"}`).
pub fn page_lines(p: &PageLayout) -> Value {
    let lines: Vec<Value> = p
        .lines
        .iter()
        .map(|l| {
            json!({
                "bbox": rect(&l.bbox),
                "text": peek_text_of_line(l, &p.spans),
                "spans": l.spans,
                "column": l.column,
                "bold_frac": num(l.bold_frac),
                "italic_frac": num(l.italic_frac),
                "skew_frac": num(l.skew_frac),
                "avg_font_size": num(l.avg_font_size),
                "max_span_height": num(l.max_span_height),
                "numbering_kind": l.numbering_kind,
                "numbering_text": l.numbering_text,
                "ink_density": num(l.ink_density),
            })
        })
        .collect();
    json!({
        "page": p.page,
        "page_bbox": rect(&p.bounds),
        "stats": serde_json::to_value(&p.stats).expect("page stats"),
        "lines": lines,
    })
}

/// One page of `03_columns` (`{"page", "columns"}`).
pub fn page_columns(p: &PageLayout) -> Value {
    json!({"page": p.page, "columns": p.columns.iter().map(rect).collect::<Vec<_>>()})
}

/// Runs stages 02-03 over a document's `01_spans` pages, as `flash/main.py::extract_toc` does.
pub fn process_document(pages: &[PageSpans]) -> Vec<PageLayout> {
    pages
        .iter()
        .map(|p| {
            let vb = p.viewbox.expect("reference requires a view box");
            process_page(&p.spans, p.page, page_bbox_from_viewbox(vb, p.rotation))
        })
        .collect()
}

/// The stage file wrapper written by `dump_reference.py::dump`.
pub fn wrap(doc: &str, stage: &str, data: Value) -> Value {
    json!({
        "schema": 1,
        "reference": "619cbd8",
        "producer": "pi-layout",
        "unicode": pi_pycompat::unicode::UNIDATA_VERSION,
        "doc": doc,
        "stage": stage,
        "data": data,
    })
}
