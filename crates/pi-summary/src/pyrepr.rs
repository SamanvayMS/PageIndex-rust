//! Python `repr()` / `str()` of JSON-like values (dict, list, str, int, float, bool, None).
//!
//! Needed where the reference formats Python objects into prompts: `generate_doc_description`
//! puts `{structure}` (a list of dicts) into an f-string, and `parse_summary` calls `str()` on
//! list items. String escaping follows `unicode_repr`: printable characters stay raw; per
//! Python, printable means not in categories Cc, Cf, Cs, Co, Cn, Zl, Zp, Zs (space excepted).
//! The category table is the `unicode-general-category` crate's, a newer Unicode than CPython
//! 3.11's 14.0: code points assigned after 14.0 print raw here where CPython escapes them.

use serde_json::Value;
use unicode_general_category::{GeneralCategory as G, get_general_category};

fn is_printable(c: char) -> bool {
    if c == ' ' {
        return true;
    }
    !matches!(
        get_general_category(c),
        G::Control
            | G::Format
            | G::Surrogate
            | G::PrivateUse
            | G::Unassigned
            | G::LineSeparator
            | G::ParagraphSeparator
            | G::SpaceSeparator
    )
}

/// `repr(s)` for a `str`.
pub fn repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            _ if c == quote || c == '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            _ if c.is_ascii() || is_printable(c) => out.push(c),
            _ => {
                let v = c as u32;
                if v <= 0xff {
                    out.push_str(&format!("\\x{v:02x}"));
                } else if v <= 0xffff {
                    out.push_str(&format!("\\u{v:04x}"));
                } else {
                    out.push_str(&format!("\\U{v:08x}"));
                }
            }
        }
    }
    out.push(quote);
    out
}

/// `repr(x)` for a float (`float_repr_style == 'short'`).
pub fn repr_float(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    // shortest round-trip digits, as d.ddd e X
    let sci = format!("{x:e}");
    let (mant, exp) = sci.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let neg = mant.starts_with('-');
    let digits: String = mant.chars().filter(char::is_ascii_digit).collect();
    let sign = if neg { "-" } else { "" };
    if (-4..16).contains(&exp) {
        let n = digits.len() as i32;
        let body = if exp >= 0 {
            let int_len = exp + 1;
            if n <= int_len {
                format!("{digits}{}.0", "0".repeat((int_len - n) as usize))
            } else {
                format!(
                    "{}.{}",
                    &digits[..int_len as usize],
                    &digits[int_len as usize..]
                )
            }
        } else {
            format!("0.{}{digits}", "0".repeat((-exp - 1) as usize))
        };
        format!("{sign}{body}")
    } else {
        let m = if digits.len() == 1 {
            digits.clone()
        } else {
            format!("{}.{}", &digits[..1], &digits[1..])
        };
        let es = if exp < 0 { '-' } else { '+' };
        format!("{sign}{m}e{es}{:02}", exp.abs())
    }
}

/// `repr(v)`.
pub fn repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => {
            if n.is_f64() {
                repr_float(n.as_f64().unwrap())
            } else {
                n.to_string()
            }
        }
        Value::String(s) => repr_str(s),
        Value::Array(a) => format!("[{}]", a.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{}: {}", repr_str(k), repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `str(v)`: the string itself for a `str`, `repr` otherwise.
pub fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => repr(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn repr_matches_cpython() {
        // expected values from CPython 3.11
        assert_eq!(repr_str("it's"), "\"it's\"");
        assert_eq!(repr_str("say \"hi\" it's"), "'say \"hi\" it\\'s'");
        assert_eq!(repr_str("a\\b\tc\nd\x01\x7f"), "'a\\\\b\\tc\\nd\\x01\\x7f'");
        assert_eq!(
            repr_str("é 日本 \u{a0}\u{200b}\u{2028}\u{85}😀"),
            "'é 日本 \\xa0\\u200b\\u2028\\x85😀'"
        );
        assert_eq!(repr_float(1.0), "1.0");
        assert_eq!(repr_float(0.1), "0.1");
        assert_eq!(repr_float(1e16), "1e+16");
        assert_eq!(repr_float(1.5e-5), "1.5e-05");
        assert_eq!(repr_float(123456.789), "123456.789");
        assert_eq!(repr_float(0.0001), "0.0001");
        assert_eq!(repr_float(-2.5e20), "-2.5e+20");
        assert_eq!(repr_float(1234567890123456.0), "1234567890123456.0");
        assert_eq!(
            repr(
                &json!([{"title": "A", "node_id": "0000", "nodes": [{"x": null, "y": true, "z": 3}]}])
            ),
            "[{'title': 'A', 'node_id': '0000', 'nodes': [{'x': None, 'y': True, 'z': 3}]}]"
        );
        assert_eq!(py_str(&json!("s")), "s");
        assert_eq!(py_str(&json!(["s"])), "['s']");
    }
}
