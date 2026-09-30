//! Heading candidates, outline assembly and the outline tree (stages 06-08).
//!
//! Port of the PageIndex "Flash" indexer (reference `pageindex/flash` @619cbd8, MIT):
//! `heading_detection/*` (stage 06, including `find_section_openers`, which the reference runs
//! during stage 05), `outline_assembly/*` (stage 07 and the validity gates of
//! `main.py::extract_toc`) and `outline_to_dict_tree` (stage 08).
//!
//! Every ported function carries a `// ref: path.py::func` pointer into the reference.

pub mod dump;
pub mod golden;
pub mod heading_detection;
pub mod model;
pub mod outline_assembly;
pub mod pipeline;

pub use model::{Cand, Doc, HeadingCandidate, Node, OutlineNode};
pub use pipeline::{OutlineOutput, extract_outline};
