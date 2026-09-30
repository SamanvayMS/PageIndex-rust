//! Body-paragraph classification and recurring-text recording.
//!
//! ref: pageindex/flash/classification/body_text.py

use pi_pycompat::{pymath, unicode};

use super::keyword_tables::{BOILERPLATE_TRIE, normalize_text_key, page_number_only};
use crate::model::block::{Block, alignment_code, first_span_of, last_span_of};
use crate::model::char_stats::{info_weight, letter_count, round_half_up_to_int};
use crate::model::numbering::to_number;
use crate::model::rects::Bounded;
use crate::model::span_line::Span;
use crate::phases::{DocPage, Document};
use crate::stats::DocStats;
use crate::tokens::{jenkins_hash, tokenize_block};

// ref: classification/body_text.py::record_recurring_text
pub fn record_recurring_text(doc: &Document, text: &str) {
    *doc.recurring
        .borrow_mut()
        .entry(jenkins_hash(text))
        .or_insert(0) += 1;
}

// ref: classification/body_text.py::is_body_paragraph
pub fn is_body_paragraph(doc_stats: &DocStats, page: &DocPage, block: &Block) -> bool {
    let pl = &page.layout;
    if block.density_chars < 0.6 {
        return false;
    }
    let width = info_weight(&block.char_stats);
    let lines = block.line_count() as f64;
    let sp = block.char_stats.category_counts[6] as f64;
    let dw = if lines != 0.0 {
        width / lines
    } else if width > 0.0 {
        f64::INFINITY
    } else {
        f64::NAN
    };
    if dw < 15.0
        || (lines >= 10.0 && dw < 20.0)
        || (lines >= 10.0 && dw < 25.0 && sp < lines / 8.0)
        || (lines >= 20.0 && dw < 40.0 && sp < lines / 20.0)
    {
        return false;
    }
    let bw = block.bbox_width();
    if lines >= 4.0 {
        let mut short = 0.0;
        for &lid in &block.lines {
            let l = &pl.lines[lid];
            if l.bbox_width() < 0.75 * bw && !pl.spans[l.spans[0]].trimmed_text.starts_with('•') {
                short += 1.0;
            }
        }
        if short >= lines / 2.0 && sp < lines / 8.0 {
            return false;
        }
    }
    if bw < pl.bounds.bbox_width() / 7.0 {
        return false;
    }
    let chars = block.char_count() as f64;
    if chars < 40.0
        || (lines >= 3.0 && alignment_code(block, pl) == 3)
        || (letter_count(&block.char_stats) as f64) < 0.1 * chars
    {
        return false;
    }
    let size = block.weighted_font_size;
    let body_size = pymath::min(pl.stats.median_font_size, doc_stats.body_font_size);
    let mut min_value = pymath::min(
        doc_stats.body_font_size,
        pymath::max(pl.bounds.bbox_height(), pl.bounds.bbox_width()) / 60.0,
    );
    min_value = pymath::min(0.7 * min_value, min_value - 3.0);
    if size < body_size - 2.0
        || size < min_value
        || BOILERPLATE_TRIE
            .prefix_match(&tokenize_block(block, pl))
            .is_some()
    {
        return false;
    }
    if chars >= 250.0 && lines >= 4.0 {
        return true;
    }
    if bw < pl.bounds.bbox_width() / 5.0 || size < body_size - 0.5 {
        return false;
    }
    if chars >= 100.0 && lines >= 2.0 && sp >= 2.0 {
        return true;
    }
    if (chars >= 100.0 || block.char_stats.last_cat == 6)
        && (size >= pl.stats.median_font_size - 0.5 || size > doc_stats.body_font_size - 0.1)
    {
        let font = &pl.stats.dominant_font;
        return pl.spans[first_span_of(block, pl)].font_name == *font
            || pl.spans[last_span_of(block, pl)].font_name == *font;
    }
    false
}

// ref: classification/body_text.py::span_style_text_key
pub fn span_style_text_key(span: &Span) -> String {
    format!(
        "{} {} {} {}",
        span.font_name,
        round_half_up_to_int(span.bbox_height()) as i64,
        if span.bold { 'B' } else { 'R' },
        unicode::lower(&span.text)
    )
}

// ref: classification/body_text.py::normalized_block_text
pub fn normalized_block_text(block: &Block, page: &DocPage) -> String {
    let mut out = String::new();
    for t in tokenize_block(block, &page.layout).iter() {
        out.push_str(&normalize_text_key(&unicode::lower(&t.text)));
    }
    out
}

/// ref: classification/body_text.py:174 `_ROMAN_NUMERALS`
pub fn roman_value(s: &str) -> Option<i64> {
    const R: [&str; 20] = [
        "I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X", "XI", "XII", "XIII", "XIV",
        "XV", "XVI", "XVII", "XVIII", "XIX", "XX",
    ];
    R.iter().position(|&r| r == s).map(|i| i as i64 + 1)
}

// ref: classification/body_text.py::span_page_number
pub fn span_page_number(span: &Span) -> Option<i64> {
    if let Some(g) = page_number_only(&span.text) {
        let n = to_number(g);
        if !n.is_nan() && n > 0.0 && n < 1e6 && n == n.ceil() {
            return Some(n as i64);
        }
        return None;
    }
    roman_value(&unicode::upper(&span.text))
}

// ref: classification/body_text.py::longest_word_and_number
pub fn longest_word_and_number(block: &Block, page: &DocPage) -> Vec<String> {
    let mut word: Option<(String, usize)> = None;
    let mut number: Option<(String, usize)> = None;
    for t in tokenize_block(block, &page.layout).iter() {
        if t.kind == 2 {
            if word.as_ref().is_none_or(|w| t.len > w.1) {
                word = Some((t.text.clone(), t.len));
            }
        } else if t.kind == 1 && number.as_ref().is_none_or(|w| t.len > w.1) {
            number = Some((t.text.clone(), t.len));
        }
    }
    let mut out = Vec::new();
    if let Some((w, n)) = word
        && n > 3
    {
        out.push(normalize_text_key(&unicode::lower(&w)));
    }
    if let Some((num, _)) = number
        && !num.is_empty()
    {
        out.push(num);
    }
    out
}
