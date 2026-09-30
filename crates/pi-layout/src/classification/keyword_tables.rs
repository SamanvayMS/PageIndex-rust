//! Dictionary-backed keyword tries and shared text patterns.
//!
//! ref: pageindex/flash/classification/keyword_tables.py

use std::sync::LazyLock;

use pi_pycompat::unicode::is_regex_number;

use crate::blocks::join_rules::{dict_list, dict_trie};
use crate::model::block::strip_diacritics;
use crate::model::char_stats::is_unicode_ws;
use crate::tokens::{TokenView, Trie};

macro_rules! trie {
    ($name:ident, $doc:literal, $init:expr) => {
        #[doc = $doc]
        pub static $name: LazyLock<Trie> = LazyLock::new(|| $init);
    };
}

trie!(
    COPYRIGHT_TRIE,
    "ref: classification/keyword_tables.py:75",
    Trie::build(["Copyright", "\u{A9}"], true, false)
);
trie!(
    VOLUME_WORDS_TRIE,
    "ref: classification/keyword_tables.py:76",
    dict_trie(&["volume_words"])
);
trie!(
    TOC_TITLES_TRIE,
    "ref: classification/keyword_tables.py:77",
    dict_trie(&["toc_titles"])
);
trie!(
    FIGURE_KEYWORDS_TRIE,
    "ref: classification/keyword_tables.py:78",
    dict_trie(&["ai_section_keywords"])
);
trie!(
    TABLE_KEYWORDS_TRIE,
    "ref: classification/keyword_tables.py:79",
    dict_trie(&["table_keywords"])
);
trie!(
    CHART_KEYWORDS_TRIE,
    "ref: classification/keyword_tables.py:82",
    dict_trie(&["chart_keywords"])
);
trie!(
    APPENDIX_SECTION_TRIE,
    "ref: classification/keyword_tables.py:84",
    dict_trie(&["appendices_dict"])
);
trie!(
    INTRODUCTION_SECTION_TRIE,
    "ref: classification/keyword_tables.py:85",
    dict_trie(&["introduction_dict"])
);
trie!(
    BOX_KEYWORD_TRIE,
    "ref: classification/keyword_tables.py:86",
    Trie::build(["box"], true, false)
);
trie!(
    KEYWORDS_SECTION_TRIE,
    "ref: classification/keyword_tables.py:87",
    dict_trie(&["keywords_dict"])
);
trie!(
    BOILERPLATE_TRIE,
    "ref: classification/keyword_tables.py:94",
    Trie::build(
        serde_json::from_str::<Vec<String>>(pi_data::BOILERPLATE_PHRASES_JSON)
            .expect("boilerplate phrases"),
        true,
        false
    )
);
trie!(
    INSTITUTION_THESIS_TRIE,
    "ref: classification/toc_boilerplate.py:37",
    dict_trie(&["institution_words", "nk_thesis_words"])
);
trie!(
    PROFESSOR_TITLES_TRIE,
    "ref: classification/toc_boilerplate.py:38",
    dict_trie(&["professor_titles"])
);

/// Reference only the list (for sets built from it).
pub fn dict_strings(key: &str) -> Vec<String> {
    dict_list(key)
}

fn is_lead_digit(c: char) -> bool {
    matches!(c, '1'..='9' | '\u{FF11}'..='\u{FF19}')
}

/// `DOT_LEADER_ROW_RE.search(s)`:
/// `([.][WS]*){5,}[WS]*[1-9１-９]\p{Number}*\Z` (ref: classification/keyword_tables.py:101).
/// The digit class and `\p{Number}` must cover the maximal trailing run of number chars (a
/// number char cannot match anything before it), and what precedes it must end in a run of
/// dots/whitespace holding at least five dots.
pub fn dot_leader_row(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    let mut i = chars.len();
    while i > 0 && is_regex_number(chars[i - 1]) {
        i -= 1;
    }
    if i == chars.len() || !is_lead_digit(chars[i]) {
        return false;
    }
    let mut dots = 0;
    let mut j = i;
    while j > 0 && (chars[j - 1] == '.' || is_unicode_ws(chars[j - 1])) {
        if chars[j - 1] == '.' {
            dots += 1;
        }
        j -= 1;
    }
    dots >= 5
}

/// `PAGE_NUMBER_ONLY_RE.match(s)` -> group 1:
/// `^[ |]*([1-9１-９]\p{Number}*)[ |]*\Z` (ref: classification/keyword_tables.py:102).
pub fn page_number_only(s: &str) -> Option<&str> {
    let t = s.trim_start_matches([' ', '|']);
    let first = t.chars().next()?;
    if !is_lead_digit(first) {
        return None;
    }
    let mut end = first.len_utf8();
    for c in t[end..].chars() {
        if !is_regex_number(c) {
            break;
        }
        end += c.len_utf8();
    }
    if t[end..].chars().all(|c| c == ' ' || c == '|') {
        Some(&t[..end])
    } else {
        None
    }
}

/// Diacritic stripping only; callers lowercase first.
// ref: classification/keyword_tables.py::_normalize_text_key
pub fn normalize_text_key(text: &str) -> String {
    strip_diacritics(text)
}

// ref: classification/keyword_tables.py::_search_trie
pub fn search_trie(trie: &Trie, tokens: &TokenView) -> Option<TokenView> {
    trie.search(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        assert!(dot_leader_row("Intro . . . . . 12"));
        assert!(dot_leader_row("Intro.....１２"));
        assert!(!dot_leader_row("Intro.... 12"));
        assert!(!dot_leader_row("Intro..... 012"));
        assert_eq!(page_number_only(" | 12 |"), Some("12"));
        assert_eq!(page_number_only("0"), None);
        assert_eq!(page_number_only("12a"), None);
    }
}
