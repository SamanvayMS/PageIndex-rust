//! Line clustering: initial lines from spans, then neighbour merging.
//!
//! ref: pageindex/flash/clustering/

pub mod build;
pub mod merge_rules;

pub use build::{LineId, build_initial_lines, cluster_lines};
pub use merge_rules::ColBounds;
