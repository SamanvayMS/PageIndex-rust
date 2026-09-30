//! Per-page heading-candidate detection (stage 06) and section openers.
//!
//! ref: pageindex/flash/heading_detection/

pub mod candidates;
pub mod detectors;
pub mod keyword_tables;
pub mod neighbors;
pub mod page_scan;
pub mod style_detectors;
pub mod text_checks;

pub use page_scan::{build_doc_heading_candidates, find_section_openers, scan_page_headings};
