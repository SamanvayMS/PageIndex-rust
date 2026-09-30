//! Block clustering: lines into blocks, heading+body splitting.
//!
//! ref: pageindex/flash/blocks/

pub mod build;
pub mod join_rules;

pub use build::{BlockList, cluster_lines_into_blocks};
pub use join_rules::{BlockClusterContext, should_join_line_to_block};
