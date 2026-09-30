//! Per-page pipeline orchestration.
//!
//! ref: pageindex/flash/phases/

pub mod classify;
pub mod document;
pub mod line_numbers;
pub mod page_view;

pub use classify::{Classified, classify_document};
pub use document::{BlockRef, DocPage, Document, build_document};
pub use page_view::{PageLayout, page_bbox_from_viewbox, process_page};
