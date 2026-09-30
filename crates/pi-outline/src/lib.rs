//! Heading candidates, outline assembly, the outline tree (stages 06-08) and embedded
//! bookmarks (stage 09).
//!
//! Port of the PageIndex "Flash" indexer (reference `pageindex/flash` @619cbd8, MIT):
//! `heading_detection/*` (stage 06), `outline_assembly/*` (stage 07 and the validity gates of
//! `main.py::extract_toc`), `outline_to_dict_tree` (stage 08) and `embedded_toc` (stage 09).
//!
//! Every ported function carries a `// ref: path.py::func` pointer into the reference.

pub mod consts;
pub mod dump;
pub mod embedded_toc;
pub mod golden;
pub mod heading_detection;
pub mod model;
pub mod outline_assembly;
pub mod pipeline;

pub use embedded_toc::{PdfSource, apply_embedded_toc};
pub use model::{Cand, Doc, HeadingCandidate, Node, OutlineNode};
pub use pipeline::{OutlineOutput, extract_outline};
