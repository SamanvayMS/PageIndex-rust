//! Block classification: header/footer, watermarks, TOC pages, boilerplate, body paragraphs.
//!
//! ref: pageindex/flash/classification/

pub mod body_text;
pub mod header_footer;
pub mod keyword_tables;
pub mod toc_boilerplate;

pub use body_text::is_body_paragraph;
pub use header_footer::{bounded_edit_distance, detect_header_footer};
pub use toc_boilerplate::{mark_toc_and_boilerplate, mark_watermarks};
