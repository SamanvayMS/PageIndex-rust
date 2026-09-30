//! Numbering-prefix detection and numeric parsing.
//!
//! ref: pageindex/flash/model/numbering.py

use pi_pycompat::unicode;

use super::char_stats::{is_unicode_ws, trim_unicode_ws};
use super::rects::Bounded;
use super::span_line::{Line, Span, raw_text_of_line};

/// Result of `_NUMBERING_PREFIX_RE.match`: the three capture groups.
#[derive(Debug, Default, PartialEq)]
pub struct NumberingMatch<'a> {
    pub g1: Option<&'a str>,
    pub g2: Option<&'a str>,
    pub g3: Option<&'a str>,
}

fn is_prefix_dot(c: char) -> bool {
    // [.．｡。)] and, for group 1, also ':'.
    matches!(c, '.' | '\u{FF0E}' | '\u{FF61}' | '\u{3002}' | ')')
}

/// Python `$` (no MULTILINE): at the end, or before a final `\n`.
fn at_dollar(rest: &str) -> bool {
    rest.is_empty() || rest == "\n"
}

/// `_NUMBERING_PREFIX_RE.match(text)` (ref: model/numbering.py:31), compiled with the `regex`
/// module:
///
/// ```text
/// ^(?:([IVX]+|[1-9１-９]\p{Number}?)(?:[.．｡。):]|-\P{Number}|-$|[WS]|$)
///   |(?:([A-Ha-h])|([ivx]))[.．｡。)])
/// ```
///
/// Hand-matched so `\p{Number}` uses the exact `regex`-module table. Backtracking cannot change
/// the result: shortening `[IVX]+` or dropping the optional `\p{Number}` leaves a roman letter or
/// a number as the next character, which the follow set never accepts.
pub fn match_numbering_prefix(text: &str) -> Option<NumberingMatch<'_>> {
    let mut chars = text.char_indices();
    let (_, c0) = chars.next()?;
    let g1_end = if matches!(c0, 'I' | 'V' | 'X') {
        let end = text
            .char_indices()
            .find(|&(_, c)| !matches!(c, 'I' | 'V' | 'X'))
            .map_or(text.len(), |(i, _)| i);
        Some(end)
    } else if matches!(c0, '1'..='9' | '\u{FF11}'..='\u{FF19}') {
        let mut end = c0.len_utf8();
        if let Some(c1) = text[end..].chars().next()
            && unicode::is_regex_number(c1)
        {
            end += c1.len_utf8();
        }
        Some(end)
    } else {
        None
    };
    if let Some(end) = g1_end {
        let rest = &text[end..];
        let mut it = rest.chars();
        let follow_ok = match it.next() {
            None => true,
            Some(c) if is_prefix_dot(c) || c == ':' => true,
            Some('-') => {
                let after = &rest[1..];
                match after.chars().next() {
                    Some(d) if !unicode::is_regex_number(d) => true,
                    _ => at_dollar(after),
                }
            }
            Some(c) if is_unicode_ws(c) => true,
            Some(_) => at_dollar(rest),
        };
        if follow_ok {
            return Some(NumberingMatch {
                g1: Some(&text[..end]),
                ..Default::default()
            });
        }
    }
    // Second alternative.
    let l = c0.len_utf8();
    let next = text[l..].chars().next();
    if next.is_some_and(is_prefix_dot) {
        if matches!(c0, 'A'..='H' | 'a'..='h') {
            return Some(NumberingMatch {
                g2: Some(&text[..l]),
                ..Default::default()
            });
        }
        if matches!(c0, 'i' | 'v' | 'x') {
            return Some(NumberingMatch {
                g3: Some(&text[..l]),
                ..Default::default()
            });
        }
    }
    None
}

/// `_BRACKETED_NUM_RE.match(text)`: `^[\[\(] *([1-9][0-9]?) *[\)\]]` (ref: model/numbering.py:39).
pub fn match_bracketed_num(text: &str) -> Option<&str> {
    let b = text.as_bytes();
    if !matches!(b.first(), Some(b'[' | b'(')) {
        return None;
    }
    let mut i = 1;
    while i < b.len() && b[i] == b' ' {
        i += 1;
    }
    let start = i;
    if !(i < b.len() && (b'1'..=b'9').contains(&b[i])) {
        return None;
    }
    let close = |mut j: usize| -> bool {
        while j < b.len() && b[j] == b' ' {
            j += 1;
        }
        j < b.len() && matches!(b[j], b')' | b']')
    };
    // Greedy `[0-9]?` first, then without it.
    if start + 1 < b.len() && b[start + 1].is_ascii_digit() && close(start + 2) {
        return Some(&text[start..start + 2]);
    }
    if close(start + 1) {
        return Some(&text[start..start + 1]);
    }
    None
}

