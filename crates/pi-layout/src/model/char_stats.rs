//! Character categories and per-run character statistics.
//!
//! ref: pageindex/flash/model/char_stats.py

use pi_pycompat::unicode::{self, Cat};

/// Tokenizer character categories (ref: model/char_stats.py:13-25).
pub mod cat {
    pub const EMPTY: u8 = 0;
    pub const NUMBER: u8 = 1;
    pub const UPPER: u8 = 2;
    pub const LOWER: u8 = 3;
    pub const OTHER_LETTER: u8 = 4;
    pub const MARK: u8 = 5;
    pub const SENTENCE_END: u8 = 6;
    pub const DASH: u8 = 7;
    pub const PUNCT: u8 = 8;
    pub const MATH: u8 = 9;
    pub const SPACE: u8 = 10;
    pub const OTHER: u8 = 11;
}

/// ref: model/char_stats.py:27 `_SENTENCE_END_CHARS`
pub const SENTENCE_END_CHARS: [char; 8] = [
    '.', '?', '!', '\u{FF61}', '\u{3002}', '\u{FF1F}', '\u{FF01}', '\u{FF0E}',
];
/// ref: model/char_stats.py:28 `_MINUS_SIGN_CHARS`
pub const MINUS_SIGN_CHARS: [char; 3] = ['\u{2212}', '\u{207B}', '\u{208B}'];

/// The package whitespace set (ref: model/char_stats.py:101 `_UNICODE_WHITESPACE_CHARS`):
/// WhiteSpace + LineTerminator + U+FEFF, *not* Python's `str.isspace` set.
pub fn is_unicode_ws(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

// ref: model/char_stats.py::_max_nan_propagating
#[inline]
pub fn max_nan(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::NAN;
    }
    if a >= b { a } else { b }
}

// ref: model/char_stats.py::_min_nan_propagating
#[inline]
pub fn min_nan(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::NAN;
    }
    if a <= b { a } else { b }
}

// ref: model/char_stats.py::char_category
pub fn char_category(c: char) -> u8 {
    let gc = unicode::category(c);
    match gc {
        Cat::Ll => return cat::LOWER,
        Cat::Lu | Cat::Lt => return cat::UPPER,
        Cat::Lo => return cat::OTHER_LETTER,
        _ => {}
    }
    if matches!(c, '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | '\u{FEFF}')
        || matches!(gc, Cat::Zs | Cat::Zl | Cat::Zp)
    {
        return cat::SPACE;
    }
    if SENTENCE_END_CHARS.contains(&c) {
        return cat::SENTENCE_END;
    }
    if matches!(gc, Cat::Pc | Cat::Pd) || MINUS_SIGN_CHARS.contains(&c) {
        return cat::DASH;
    }
    match gc.major() {
        'P' => cat::PUNCT,
        'N' => cat::NUMBER,
        'M' => cat::MARK,
        _ if gc == Cat::Sm => cat::MATH,
        _ => cat::OTHER,
    }
}

// ref: model/char_stats.py::_trim_unicode_ws
pub fn trim_unicode_ws(s: &str) -> &str {
    s.trim_matches(is_unicode_ws)
}

// ref: model/char_stats.py::_round_half_up_to_int
pub fn round_half_up_to_int(value: f64) -> f64 {
    let fl = value.floor();
    if value - fl < 0.5 { fl } else { fl + 1.0 }
}

/// ref: model/char_stats.py::CharStats
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CharStats {
    /// ref slot: `primary_slot`
    pub category_counts: [u32; 12],
    /// ref slot: `secondary_slot`
    pub first_cat: u8,
    /// ref slot: `tertiary_slot`
    pub last_cat: u8,
    /// ref slot: `auxiliary_slot`
    pub total_chars: u32,
}

impl CharStats {
    // ref: model/char_stats.py::CharStats.__init__
    pub fn new(text: &str) -> Self {
        let mut st = CharStats::default();
        for c in text.chars() {
            let k = char_category(c);
            if st.first_cat == 0 {
                st.first_cat = k;
            }
            st.last_cat = k;
            st.category_counts[k as usize] += 1;
            st.total_chars += 1;
        }
        st
    }
}

// ref: model/char_stats.py::merge_char_stats
pub fn merge_char_stats(a: &mut CharStats, b: &CharStats) {
    if a.first_cat == 0 {
        a.first_cat = b.first_cat;
    }
    if b.last_cat != 0 {
        a.last_cat = b.last_cat;
    }
    for i in 0..12 {
        a.category_counts[i] += b.category_counts[i];
    }
    a.total_chars += b.total_chars;
}

// ref: model/char_stats.py::letter_count
pub fn letter_count(cs: &CharStats) -> u32 {
    cs.category_counts[3] + cs.category_counts[2] + cs.category_counts[4]
}

// ref: model/char_stats.py::punct_count
pub fn punct_count(cs: &CharStats) -> u32 {
    cs.category_counts[6] + cs.category_counts[7] + cs.category_counts[8]
}

// ref: model/char_stats.py::info_weight
pub fn info_weight(cs: &CharStats) -> f64 {
    let c = &cs.category_counts;
    (c[3] as i64 + c[2] as i64 + 2 * c[4] as i64) as f64
        + 0.5 * (cs.total_chars as i64 - letter_count(cs) as i64) as f64
}

// ref: model/char_stats.py::is_upper_dominant
pub fn is_upper_dominant(cs: &CharStats) -> bool {
    let upper = cs.category_counts[2] as f64;
    let letters = letter_count(cs) as i64;
    let a = pi_pycompat::pymath::max((letters * 3) as f64 / 4.0, (letters - 4) as f64);
    let b = pi_pycompat::pymath::max(3.0, cs.total_chars as f64 / 3.0);
    upper > a && upper > b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories() {
        assert_eq!(char_category('a'), cat::LOWER);
        assert_eq!(char_category('Ǆ'), cat::UPPER);
        assert_eq!(char_category('中'), cat::OTHER_LETTER);
        assert_eq!(char_category('\u{1C}'), cat::OTHER);
        assert_eq!(char_category('\u{FEFF}'), cat::SPACE);
        assert_eq!(char_category('。'), cat::SENTENCE_END);
        assert_eq!(char_category('−'), cat::DASH);
        assert_eq!(char_category('²'), cat::NUMBER);
        assert_eq!(char_category('+'), cat::MATH);
    }

    #[test]
    fn stats_and_weight() {
        let cs = CharStats::new("Ab 1.");
        assert_eq!(cs.first_cat, cat::UPPER);
        assert_eq!(cs.last_cat, cat::SENTENCE_END);
        assert_eq!(cs.total_chars, 5);
        assert_eq!(info_weight(&cs), 2.0 + 0.5 * 3.0);
        assert_eq!(trim_unicode_ws("\u{1C} x \u{FEFF}"), "\u{1C} x");
    }
}
