//! Per-page pipeline orchestration.
//!
//! ref: pageindex/flash/phases/

pub mod line_numbers;
pub mod page_view;

pub use page_view::{PageLayout, page_bbox_from_viewbox, process_page};
