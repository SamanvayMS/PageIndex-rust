//! Named parameters of stages 06-08 (reference @619cbd8).
//!
//! As in `pi_layout::consts`, only the constants that parameterize a stage are named here;
//! numeric literals that belong to a single ported predicate stay inline next to it, each
//! function carrying its `// ref:` pointer.

/// Neighbor-map bucket width floor (points) and page-width divisor.
/// ref: heading_detection/neighbors.py:81
pub const NEIGHBOR_BUCKET_MIN_WIDTH: f64 = 5.0;
pub const NEIGHBOR_BUCKET_DIVISOR: f64 = 300.0;

/// Pages with this many raw candidates are treated as noise and contribute none.
/// ref: heading_detection/page_scan.py:318
pub const MAX_PAGE_CANDIDATES: usize = 20;

/// A matching signature closer than this many pages is a duplicate.
/// ref: outline_assembly/style_context.py:69
pub const NEARBY_DUPLICATE_PAGES: i64 = 20;

/// Clique-tree depth cap. ref: outline_assembly/cliques.py:147
pub const MAX_CLIQUE_DEPTH: i32 = 8;

/// Body text (info weight) before the first cluster candidate that aborts sub-heading
/// extraction. ref: outline_assembly/selection.py:264
pub const SUBHEADING_CONTENT_CAP: f64 = 1000.0;

/// Invalid outlines whose largest heading gap exceeds this fraction of the page count are
/// dropped, for documents of at least `MAX_GAP_MIN_PAGES` pages. ref: main.py:267-268
pub const MAX_GAP_PAGE_FRACTION: f64 = 0.85;
pub const MAX_GAP_MIN_PAGES: usize = 3;
