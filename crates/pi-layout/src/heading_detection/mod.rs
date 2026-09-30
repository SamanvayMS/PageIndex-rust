//! Heading detection: candidates, neighbour maps, detectors and `find_section_openers`.
//!
//! ref: pageindex/flash/heading_detection/ (and the candidate / style-context types from
//! pageindex/flash/outline_assembly/ that it depends on)

pub mod candidate;
pub mod detectors;
pub mod neighbors;

pub use candidate::{HeadingCandidate, OutlineContext, OutlineNode, format_number};
pub use detectors::{PageScanState, find_section_openers, matches_references};
pub use neighbors::PageNeighborMap;
