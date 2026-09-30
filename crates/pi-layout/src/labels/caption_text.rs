//! Caption label text helpers and structural-number parsing.
//!
//! ref: pageindex/flash/labels/caption_text.py

use std::sync::LazyLock;

use pi_pycompat::unicode::is_regex_number;

use crate::clustering::LineId;
use crate::model::char_stats::{char_category, trim_unicode_ws};
use crate::tokens::{Token, TokenView, Trie, is_word_token};

/// ref: labels/caption_text.py:20 `PERIOD_CHARS`
pub const PERIOD_CHARS: [&str; 4] = [".", "\u{FF0E}", "\u{FF61}", "\u{3002}"];

/// `STRUCTURAL_NUMBER_RE.match(s)`:
/// `^(?:[A-M]*\p{Number}+[A-Ma-m]?|[A-Ma-m]\p{Number}*|[IVX]+)\Z` (ref: labels/caption_text.py:26).
/// The classes of consecutive items are disjoint, so greedy matching needs no backtracking.
pub fn is_structural_number(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    // Alt 1.
    let mut i = 0;
    while i < chars.len() && ('A'..='M').contains(&chars[i]) {
        i += 1;
    }
    let digits_start = i;
    while i < chars.len() && is_regex_number(chars[i]) {
        i += 1;
    }
    if i > digits_start {
        let rest = &chars[i..];
        if rest.is_empty() || (rest.len() == 1 && matches!(rest[0], 'A'..='M' | 'a'..='m')) {
            return true;
        }
    }
    // Alt 2.
    if let Some(&c0) = chars.first()
        && matches!(c0, 'A'..='M' | 'a'..='m')
        && chars[1..].iter().all(|&c| is_regex_number(c))
    {
        return true;
    }
    // Alt 3.
    !chars.is_empty() && chars.iter().all(|c| matches!(c, 'I' | 'V' | 'X'))
}

// ref: labels/caption_text.py::is_number_separator
pub fn is_number_separator(t: Option<&Token>, allow_symbol: bool) -> bool {
    match t {
        None => false,
        Some(t) if t.boundary => false,
        Some(t) => t.kind == 3 || (allow_symbol && t.kind == 4),
    }
}

// ref: labels/caption_text.py::extract_structural_number
pub fn extract_structural_number(tokens: &TokenView, allow_symbol: bool) -> Option<TokenView> {
    if tokens.length < 1 {
        return None;
    }
    let mut cur = tokens.clone();
    let first = tokens.token_at(0)?;
    if first.len == 1 && matches!(first.text.chars().next(), Some('A'..='H')) {
        if !is_number_separator(tokens.token_at(1), allow_symbol) {
            return None;
        }
        cur = tokens.from(2);
    }
    if cur.length < 1 {
        return None;
    }
    let head = cur.first()?;
    if !is_structural_number(&head.text) {
        return None;
    }
    cur = cur.from(1);
    while cur.length >= 2
        && is_number_separator(cur.token_at(0), allow_symbol)
        && is_structural_number(&cur.token_at(1).expect("len >= 2").text)
    {
        cur = cur.from(2);
    }
    Some(tokens.slice(0, tokens.length - cur.length))
}

// ref: labels/caption_text.py::format_caption_label
pub fn format_caption_label(kind: u8, num: Option<&TokenView>) -> String {
    let mut letter = match kind {
        4 => "F".to_string(),
        5 => "T".to_string(),
        11 => "Q".to_string(),
        _ => return String::new(),
    };
    if let Some(n) = num {
        letter.push_str(trim_unicode_ws(&n.to_string_py()));
    }
    letter
}

/// ref: labels/caption_text.py:79 `REFERENCE_PHRASE_TRIE` (case-sensitive)
pub static REFERENCE_PHRASE_TRIE: LazyLock<Trie> = LazyLock::new(|| {
    Trie::build(
        ["lists the", "presents", "show the", "showed the", "shows"],
        false,
        false,
    )
});

// ref: labels/caption_text.py::is_uppercase_dominant
pub fn is_uppercase_dominant(tokens: &TokenView) -> bool {
    let (mut upper, mut lower) = (0i64, 0i64);
    for t in tokens.iter() {
        if t.kind != 2 {
            continue;
        }
        if t.first_cat == 2 {
            upper += 1;
        } else if t.first_cat == 3 {
            // (`len > 4 and len >= 2` in the reference)
            if t.len > 4 && char_category(t.text.chars().nth(1).expect("len >= 2")) != 2 {
                return false;
            }
            lower += 1;
        }
    }
    upper > 2.max(lower)
}

// ref: labels/caption_text.py::trie_matches_all
pub fn trie_matches_all(trie: &Trie, tokens: &TokenView) -> bool {
    let Some(m) = trie.prefix_match(tokens) else {
        return false;
    };
    if m.length == tokens.length {
        return true;
    }
    if m.length == tokens.length - 1 {
        return tokens.last().is_some_and(is_word_token);
    }
    false
}

// ref: labels/caption_text.py::advance_past_line
pub fn advance_past_line(tokens: &TokenView, line: LineId, mut index: i64) -> i64 {
    while index < tokens.length {
        match tokens.token_at(index) {
            Some(t) if t.line() == Some(line) => index += 1,
            _ => break,
        }
    }
    index
}

// ref: labels/caption_text.py::skip_bracketed_word
pub fn skip_bracketed_word(tokens: &TokenView, index: i64) -> i64 {
    match tokens.token_at(index) {
        Some(t) if t.boundary && is_word_token(t) => index + 1,
        _ => index,
    }
}

// ref: labels/caption_text.py::token_case_signal
pub fn token_case_signal(t: Option<&Token>) -> i32 {
    match t.map(|t| t.first_cat) {
        Some(7) | Some(6) => 2,
        Some(2) => 1,
        Some(3) => -1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_numbers() {
        for s in ["1", "12", "A1", "A1b", "a", "b12", "IV", "٣", "AB12"] {
            assert!(is_structural_number(s), "{s}");
        }
        for s in ["", "1ab", "N1", "iv", "1-2", "Z"] {
            assert!(!is_structural_number(s), "{s}");
        }
    }
}
