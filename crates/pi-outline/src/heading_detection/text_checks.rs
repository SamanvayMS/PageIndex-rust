//! Block-text predicates: keyword matches, continuation, content, and number parsing.
//!
//! ref: pageindex/flash/heading_detection/text_checks.py

use pi_layout::labels::caption_text::{extract_structural_number, trie_matches_all};
use pi_layout::model::block::{Block, block_text};
use pi_layout::model::char_stats::{is_upper_dominant, trim_unicode_ws};
use pi_layout::model::rects::{
    Bounded, center_aligned, left_aligned, right_aligned, x_aligned, y_overlaps,
};
use pi_layout::model::to_number;
use pi_layout::phases::{DocPage, Document};
use pi_layout::tokens::{
    Token, TokenView, is_word_token, strip_trie_match, token_numeric_value, tokenize_block,
};
use pi_pycompat::unicode;

use super::keyword_tables::{
    ABSTRACT_KEYWORDS_SET, ABSTRACT_KEYWORDS_TRIE, EQUATION_KEYWORDS_TRIE, REFERENCES_SET,
    REFERENCES_TRIE, dead_digit_match, english_word_to_number, formula_char_weight,
    normalize_text_key, numbered_prefix_match, roman_numeral,
};
use crate::model::{Num, clamp};

/// Tokenized text joined with the tokens' spacing flags, trimmed.
// ref: heading_detection/text_checks.py::token_text_of_block
pub fn token_text_of_block(b: &Block, page: &DocPage) -> String {
    trim_unicode_ws(&tokenize_block(b, &page.layout).to_string_py()).to_string()
}

// ref: heading_detection/text_checks.py::similar_style
pub fn similar_style(a: &Block, b: &Block) -> bool {
    (a.bold_frac - b.bold_frac).abs() < 0.5
        && (a.weighted_font_size - b.weighted_font_size).abs() < 1.0
}

/// Whether `block` is the next numbered heading after `other` (numbers differ by one).
// ref: heading_detection/text_checks.py::is_heading_continuation
pub fn is_heading_continuation(page: &DocPage, block: &Block, other: &Block, n: Num) -> bool {
    if block.kind.get() != 0 || block.char_count() >= 500 || !similar_style(other, block) {
        return false;
    }
    let text = token_text_of_block(block, page);
    if block.left_edge() >= other.left_edge() && text.starts_with('•') {
        return true;
    }
    if is_upper_dominant(&other.char_stats)
        && is_upper_dominant(&block.char_stats)
        && !left_aligned(other, block, 1.0)
        && !right_aligned(other, block, 1.0)
        && center_aligned(other, block, 1.0)
    {
        return false;
    }
    match numbered_prefix_match(&text) {
        Some(g) => (n - to_number(g)).abs() == 1.0,
        None => false,
    }
}

// ref: heading_detection/text_checks.py::matches_abstract
pub fn matches_abstract(tokens: &TokenView) -> bool {
    if trie_matches_all(&ABSTRACT_KEYWORDS_TRIE, tokens) {
        return true;
    }
    if tokens.length > 10 {
        return false;
    }
    let mut normalized = String::new();
    let mut nlen = 0usize;
    for t in tokens.iter() {
        if is_word_token(t) {
            continue;
        }
        if t.kind != 2 || nlen + t.len > 20 {
            return false;
        }
        let piece = normalize_text_key(&unicode::lower(&t.text));
        nlen += piece.chars().count();
        normalized.push_str(&piece);
    }
    ABSTRACT_KEYWORDS_SET.contains(&normalized)
}

// ref: heading_detection/text_checks.py::matches_references
pub fn matches_references(tokens: &TokenView) -> bool {
    let Some(m) = REFERENCES_TRIE.prefix_match(tokens) else {
        if tokens.length <= 15 {
            let mut normalized = String::new();
            let mut nlen = 0usize;
            for t in tokens.iter() {
                if is_word_token(t) {
                    continue;
                }
                if t.kind != 2 || nlen + t.len > 20 {
                    return false;
                }
                let piece = unicode::lower(&t.text);
                nlen += piece.chars().count();
                normalized.push_str(&piece);
            }
            return REFERENCES_SET.contains(&normalized);
        }
        return false;
    };
    if m.length == tokens.length {
        return true;
    }
    let rest = tokens.from(m.length);
    if rest.length == 1 && rest.token_at(0).is_some_and(is_word_token) {
        return true;
    }
    trie_matches_all(&REFERENCES_TRIE, &rest)
}

// ref: heading_detection/text_checks.py::vertically_close
pub fn vertically_close(a: Option<&Block>, b: &Block) -> bool {
    let Some(a) = a else { return false };
    let d = if a.top_edge() > b.top_edge() {
        a.bottom_edge() - b.top_edge()
    } else {
        b.bottom_edge() - a.top_edge()
    };
    d < 2.0 * b.weighted_font_size || (x_aligned(a, b, 1.0) && d < 5.0 * b.weighted_font_size)
}

