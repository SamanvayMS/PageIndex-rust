//! Named thresholds of the stage 02-03 layout pipeline (reference @619cbd8).
//!
//! Only the constants that parameterize a stage are named here; numeric literals that are part
//! of a single ported scoring formula (e.g. the gutter score multipliers in
//! `columns/gutters.py::_score_gutter_gap`) stay inline next to the formula they belong to, each
//! function carrying its `// ref:` pointer.

/// First clustering pass tolerance, in line heights. ref: phases/page_view.py:109
pub const CLUSTER_TOL_FIRST: f64 = 0.75;
/// Second (column-aware) clustering pass tolerance. ref: phases/page_view.py:117
pub const CLUSTER_TOL_SECOND: f64 = 0.5;

/// Tiny glyph cutoff as a fraction of the page area. ref: clustering/build.py:44
pub const TINY_MARK_AREA_FRAC: f64 = 1e-6;
/// Overstrike duplicate geometry tolerance (fraction of the first span's size).
/// ref: clustering/build.py:64-65, 172-173
pub const OVERSTRIKE_TOL_FRAC: f64 = 0.1;
/// Exact-duplicate position tolerance. ref: clustering/build.py:175
pub const EXACT_DUP_TOL: f64 = 0.01;
/// Max spans of a line that can absorb a single-span overstrike. ref: clustering/build.py:169
pub const OVERSTRIKE_MAX_SPANS: usize = 5;
/// Lines whose last span has skew above this are emitted unclustered. ref: clustering/build.py:145
pub const ROTATED_SKEW: f64 = 1.0;
/// Label stack: display size vs body, and vs the text below. ref: clustering/build.py:108
pub const LABEL_STACK_BODY_RATIO: f64 = 2.0;
pub const LABEL_STACK_LOWER_RATIO: f64 = 1.5;

/// Number of horizontal buckets for the overlap-gap statistic. ref: stats/aggregates.py:117-118
pub const OVERLAP_BUCKETS: usize = 20;
/// Script histogram character cap. ref: stats/aggregates.py:263
pub const SCRIPT_CHAR_CAP: u64 = 100_000;

/// Line-number candidates must start left of this page-width fraction. ref: phases/line_numbers.py:112
pub const LINE_NUMBER_MAX_LEFT_FRAC: f64 = 0.15;
/// Clusters hugging the left edge are accepted directly. ref: phases/line_numbers.py:63
pub const LINE_NUMBER_EDGE_FRAC: f64 = 0.05;
/// Minimum lines in a line-number cluster. ref: phases/line_numbers.py:61
pub const LINE_NUMBER_MIN_LINES: usize = 5;
/// Upper bound (exclusive) of a line number. ref: phases/line_numbers.py:45
pub const LINE_NUMBER_MAX: f64 = 1e4;
/// Irregular spacing ratio above which a cluster is not a line-number column. ref: phases/line_numbers.py:93
pub const LINE_NUMBER_GAP_RATIO: f64 = 1.3;
/// Body weight fraction that must sit beside the cluster. ref: phases/line_numbers.py:102
pub const LINE_NUMBER_COVERAGE: f64 = 0.8;