fn is_ascii_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `^[+-]?(?:[0-9]+\.?[0-9]*|\.[0-9]+)(?:[eE][+-]?[0-9]+)?$` (ref: model/numbering.py:46).
fn is_decimal_literal(s: &str) -> bool {
    let s = s.strip_prefix(['+', '-']).unwrap_or(s);
    let (mant, exp) = match s.find(['e', 'E']) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    if let Some(e) = exp {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        if !is_ascii_digits(e) {
            return false;
        }
    }
    match mant.find('.') {
        None => is_ascii_digits(mant),
        Some(i) => {
            let (int, frac) = (&mant[..i], &mant[i + 1..]);
            let frac_ok = frac.is_empty() || is_ascii_digits(frac);
            if int.is_empty() {
                !frac.is_empty() && frac_ok
            } else {
                is_ascii_digits(int) && frac_ok
            }
        }
    }
}

/// `float(int(digits, radix))` with Python's correctly rounded int -> float conversion.
fn radix_to_f64(digits: &str, radix: u32) -> f64 {
    let mut acc: u128 = 0;
    for c in digits.chars() {
        let d = c.to_digit(radix).unwrap() as u128;
        match acc
            .checked_mul(radix as u128)
            .and_then(|v| v.checked_add(d))
        {
            Some(v) => acc = v,
            None => {
                // Beyond u128 (> 38 decimal digits): float accumulation, not correctly
                // rounded. Not reachable from layout text in practice.
                return digits.chars().fold(0.0, |f, c2| {
                    f * radix as f64 + c2.to_digit(radix).unwrap() as f64
                });
            }
        }
    }
    acc as f64
}

