//! Dictionary-backed keyword tries, keyword sets, and numbering tables.
//!
//! ref: pageindex/flash/heading_detection/keyword_tables.py

use std::collections::HashSet;
use std::sync::LazyLock;

use pi_layout::model::block::strip_diacritics;
use pi_layout::model::char_stats::is_unicode_ws;
use pi_layout::model::to_number;
use pi_layout::tokens::Trie;
use pi_pycompat::unicode;

static DICTS: LazyLock<serde_json::Value> =
    LazyLock::new(|| serde_json::from_str(pi_data::DICTIONARIES_JSON).expect("dictionaries.json"));

/// `_DICTS.get(key, [])` as owned strings.
fn dict(key: &str) -> Vec<String> {
    DICTS
        .get(key)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn folded(key: &str) -> Trie {
    Trie::build(dict(key), true, false)
}

/// ref: heading_detection/keyword_tables.py:31
pub static SECTION_KEYWORDS_TRIE: LazyLock<Trie> = LazyLock::new(|| folded("section_keywords"));
/// ref: heading_detection/keyword_tables.py:32
pub static ABSTRACT_KEYWORDS_TRIE: LazyLock<Trie> = LazyLock::new(|| folded("abstract_keywords"));
/// ref: heading_detection/keyword_tables.py:33
pub static REFERENCES_TRIE: LazyLock<Trie> = LazyLock::new(|| folded("references"));
/// ref: heading_detection/keyword_tables.py:34
pub static APPENDIX_SECTION_TRIE: LazyLock<Trie> = LazyLock::new(|| folded("appendices_dict"));
/// ref: heading_detection/keyword_tables.py:35
pub static INTRODUCTION_SECTION_TRIE: LazyLock<Trie> =
    LazyLock::new(|| folded("introduction_dict"));
/// ref: heading_detection/keyword_tables.py:36
pub static BOX_KEYWORD_TRIE: LazyLock<Trie> = LazyLock::new(|| Trie::build(["box"], true, false));
/// ref: heading_detection/keyword_tables.py:37
pub static KEYWORDS_SECTION_TRIE: LazyLock<Trie> = LazyLock::new(|| folded("keywords_dict"));
/// ref: heading_detection/keyword_tables.py:38
pub static CHAPTER_WORDS_TRIE: LazyLock<Trie> = LazyLock::new(|| folded("chapter_words"));
/// ref: heading_detection/keyword_tables.py:39
pub static APPENDIX_KEYWORDS_TRIE: LazyLock<Trie> = LazyLock::new(|| folded("appendix_keywords"));
/// ref: heading_detection/keyword_tables.py:64
pub static EQUATION_KEYWORDS_TRIE: LazyLock<Trie> = LazyLock::new(|| {
    Trie::build(
        [
            "equation",
            "equation.",
            "eqn",
            "eqn.",
            "eq",
            "eq.",
            "ecuación",
            "equação",
            "gleichung",
            "equazione",
            "ekvation",
            "yhtälö",
            "ligning",
            "persamaan",
            "denklem",
            "ecuația",
            "equació",
            "rovnica",
            "rovnice",
            "równanie",
            "vergelijking",
            "jednadžba",
            "jöfnu",
            "võrrand",
            "vienādojums",
            "lygtis",
            "enačba",
            "egyenlet",
            "phương trình",
            "εξίσωση",
            "方程",
            "방정식",
            "уравнение",
            "рівняння",
            "раўнанне",
            "једначина",
        ],
        true,
        false,
    )
});
/// English section keywords. ref: outline/filtering.py:17 `SECTION_KEYWORD_TRIE`
pub static SECTION_KEYWORD_TRIE: LazyLock<Trie> = LazyLock::new(|| {
    Trie::build(
        [
            "acknowledgements",
            "acknowledgments",
            "background",
            "conclusion",
            "conclusions",
            "discussion",
            "introduction",
            "materials and methods",
            "method",
            "methods",
            "results",
        ],
        true,
        false,
    )
});

/// ref: heading_detection/keyword_tables.py::_normalize_text_key
pub fn normalize_text_key(text: &str) -> String {
    strip_diacritics(text)
}

/// ref: heading_detection/keyword_tables.py:49 `ABSTRACT_KEYWORDS_SET`
pub static ABSTRACT_KEYWORDS_SET: LazyLock<HashSet<String>> = LazyLock::new(|| {
    dict("abstract_keywords")
        .iter()
        .map(|t| strip_diacritics(&unicode::lower(t)))
        .collect()
});

/// ref: heading_detection/keyword_tables.py:50 `REFERENCES_SET`
pub static REFERENCES_SET: LazyLock<HashSet<String>> = LazyLock::new(|| {
    dict("references")
        .iter()
        .map(|t| unicode::lower(t))
        .collect()
});

/// `NUMBERED_PREFIX_RE.match(text)` →
/// `^([1-9１-９]\p{Number}*)[ .-](?:[<ws>]|\p{Lu})`; returns group 1.
/// Hand-written matcher: `\p{Number}` follows the `regex` module table
/// (`unicode::is_regex_number`), `\p{Lu}` the Unicode 14 general category.
// ref: heading_detection/keyword_tables.py:56 `NUMBERED_PREFIX_RE`
pub fn numbered_prefix_match(text: &str) -> Option<&str> {
    let mut it = text.char_indices();
    let (_, c0) = it.next()?;
    if !(('1'..='9').contains(&c0) || ('\u{FF11}'..='\u{FF19}').contains(&c0)) {
        return None;
    }
    let mut end = c0.len_utf8();
    let mut rest = text[end..].chars();
    // `\p{Number}*` is greedy; the separator class `[ .-]` holds no numbers, so no backtracking
    // can change the outcome.
    let mut sep = None;
    for c in rest.by_ref() {
        if unicode::is_regex_number(c) {
            end += c.len_utf8();
            continue;
        }
        sep = Some(c);
        break;
    }
    let sep = sep?;
    if !matches!(sep, ' ' | '.' | '-') {
        return None;
    }
    let next = rest.next()?;
    if is_unicode_ws(next) || unicode::category(next).as_str() == "Lu" {
        Some(&text[..end])
    } else {
        None
    }
}

/// `DEAD_DIGIT_RE = re.compile(r"^.p\{Number\}+.$")` under stdlib `re`: the literal text
/// `p{Number}` repeated, between two arbitrary non-newline characters, `$` allowing a final `\n`.
// ref: heading_detection/keyword_tables.py:60 `DEAD_DIGIT_RE`
pub fn dead_digit_match(text: &str) -> bool {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let chars: Vec<char> = body.chars().collect();
    if chars.len() < 2 + 9 || chars[0] == '\n' || chars[chars.len() - 1] == '\n' {
        return false;
    }
    let mid: String = chars[1..chars.len() - 1].iter().collect();
    // `p\{Number\}+` = "p{Number" followed by one or more "}".
    let Some(tail) = mid.strip_prefix("p{Number") else {
        return false;
    };
    !tail.is_empty() && tail.chars().all(|c| c == '}')
}

/// ref: heading_detection/keyword_tables.py:79 `ENGLISH_WORD_TO_NUMBER`
pub fn english_word_to_number(s: &str) -> Option<i64> {
    const W: [&str; 20] = [
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
    ];
    W.iter().position(|w| *w == s).map(|i| i as i64 + 1)
}

/// ref: heading_detection/keyword_tables.py:85 `ROMAN_NUMERAL_MAP`
pub fn roman_numeral(s: &str) -> Option<i64> {
    const R: [&str; 20] = [
        "I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X", "XI", "XII", "XIII", "XIV",
        "XV", "XVI", "XVII", "XVIII", "XIX", "XX",
    ];
    R.iter().position(|w| *w == s).map(|i| i as i64 + 1)
}

/// ref: heading_detection/keyword_tables.py:93 `FORMULA_CHAR_WEIGHTS`
pub fn formula_char_weight(s: &str) -> Option<f64> {
    Some(match s {
        "=" => 10.0,
        "{" | "}" | "+" => 5.0,
        "/" | "*" => 3.0,
        "-" | "~" | "[" | "]" | "(" | ")" => 1.0,
        _ => return None,
    })
}

/// `to_number` re-export for the numbered-prefix group.
pub fn prefix_number(group: &str) -> f64 {
    to_number(group)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_prefix() {
        assert_eq!(numbered_prefix_match("2. Foo"), Some("2"));
        assert_eq!(numbered_prefix_match("12 Bar"), Some("12"));
        assert_eq!(numbered_prefix_match("3-X"), Some("3"));
        assert_eq!(numbered_prefix_match("0. Foo"), None);
        assert_eq!(numbered_prefix_match("2.x"), None);
        assert_eq!(numbered_prefix_match("2."), None);
        assert_eq!(
            numbered_prefix_match("２\u{00B2}.\u{3000}"),
            Some("２\u{00B2}")
        );
    }

    #[test]
    fn dead_digit() {
        assert!(dead_digit_match("xp{Number}y"));
        assert!(dead_digit_match("xp{Number}}}y\n"));
        assert!(!dead_digit_match("(12)"));
    }

    #[test]
    fn tables() {
        assert_eq!(roman_numeral("XIV"), Some(14));
        assert_eq!(english_word_to_number("twenty"), Some(20));
        assert!(REFERENCES_SET.contains("references"));
        assert!(!SECTION_KEYWORD_TRIE.is_reverse());
    }
}