/// Whether a (one-line) block next to `block` looks like an equation number.
// ref: heading_detection/text_checks.py::is_equation_adjacent_line
pub fn is_equation_adjacent_line(page: &DocPage, line: Option<&Block>, block: &Block) -> bool {
    let Some(line) = line else { return false };
    if line.line_count() != 1 {
        return false;
    }
    if line.left_edge() < block.right_edge() || !y_overlaps(block, line) {
        return false;
    }
    if dead_digit_match(block_text(line, &page.layout)) {
        return true;
    }
    let mut tokens = tokenize_block(line, &page.layout);
    if tokens.length >= 3 {
        let f = tokens.first().expect("len >= 3");
        let l = tokens.last().expect("len >= 3");
        if f.len <= 1 && f.kind != 1 && l.len <= 1 && l.kind != 1 {
            tokens = tokens.slice(1, tokens.length - 1);
        }
    }
    let tokens = strip_trie_match(&tokens, &EQUATION_KEYWORDS_TRIE);
    extract_structural_number(&tokens, true).is_some_and(|m| m.length == tokens.length)
}

/// Heuristic "formula-like content" score >= 5.
// ref: heading_detection/text_checks.py::has_substantive_content
pub fn has_substantive_content(
    page: &DocPage,
    block: &Block,
    prev: Option<&Block>,
    next: Option<&Block>,
) -> bool {
    let lay = &page.layout;
    let mut score = 0.0f64;
    for t in tokenize_block(block, lay).iter() {
        let anchor = &lay.spans[t.first_anchor_span().expect("token anchor span")];
        let line = &lay.lines[t.line().expect("token line")];
        let size = line.max_span_height;
        let flag = anchor.top_edge() < line.bottom_edge() + 0.8 * size
            || anchor.bottom_edge() > line.top_edge() - 0.8 * size;
        let m = if flag { 3.0 } else { 1.0 };
        if t.kind == 1 {
            score += if flag { 2.0 } else { 1.0 };
            continue;
        }
        if let Some(w) = formula_char_weight(&t.text) {
            score += m * w;
            continue;
        }
        if t.kind == 6 {
            score += m * 5.0;
            continue;
        }
        if t.len <= 3 && t.first_cat != 4 {
            if flag {
                score += if is_word_token(t) { 5.0 } else { 1.0 };
            }
            continue;
        }
        if flag {
            continue;
        }
        let len_value = (if anchor.bold { 2.0 } else { 1.0 }) * t.len as f64;
        match t.first_cat {
            4 => score -= 2.0 * len_value,
            2 => score -= len_value,
            3 => score -= 0.5 * len_value,
            _ => {}
        }
    }
    if score < 0.0 {
        return false;
    }
    if score >= 5.0 {
        return true;
    }
    is_equation_adjacent_line(page, prev, block) || is_equation_adjacent_line(page, next, block)
}

/// Title-marked early page with light content or no body text.
// ref: heading_detection/text_checks.py::is_cover_page
pub fn is_cover_page(doc: &Document, page: &DocPage) -> bool {
    page.title_or_refs.get()
        && (page.index() as f64) < pi_pycompat::pymath::max(2.0, doc.pages.len() as f64 / 2.0)
        && (page.layout.stats.total_line_weight
            < clamp(0.5 * doc.stats.median_page_weight, 200.0, 1000.0)
            || !page.has_body.get())
}

/// Numeric value of a token: digits, Roman numeral or English number word.
// ref: heading_detection/text_checks.py::token_to_number
pub fn token_to_number(t: Option<&Token>) -> Option<Num> {
    let t = t?;
    if t.kind == 1 {
        let v = token_numeric_value(t);
        return (!v.is_nan() && v > 0.0).then_some(v);
    }
    // `ROMAN.get(s) or ENGLISH.get(s.lower())`: both tables hold only positive values.
    roman_numeral(&t.text)
        .or_else(|| english_word_to_number(&unicode::lower(&t.text)))
        .map(|v| v as f64)
}

/// 'a'/'A' -> 1 ... 'h' -> 8, on the first UTF-16 unit of the lowercased character.
// ref: heading_detection/text_checks.py::letter_to_ordinal
pub fn letter_to_ordinal(s: &str) -> Option<i64> {
    let mut it = s.chars();
    let c = it.next()?;
    if it.next().is_some() {
        return None;
    }
    let low = unicode::lower(&c.to_string());
    let mut unit = low.chars().next()? as i64;
    if unit > 0xFFFF {
        unit = 0xD800 + ((unit - 0x10000) >> 10);
    }
    let v = unit - 96;
    (1..=8).contains(&v).then_some(v)
}
