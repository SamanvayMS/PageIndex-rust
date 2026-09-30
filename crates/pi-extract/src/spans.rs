//! Final emission: merged items → `pi_core::Span`.
//!
//! ref: parser_pdfium_charlevel/pipeline.py::_page_spans, cmap_parse.py::_compute_skew,
//! model/span_line.py::Span.__init__ (font-name normalization, bold/italic regexes).

use std::sync::LazyLock;

use pi_core::{Rect, Span, SpanSource};
use regex::Regex;

use crate::model::{Item, TextObj};
use crate::text_normalize::{
    apply_bidi_reordering, normalized_piece, reverse_if_rtl, translate_drop_chars,
};

// ref: model/span_line.py:34-35 — re.IGNORECASE | re.ASCII; `\Z` -> `\z`.
static BOLD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i-u)(bold|timesb)").expect("bold re"));
static ITALIC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i-u)(ital|it\z|i[1-9][0-9]*\z|obliq)").expect("italic re"));
static SUBSET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z]{6}\+").expect("subset re"));

/// ref: cmap_parse.py::_ieee_div
fn ieee_div(a: f64, b: f64) -> f64 {
    if b != 0.0 {
        return a / b;
    }
    if a == 0.0 || a.is_nan() {
        return f64::NAN;
    }
    if (a > 0.0) == (b.is_sign_positive()) {
        f64::INFINITY
    } else {
        f64::NEG_INFINITY
    }
}

/// ref: cmap_parse.py::_compute_skew — (b/a)² + (c/d)²
pub fn compute_skew(m: &[f64; 4]) -> f64 {
    let q1 = ieee_div(m[1], m[0]);
    let q2 = ieee_div(m[2], m[3]);
    q1 * q1 + q2 * q2
}

/// ref: span_line.py::Span.__init__ font-name normalisation.
pub fn normalize_font_name(raw: &str) -> String {
    let mut name = raw;
    if SUBSET_RE.is_match(name) {
        name = match name.char_indices().nth(7) {
            Some((i, _)) => &name[i..],
            None => "",
        };
    }
    match name.to_lowercase().as_str() {
        "timesnewroman" | "times-new-roman" | "timesroman" | "times-roman" | "timesnew"
        | "times-new" => "Times".to_string(),
        _ => name.to_string(),
    }
}

/// ref: pipeline.py::_page_spans
pub fn page_spans(items: &[Item], objects: &[TextObj]) -> Vec<Span> {
    let mut spans = Vec::new();
    for it in items {
        let joined: String = it
            .pieces
            .iter()
            .map(|p| reverse_if_rtl(&normalized_piece(p)))
            .collect();
        let joined = apply_bidi_reordering(&joined, -1, objects[it.obj].vertical);
        let text = translate_drop_chars(&joined);
        if text.is_empty() {
            continue;
        }
        let font_name = normalize_font_name(&it.font_name);
        let bold = BOLD_RE.is_match(&font_name);
        let italic = ITALIC_RE.is_match(&font_name);
        spans.push(Span {
            bbox: Rect::new(it.left, it.right, it.top, it.bottom),
            text,
            font_name: Some(font_name),
            font_size: it.fs,
            bold,
            italic,
            skew: compute_skew(&it.mtx0.unwrap_or(objects[it.obj].mtx)),
            source: SpanSource::TextLayer,
        });
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_names() {
        assert_eq!(normalize_font_name("ABCDEF+Times-Roman"), "Times");
        assert_eq!(normalize_font_name("Helvetica-Bold"), "Helvetica-Bold");
        assert!(BOLD_RE.is_match("TimesB"));
        assert!(ITALIC_RE.is_match("CMTI10")); // "I10" matches i[1-9][0-9]*\z, as in Python
        assert!(ITALIC_RE.is_match("Arial-Italic"));
        assert!(ITALIC_RE.is_match("CMMI10"));
        assert!(!BOLD_RE.is_match("Bſld"));
    }

    #[test]
    fn skew() {
        assert_eq!(compute_skew(&[1.0, 0.0, 0.0, 1.0]), 0.0);
        assert!(compute_skew(&[0.0, 1.0, -1.0, 0.0]).is_infinite());
    }
}
