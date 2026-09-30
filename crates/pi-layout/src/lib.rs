//! Lines, columns, blocks, classification.
//!
//! Port of the PageIndex "Flash" layout indexer (reference `pageindex/flash` @619cbd8, MIT).
//! Implemented so far: stages 02 (lines + page statistics) and 03 (columns) via
//! [`process_page`], and document statistics via [`compute_doc_stats`].
//!
//! Every ported function carries a `// ref: path.py::func` pointer into the reference.

pub mod blocks;
pub mod clustering;
pub mod columns;
pub mod consts;
pub mod dump;
pub mod labels;
pub mod model;
pub mod phases;
pub mod stats;
pub mod tokens;

pub use phases::{PageLayout, page_bbox_from_viewbox, process_page};
pub use stats::{DocStats, PageStats, compute_doc_stats};
