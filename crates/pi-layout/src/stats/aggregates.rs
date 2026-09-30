//! Page-level and document-level statistics aggregation.
//!
//! ref: pageindex/flash/stats/aggregates.py

use indexmap::IndexMap;
use pi_core::Rect;
use pi_pycompat::pysort;
use serde::Serialize;

use super::scripts::{ScriptHistogram, dominant_script_family, tally_scripts};
use crate::consts::{OVERLAP_BUCKETS, SCRIPT_CHAR_CAP};
use crate::model::char_stats::{info_weight, max_nan};
use crate::model::rects::Bounded;
use crate::model::span_line::{Line, Span, style_key};
use crate::phases::page_view::PageLayout;

// ref: stats/aggregates.py::_percentile_sample_cmp (as `cmp_to_key(...)`'s `<`)
fn sample_lt(a: &(f64, f64), b: &(f64, f64)) -> bool {
    let d = if a.0 != b.0 { a.0 - b.0 } else { a.1 - b.1 };
    d < 0.0
}

/// Weighted percentile over `(value, weight)` samples; NaN for empty input or a percentile
/// outside [0, 100]. The sort is CPython's timsort with the reference comparator, whose NaN
/// results compare as "equal".
// ref: stats/aggregates.py::weighted_percentile
// `pct < 0 || pct > 100` (not a range test): a NaN percentile proceeds, as in the reference.
#[allow(clippy::manual_range_contains)]
pub fn weighted_percentile(mut values: Vec<(f64, f64)>, pct: f64) -> f64 {
    if values.is_empty() || pct < 0.0 || pct > 100.0 {
        return f64::NAN;
    }
    pysort::sort_by_lt(&mut values, sample_lt);
    let total: f64 = values.iter().fold(0.0, |acc, v| acc + v.1);
    let target = total * pct / 100.0;
    let mut acc = 0.0;
    let mut prev: Option<f64> = None;
    for &(value, weight) in &values {
        if acc == target {
            return match prev {
                None => value,
                Some(p) => (p + value) / 2.0,
            };
        }
        prev = Some(value);
        acc += weight;
        if acc > target {
            return value;
        }
    }
    prev.unwrap_or(f64::NAN)
}

/// ref: stats/aggregates.py::PageStats (serialized with `dump_reference.py::page_stats_d` names)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PageStats {
    pub line_count: u32,
    /// ref slot: `secondary_slot`
    #[serde(with = "pi_core::float")]
    pub total_line_weight: f64,
    /// ref slot: `tertiary_slot`
    #[serde(with = "pi_core::float")]
    pub median_overlap_gap: f64,
    /// ref slot: `previous_slot`
    #[serde(with = "pi_core::float")]
    pub median_line_width: f64,
    /// ref slot: `style_slot`
    #[serde(with = "pi_core::float")]
    pub median_char_count: f64,
    /// Weighted median ink density. ref slot: `cache_slot`
    #[serde(with = "pi_core::float")]
    pub median_density: f64,
    /// ref slot: `option_slot`
    #[serde(with = "pi_core::float")]
    pub median_center_y: f64,
    /// ref slot: `primary_slot`
    #[serde(with = "pi_core::float")]
    pub median_font_size: f64,
    /// ref slot: `measure_slot`
    #[serde(with = "pi_core::float")]
    pub avg_char_width: f64,
    /// ref slot: `state_slot`
    pub dominant_font: String,
    /// ref slot: `auxiliary_slot`
    pub dominant_style: String,
}

