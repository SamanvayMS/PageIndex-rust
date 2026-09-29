//! Python `str` semantics that differ from Rust's.

/// Code points for which Python's `str.isspace()` is true (Unicode 14 / Python 3.11):
/// bidi class WS/B/S or category Zs. Differs from Rust's `char::is_whitespace` by including
/// U+001C..U+001F and excluding nothing else relevant.
pub fn is_py_space(c: char) -> bool {
    matches!(c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | '\u{1C}'..='\u{1F}' | ' ' | '\u{85}' | '\u{A0}'
        | '\u{1680}' | '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}'
        | '\u{3000}')
}

/// `s.split()` (no argument).
pub fn split(s: &str) -> Vec<&str> {
    s.split(is_py_space).filter(|p| !p.is_empty()).collect()
}

/// `s.strip()` (no argument).
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_py_space)
}
