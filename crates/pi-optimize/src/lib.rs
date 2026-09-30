//! Tree merge / expand / relabel: a port of `pageindex/tree_optimize.py` and the
//! `flash/api.py` post-processing around it (VectifyAI/PageIndex @619cbd8, MIT).
//!
//! Search cost is measured in pages with a routing cost R(v) = 1. `merge` collapses a subtree
//! whose structure does not beat a linear scan (`S(v) <= tree_cost(v)`, ties merge), keeping the
//! removed titles as the parent's `key_items`; `merge_same_page` fuses frontier siblings that
//! cover the same pages; `expand` asks a model for subsections of large collapsed nodes.

pub mod consts;
pub mod cost;
pub mod expand;
pub mod flash;
pub mod log;
pub mod merge;
pub mod optimize;
pub mod tree;

pub use flash::{Prepared, postprocess_merge, prepare};
pub use optimize::{
    OnFinal, OptimizeError, OptimizeOptions, Outcome, optimize, optimize_merge_only,
};
pub use tree::{Nid, Tree, TreeError};
