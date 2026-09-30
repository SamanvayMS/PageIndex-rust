//! Span and Line types with text and style helpers.
//!
//! ref: pageindex/flash/model/span_line.py
//!
//! Storage is arena style: a page owns `Vec<Span>` (indexed by [`SpanId`], the position in the
//! producer's span list, which is also the index written to `01_spans.json`) and lines refer to
//! spans by id. Functions that need span data take the arena explicitly.

use std::sync::LazyLock;

use pi_core::Rect;
use regex::Regex;

use super::char_stats::{CharStats, info_weight, letter_count, merge_char_stats, trim_unicode_ws};
use super::rects::{Bounded, EMPTY_RECT, rect_union};
use pi_pycompat::pymath;

/// Index of a span in the page's span arena (= position in the producer's span list).
pub type SpanId = usize;

// ref: model/span_line.py:34 `_bold_font_re` (re.IGNORECASE | re.ASCII -> `(?i-u)`)
static BOLD_FONT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i-u)(bold|timesb)").unwrap());
// ref: model/span_line.py:35 `_italic_font_re` (`\Z` -> `\z`)
static ITALIC_FONT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i-u)(ital|it\z|i[1-9][0-9]*\z|obliq)").unwrap());
// ref: model/span_line.py:47 `_subset_prefix_re`
static SUBSET_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Z]{6}\+").unwrap());

/// ref: model/span_line.py:37 `_font_name_aliases`
const FONT_NAME_ALIASES: [(&str, &str); 6] = [
    ("timesnewroman", "Times"),
    ("times-new-roman", "Times"),
    ("timesroman", "Times"),
    ("times-roman", "Times"),
    ("timesnew", "Times"),
    ("times-new", "Times"),
];

/// ref: model/span_line.py::Span
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    /// ref slot: `secondary_slot` (Bounded)
    pub bbox: Rect,
    pub text: String,
    /// Text trimmed of the package whitespace set. ref slot: `state_slot`
    pub trimmed_text: String,
    pub char_stats: CharStats,
    /// ref slot: `previous_slot`
    pub skew: f64,
    pub font_family: String,
    pub font_name: String,
    pub font_size: f64,
    /// ref slot: `primary_slot`
    pub bold: bool,
    /// ref slot: `measure_slot`
    pub italic: bool,
}

impl Bounded for Span {
    fn rect(&self) -> &Rect {
        &self.bbox
    }
}

impl Span {
    /// The reference constructor: normalizes the raw font name (subset prefix, aliases) and
    /// derives bold (flag or name) and italic (name only; the `italic` argument is ignored).
    // ref: model/span_line.py::Span.__init__
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        bbox: Rect,
        text: &str,
        font_name_raw: &str,
        font_size: f64,
        bold: bool,
        _italic: bool,
        skew: f64,
        font_family: &str,
    ) -> Self {
        let mut name = font_name_raw;
        if SUBSET_PREFIX_RE.is_match(name) {
            // `reference_item[7:]`: 6 ASCII letters + '+' are 7 bytes.
            name = &name[7..];
        }
        let lowered = pi_pycompat::unicode::lower(name);
        let font_name = FONT_NAME_ALIASES
            .iter()
            .find(|(k, _)| *k == lowered)
            .map(|(_, v)| v.to_string())
            .unwrap_or_else(|| name.to_string());
        let bold = bold || BOLD_FONT_RE.is_match(&font_name);
        let italic = ITALIC_FONT_RE.is_match(&font_name);
        Self::with_style(
            bbox,
            text,
            font_name,
            font_size,
            bold,
            italic,
            skew,
            font_family,
        )
    }

    /// Build from values that already went through the reference constructor (the shape of
    /// `pi_core::Span` / `01_spans.json`): font name, bold and italic are taken as given.
    #[allow(clippy::too_many_arguments)]
    pub fn with_style(
        bbox: Rect,
        text: &str,
        font_name: String,
        font_size: f64,
        bold: bool,
        italic: bool,
        skew: f64,
        font_family: &str,
    ) -> Self {
        let trimmed = trim_unicode_ws(text).to_string();
        let char_stats = CharStats::new(&trimmed);
        Span {
            bbox,
            text: text.to_string(),
            trimmed_text: trimmed,
            char_stats,
            skew,
            font_family: font_family.to_string(),
            font_name,
            font_size,
            bold,
            italic,
        }
    }

    pub fn from_core(s: &pi_core::Span) -> Self {
        Self::with_style(
            s.bbox,
            &s.text,
            s.font_name.clone().unwrap_or_default(),
            s.font_size,
            s.bold,
            s.italic,
            s.skew,
            "",
        )
    }

    // ref: model/span_line.py::Span.char_count
    pub fn char_count(&self) -> u32 {
        self.char_stats.total_chars
    }

    // ref: model/span_line.py::Span.font_style
    pub fn font_style(&self) -> String {
        format!("{} {}", self.font_name, if self.bold { 'B' } else { 'R' })
    }
}

