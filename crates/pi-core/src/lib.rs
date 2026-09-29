//! Core types shared by every stage: geometry, spans and per-page span lists.
//!
//! Everything downstream of `pi-extract` consumes `&[PageSpans]` and must not care which
//! producer (PDF text layer or OCR) made the spans.

pub mod float;
pub mod geometry;
pub mod span;

pub use geometry::Rect;
pub use span::{PageSpans, Span, SpanSource};