// ref: model/numbering.py::to_number
pub fn to_number(text: &str) -> f64 {
    let normalized = unicode::nfkc(text);
    let t = trim_unicode_ws(&normalized);
    if t.is_empty() {
        return 0.0;
    }
    // Every grammar below is `$`-anchored; the value is trimmed, so no trailing "\n" remains.
    let unsigned = t.strip_prefix(['+', '-']).unwrap_or(t);
    if unsigned == "Infinity" {
        return if t.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    let b = t.as_bytes();
    if b.len() > 2 && b[0] == b'0' {
        let body = &t[2..];
        match b[1] {
            b'x' | b'X' if body.bytes().all(|c| c.is_ascii_hexdigit()) => {
                return radix_to_f64(body, 16);
            }
            b'o' | b'O' if body.bytes().all(|c| (b'0'..=b'7').contains(&c)) => {
                return radix_to_f64(body, 8);
            }
            b'b' | b'B' if body.bytes().all(|c| c == b'0' || c == b'1') => {
                return radix_to_f64(body, 2);
            }
            _ => {}
        }
    }
    if is_decimal_literal(t) {
        return t.parse::<f64>().unwrap_or(f64::NAN);
    }
    f64::NAN
}

/// The value `_detect_numbering` would store: `(kind, Some(text))`, or `(0, None)` when it
/// leaves the cached text untouched.
fn compute_numbering(line: &Line, spans: &[Span]) -> (i32, Option<String>) {
    if line.char_count() == 0 {
        return (0, None);
    }
    if line.spans.len() > 1 {
        let first = &spans[line.spans[0]];
        let second = &spans[line.spans[1]];
        let cand = if second.char_count() == 0 && line.spans.len() > 2 {
            &spans[line.spans[2]]
        } else {
            second
        };
        if first.bbox_height() < cand.bbox_height()
            && first.bottom_edge() > cand.bottom_edge() + 0.05 * cand.bbox_height()
            && !to_number(&first.text).is_nan()
        {
            return (1, Some(first.text.clone()));
        }
    }
    let text = raw_text_of_line(line, spans);
    if let Some(m) = match_numbering_prefix(&text) {
        if let Some(g1) = m.g1
            && matches!(g1.chars().next(), Some('1'..='9'))
        {
            return (1, Some(g1.to_string()));
        }
        if let Some(g) = m.g1.or(m.g3) {
            return (2, Some(g.to_string()));
        }
        if let Some(g2) = m.g2 {
            return (3, Some(g2.to_string()));
        }
    }
    if let Some(g) = match_bracketed_num(&text) {
        return (1, Some(g.to_string()));
    }
    (0, None)
}

// ref: model/numbering.py::_detect_numbering
pub fn detect_numbering(line: &mut Line, spans: &[Span]) {
    if line.numbering_kind != -1 {
        return;
    }
    let (kind, text) = compute_numbering(line, spans);
    line.numbering_kind = kind;
    if let Some(t) = text {
        line.numbering_text = t;
    }
}

/// `(numbering_kind(line), numbering_text(line))` without filling the cache. From stage 04 on
/// lines are never modified, so a cached value is returned as is and an uncached one computed
/// fresh gives exactly what the reference's lazy fill would.
pub fn numbering_ro(line: &Line, spans: &[Span]) -> (i32, String) {
    if line.numbering_kind != -1 {
        return (line.numbering_kind, line.numbering_text.clone());
    }
    let (kind, text) = compute_numbering(line, spans);
    (kind, text.unwrap_or_else(|| line.numbering_text.clone()))
}

/// `numbering_value(line)` without filling the cache (see [`numbering_ro`]).
pub fn numbering_value_ro(line: &Line, spans: &[Span]) -> f64 {
    let (kind, text) = numbering_ro(line, spans);
    if kind == 1 {
        to_number(&text)
    } else {
        f64::NAN
    }
}

// ref: model/numbering.py::numbering_text
pub fn numbering_text(line: &mut Line, spans: &[Span]) -> String {
    detect_numbering(line, spans);
    line.numbering_text.clone()
}

// ref: model/numbering.py::numbering_value
pub fn numbering_value(line: &mut Line, spans: &[Span]) -> f64 {
    detect_numbering(line, spans);
    if line.numbering_kind == 1 {
        to_number(&line.numbering_text)
    } else {
        f64::NAN
    }
}

// ref: model/numbering.py::numbering_kind
pub fn numbering_kind(line: &mut Line, spans: &[Span]) -> i32 {
    detect_numbering(line, spans);
    line.numbering_kind
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(t: &str) -> (Option<&str>, Option<&str>, Option<&str>) {
        match match_numbering_prefix(t) {
            Some(m) => (m.g1, m.g2, m.g3),
            None => (None, None, None),
        }
    }

    #[test]
    fn numbering_prefix() {
        assert_eq!(g("12. Intro"), (Some("12"), None, None));
        assert_eq!(g("1"), (Some("1"), None, None));
        assert_eq!(g("1\n"), (Some("1"), None, None));
        assert_eq!(g("1-"), (Some("1"), None, None));
        assert_eq!(g("1-2"), (None, None, None));
        assert_eq!(g("1-a"), (Some("1"), None, None));
        assert_eq!(g("123"), (None, None, None));
        assert_eq!(g("IV. x"), (Some("IV"), None, None));
        assert_eq!(g("IVa"), (None, None, None));
        assert_eq!(g("C. x"), (None, Some("C"), None));
        assert_eq!(g("i) x"), (None, None, Some("i")));
        assert_eq!(g("１２．"), (Some("１２"), None, None));
        assert_eq!(g("1²:"), (Some("1²"), None, None));
    }

    #[test]
    fn bracketed() {
        assert_eq!(match_bracketed_num("[12]"), Some("12"));
        assert_eq!(match_bracketed_num("( 3 )x"), Some("3"));
        assert_eq!(match_bracketed_num("[123]"), None);
        assert_eq!(match_bracketed_num("[0]"), None);
    }

    #[test]
    fn to_number_grammar() {
        assert_eq!(to_number("  12 "), 12.0);
        assert_eq!(to_number(""), 0.0);
        assert_eq!(to_number("1."), 1.0);
        assert_eq!(to_number(".5"), 0.5);
        assert_eq!(to_number("-Infinity"), f64::NEG_INFINITY);
        assert_eq!(to_number("0x1F"), 31.0);
        assert_eq!(to_number("１２"), 12.0);
        assert!(to_number("1_000").is_nan());
        assert!(to_number("inf").is_nan());
        assert!(to_number("½").is_nan());
        assert!(to_number(".").is_nan());
    }
}