/// ref: model/span_line.py::Line
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// ref slot: `secondary_slot` (Bounded)
    pub bbox: Rect,
    /// ref slot: `primary_slot`
    pub spans: Vec<SpanId>,
    pub char_stats: CharStats,
    /// First span with letters. ref slot: `alignment_slot`
    pub first_letter_span: Option<SpanId>,
    /// ref slot: `weighted_ratio_primary`
    pub bold_frac: f64,
    /// ref slot: `weighted_ratio_secondary`
    pub italic_frac: f64,
    /// ref slot: `weighted_ratio_tertiary`
    pub skew_frac: f64,
    /// ref slot: `metric_slot`
    pub avg_font_size: f64,
    /// ref slot: `previous_slot`
    pub max_span_height: f64,
    /// Column index, -1 unassigned. ref slot: `measure_slot`
    pub column: i32,
    /// Cached trimmed text. ref slot: `marker_slot`
    pub text_cache: Option<String>,
    /// -1 uncomputed, 0 none, 1 digit, 2 upper, 3 lower. ref slot: `state_slot`
    pub numbering_kind: i32,
    /// ref slot: `style_slot`
    pub numbering_text: String,
    /// ref slot: `cache_slot`
    pub ink_density: f64,
}

impl Default for Line {
    // ref: model/span_line.py::Line.__init__
    fn default() -> Self {
        Line {
            bbox: EMPTY_RECT,
            spans: Vec::new(),
            char_stats: CharStats::default(),
            first_letter_span: None,
            bold_frac: 0.0,
            italic_frac: 0.0,
            skew_frac: 0.0,
            avg_font_size: 0.0,
            max_span_height: 0.0,
            column: -1,
            text_cache: None,
            numbering_kind: -1,
            numbering_text: String::new(),
            ink_density: 0.0,
        }
    }
}

impl Bounded for Line {
    fn rect(&self) -> &Rect {
        &self.bbox
    }
}

impl Line {
    pub fn new() -> Self {
        Self::default()
    }

    // ref: model/span_line.py::Line.char_count
    pub fn char_count(&self) -> u32 {
        self.char_stats.total_chars
    }
}

// ref: model/span_line.py::append_span
pub fn append_span(line: &mut Line, sid: SpanId, spans: &[Span]) {
    let s = &spans[sid];
    line.spans.push(sid);
    let old_weight = info_weight(&line.char_stats);
    let added = info_weight(&s.char_stats);
    let total = old_weight + added;
    if total > 0.0 {
        let b = if s.bold { 1.0 } else { 0.0 };
        let i = if s.italic { 1.0 } else { 0.0 };
        line.bold_frac = (line.bold_frac * old_weight + b * added) / total;
        line.italic_frac = (line.italic_frac * old_weight + i * added) / total;
        line.skew_frac = (line.skew_frac * old_weight + s.skew * added) / total;
        line.avg_font_size = (line.avg_font_size * old_weight + s.font_size * added) / total;
    }
    merge_char_stats(&mut line.char_stats, &s.char_stats);
    if line.first_letter_span.is_none() && letter_count(&s.char_stats) > 0 {
        line.first_letter_span = Some(sid);
    }
    if s.char_count() == 0 {
        return;
    }
    let old_area = line.area();
    line.max_span_height = pymath::max(line.max_span_height, s.bbox_height());
    line.bbox = rect_union(&line.bbox, &s.bbox);
    line.ink_density = pymath::min(
        1.0,
        (line.ink_density * old_area + s.area()) / pymath::max(1.0, line.area()),
    );
    line.text_cache = None;
    line.numbering_kind = -1;
    line.numbering_text.clear();
}

