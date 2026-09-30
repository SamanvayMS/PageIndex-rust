//! Span producers for the PageIndex pipeline.
//!
//! - [`extract_pdf`]: the PDF text layer (PDFium chars + lopdf content streams), a port of the
//!   reference's `parser_pdfium_charlevel` (stage 01).
//! - [`read_page_spans_json`]: `PageSpans` from JSON (OCR producers, parity goldens).

// Nested `if`s deliberately mirror the reference's control flow line by line, which keeps the
// port reviewable against the Python; collapsing them into let-chains would obscure that.
#![allow(clippy::collapsible_if)]

mod char_extract;
mod cmap;
mod code_walk;
mod content_stream;
mod font_unicode;
mod geometry;
mod merge;
mod model;
pub mod pdfium;
mod pdfobj;
mod pipeline;
mod pyuni;
mod remerge;
mod spans;
mod text_normalize;
mod unicode_apply;

pub use pipeline::{extract_pdf, extract_pdf_bytes};

/// Read `PageSpans` pages from JSON: either a bare list or a parity stage file
/// (`{"data": [...]}`).
pub fn read_page_spans_json(text: &str) -> anyhow::Result<Vec<pi_core::PageSpans>> {
    let v: serde_json::Value = serde_json::from_str(text)?;
    let data = v.get("data").cloned().unwrap_or(v);
    Ok(serde_json::from_value(data)?)
}
