//! Column detection via sweep-line gutter scoring and recursive page splitting.
//!
//! ref: pageindex/flash/columns/

pub mod gutters;
pub mod splitting;

pub use gutters::{ColumnDetectionContext, LineStore};
pub use splitting::{columns_to_x_bounds, detect_columns};