// ref: model/span_line.py::last_span
pub fn last_span(line: &Line) -> SpanId {
    *line.spans.last().expect("line has spans")
}

/// ref: model/span_line.py::avg_char_width (also `avg_char_width2`, identical)
pub fn avg_char_width(bbox_width: f64, char_count: u32) -> f64 {
    if char_count == 0 {
        0.0
    } else {
        bbox_width / char_count as f64
    }
}

pub fn line_avg_char_width(line: &Line) -> f64 {
    avg_char_width(line.bbox_width(), line.char_count())
}

pub fn span_avg_char_width(span: &Span) -> f64 {
    avg_char_width(span.bbox_width(), span.char_count())
}

// ref: model/span_line.py::raw_text_of_line
pub fn raw_text_of_line(line: &Line, spans: &[Span]) -> String {
    let mut out = String::new();
    for &sid in &line.spans {
        out.push_str(&spans[sid].text);
    }
    out
}

// ref: model/span_line.py::text_of_line
pub fn text_of_line(line: &mut Line, spans: &[Span]) -> String {
    if let Some(t) = &line.text_cache {
        return t.clone();
    }
    let t = trim_unicode_ws(&raw_text_of_line(line, spans)).to_string();
    line.text_cache = Some(t.clone());
    t
}

/// `text_of_line` without filling the cache (for read-only callers such as serializers).
pub fn peek_text_of_line(line: &Line, spans: &[Span]) -> String {
    match &line.text_cache {
        Some(t) => t.clone(),
        None => trim_unicode_ws(&raw_text_of_line(line, spans)).to_string(),
    }
}

/// `str(Decimal(value).quantize(Decimal("0.1"), rounding=ROUND_HALF_UP))`: the exact double
/// rounded half away from zero to one decimal.
// ref: model/span_line.py::_format_half_up_one_decimal
pub fn format_half_up_one_decimal(value: f64) -> String {
    if !value.is_finite() {
        // Decimal("NaN") quantizes to NaN; infinities raise in the reference.
        return if value.is_nan() {
            "NaN".into()
        } else if value > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    // Rust's `{:.1}` rounds the exact binary value to nearest, ties to even. A tie needs
    // value*10 to end in exactly .5, i.e. value = odd/4 (x.25 or x.75); round those away from 0.
    let q = value * 4.0; // exact
    if q == q.trunc() && (q % 2.0).abs() == 1.0 {
        let away = value.abs() + 0.05;
        let s = format!("{away:.1}");
        return if value < 0.0 { format!("-{s}") } else { s };
    }
    format!("{value:.1}")
}

// ref: model/span_line.py::style_key
pub fn style_key(span: &Span) -> String {
    format!(
        "{} {}",
        span.font_style(),
        format_half_up_one_decimal(span.font_size)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_up_one_decimal() {
        assert_eq!(format_half_up_one_decimal(0.25), "0.3");
        assert_eq!(format_half_up_one_decimal(0.75), "0.8");
        assert_eq!(format_half_up_one_decimal(-0.25), "-0.3");
        assert_eq!(format_half_up_one_decimal(8.966), "9.0");
        assert_eq!(format_half_up_one_decimal(0.35), "0.3"); // 0.35 is below .35 in binary
        assert_eq!(format_half_up_one_decimal(-0.04), "-0.0");
        assert_eq!(format_half_up_one_decimal(9.95), "9.9"); // 9.949999...
    }

    #[test]
    fn font_regexes() {
        let r = Rect::new(0.0, 1.0, 1.0, 0.0);
        let s = Span::new(r, "x", "ABCDEF+TimesNewRoman", 10.0, false, true, 0.0, "");
        assert_eq!(s.font_name, "Times");
        assert!(!s.bold && !s.italic);
        let s = Span::new(r, "x", "Helvetica-BoldOblique", 10.0, false, false, 0.0, "");
        assert!(s.bold && s.italic);
        let s = Span::new(r, "x", "CMTI12", 10.0, false, false, 0.0, "");
        assert!(s.italic);
        let s = Span::new(r, "x", "Arial\u{17F}", 10.0, false, false, 0.0, "");
        assert!(!s.italic);
        let s = Span::new(r, "x", "SomeIt", 10.0, false, false, 0.0, "");
        assert!(s.italic);
    }
}