// ref: stats/aggregates.py::compute_page_stats
pub fn compute_page_stats<'a>(
    page: &Rect,
    lines: impl IntoIterator<Item = &'a Line>,
    spans: &[Span],
) -> PageStats {
    let mut overlap_gap_samples = Vec::new();
    let mut line_width_samples = Vec::new();
    let mut char_count_samples = Vec::new();
    let mut density_samples = Vec::new();
    let mut center_y_samples = Vec::new();
    let mut font_size_samples = Vec::new();
    let mut font: IndexMap<&str, f64> = IndexMap::new();
    let mut style: IndexMap<String, f64> = IndexMap::new();
    let mut chars: u64 = 0;
    let mut width = 0.0;
    let mut total = 0.0;
    let mut valid = 0u32;

    let bucket_size = page.bbox_width() / OVERLAP_BUCKETS as f64;
    let mut buckets: [Option<&Line>; OVERLAP_BUCKETS + 1] = [None; OVERLAP_BUCKETS + 1];

    for line in lines {
        if line.skew_frac > 1.0 {
            continue;
        }
        valid += 1;
        for &sid in &line.spans {
            let span = &spans[sid];
            let w = info_weight(&span.char_stats);
            let w = w * w * span.bbox_height();
            *font.entry(span.font_name.as_str()).or_insert(0.0) += w;
            *style.entry(style_key(span)).or_insert(0.0) += w;
            chars += span.char_count() as u64;
            width += span.bbox_width();
        }
        let line_weight = info_weight(&line.char_stats);
        total += line_weight;
        let sample = line_weight * line.avg_font_size;
        font_size_samples.push((line.avg_font_size, sample));
        line_width_samples.push((line.bbox_width(), sample));
        char_count_samples.push((line.char_count() as f64, sample));
        density_samples.push((line.ink_density, line.area()));
        center_y_samples.push((line.center_y(), sample));

        let left = line.left_edge();
        let right = line.right_edge();
        if !(left < f64::INFINITY && right > f64::NEG_INFINITY) {
            continue;
        }
        if bucket_size <= 0.0 {
            continue;
        }
        let bucket_left: i64 = if left == f64::NEG_INFINITY {
            0
        } else {
            ((left / bucket_size).trunc() as i64).max(0)
        };
        let bucket_right: i64 = if right == f64::INFINITY {
            OVERLAP_BUCKETS as i64
        } else {
            ((right / bucket_size).ceil() as i64).min(OVERLAP_BUCKETS as i64)
        };
        let mut best_gap = f64::INFINITY;
        let mut best_prev: Option<&Line> = None;
        let mut idx = bucket_left;
        while idx < bucket_right {
            let prev = buckets[idx as usize];
            buckets[idx as usize] = Some(line);
            idx += 1;
            let Some(prev) = prev else { continue };
            let gap =
                pi_pycompat::pymath::max(prev.bottom_edge(), line.top_edge()) - line.bottom_edge();
            if gap < best_gap {
                best_gap = gap;
                best_prev = Some(prev);
            }
        }
        if best_gap < f64::INFINITY
            && let Some(prev) = best_prev
        {
            overlap_gap_samples.push((best_gap, info_weight(&prev.char_stats) * line_weight));
        }
    }

    let mut dominant_font = String::new();
    let mut dominant_font_weight = 0.0;
    for (name, &w) in &font {
        if w > dominant_font_weight {
            dominant_font_weight = w;
            dominant_font = name.to_string();
        }
    }
    let mut dominant_style = String::new();
    let mut dominant_style_weight = 0.0;
    for (name, &w) in &style {
        if w > dominant_style_weight {
            dominant_style_weight = w;
            dominant_style = name.clone();
        }
    }

    PageStats {
        line_count: valid,
        total_line_weight: total,
        median_overlap_gap: weighted_percentile(overlap_gap_samples, 50.0),
        median_line_width: weighted_percentile(line_width_samples, 50.0),
        median_char_count: weighted_percentile(char_count_samples, 50.0),
        median_density: weighted_percentile(density_samples, 50.0),
        median_center_y: weighted_percentile(center_y_samples, 50.0),
        median_font_size: weighted_percentile(font_size_samples, 50.0),
        avg_char_width: if chars != 0 {
            width / chars as f64
        } else if width > 0.0 {
            f64::INFINITY
        } else {
            f64::NAN
        },
        dominant_font,
        dominant_style,
    }
}

