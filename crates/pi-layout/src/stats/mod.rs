//! Page-level and document-level layout statistics.
//!
//! ref: pageindex/flash/stats/

pub mod aggregates;
pub mod scripts;

pub use aggregates::{
    DocStats, PageStats, compute_doc_stats, compute_page_stats, weighted_percentile,
};
