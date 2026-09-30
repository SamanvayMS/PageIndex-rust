//! Outline assembly chain (stage 07) and the outline tree (stage 08).
//!
//! ref: pageindex/flash/outline_assembly/

pub mod assembly;
pub mod cliques;
pub mod selection;
pub mod style_context;

pub use assembly::{
    assemble_outline, compute_max_heading_gap, has_table_or_prominent, mark_outline_block_types,
    outline_to_dict_tree,
};
pub use selection::{is_chapter_outline_valid, is_outline_valid};