/// ref: stats/aggregates.py::DocStats (serialized with `dump_reference.py::doc_stats_d` names)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DocStats {
    /// ref slot: `tertiary_slot`
    pub dominant_script: u8,
    /// ref slot: `style_slot`
    pub landscape_pages: u32,
    /// ref slot: `cache_slot`
    pub total_lines: u64,
    /// ref slot: `state_slot`
    #[serde(with = "pi_core::float")]
    pub total_weight: f64,
    /// ref slot: `previous_slot`
    #[serde(with = "pi_core::float")]
    pub max_page_weight: f64,
    /// ref slot: `secondary_slot`
    #[serde(with = "pi_core::float")]
    pub median_page_weight: f64,
    /// ref slot: `option_slot`
    #[serde(with = "pi_core::float")]
    pub median_line_width: f64,
    /// 80th percentile of the per-page median density. ref slot: `auxiliary_slot`
    #[serde(with = "pi_core::float")]
    pub p80_density: f64,
    /// ref slot: `measure_slot`
    #[serde(with = "pi_core::float")]
    pub median_center_y: f64,
    /// ref slot: `primary_slot`
    #[serde(with = "pi_core::float")]
    pub body_font_size: f64,
}

// ref: stats/aggregates.py::compute_doc_stats
pub fn compute_doc_stats(pages: &[PageLayout]) -> DocStats {
    let mut script = ScriptHistogram::default();
    let mut landscape = 0u32;
    let mut total_lines = 0u64;
    let mut total_weight = 0.0;
    let mut max_weight = 0.0;
    let mut total_weights = Vec::new();
    let mut widths = Vec::new();
    let mut densities = Vec::new();
    let mut centers = Vec::new();
    let mut font_sizes = Vec::new();

    for page in pages {
        if script.total_chars < SCRIPT_CHAR_CAP {
            for span in &page.spans {
                if script.total_chars >= SCRIPT_CHAR_CAP {
                    break;
                }
                tally_scripts(&mut script, &span.text);
            }
        }
        if page.bounds.bbox_width() > page.bounds.bbox_height() {
            landscape += 1;
        }
        let stats = &page.stats;
        total_lines += stats.line_count as u64;
        let weight = stats.total_line_weight;
        total_weight += weight;
        max_weight = max_nan(max_weight, weight);
        if stats.line_count == 0 || weight <= 0.0 {
            continue;
        }
        let per_page = pi_pycompat::pymath::min(100.0, weight / stats.line_count as f64);
        total_weights.push((weight, per_page));
        widths.push((stats.median_line_width, per_page));
        densities.push((stats.median_density, per_page));
        centers.push((stats.median_center_y, per_page));
        font_sizes.push((stats.median_font_size, per_page));
    }

    DocStats {
        dominant_script: dominant_script_family(&script),
        landscape_pages: landscape,
        total_lines,
        total_weight,
        max_page_weight: max_weight,
        median_page_weight: weighted_percentile(total_weights, 50.0),
        median_line_width: weighted_percentile(widths, 50.0),
        p80_density: weighted_percentile(densities, 80.0),
        median_center_y: weighted_percentile(centers, 50.0),
        body_font_size: weighted_percentile(font_sizes, 50.0),
    }
}

/// Column index stored on a container's first child. The reference passes blocks here, so the
/// first child is a line and its column is returned.
// ref: stats/aggregates.py::column_index_of
pub fn column_index_of_first_line(first: Option<&Line>) -> i32 {
    first.map_or(-1, |l| l.column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_ties_and_overshoot() {
        assert_eq!(weighted_percentile(vec![(1.0, 1.0), (3.0, 1.0)], 50.0), 2.0);
        // acc == target at the second sample: average with the previous value.
        assert_eq!(
            weighted_percentile(vec![(1.0, 1.0), (3.0, 1.0), (5.0, 2.0)], 50.0),
            4.0
        );
        assert_eq!(weighted_percentile(vec![(1.0, 1.0), (3.0, 2.0)], 50.0), 3.0);
        assert!(weighted_percentile(vec![], 50.0).is_nan());
        assert_eq!(weighted_percentile(vec![(2.0, 0.0)], 50.0), 2.0);
    }
}
