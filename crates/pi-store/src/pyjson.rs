//! `json.dumps(value, ensure_ascii=False)` byte-for-byte, plus the few `str()`/`repr()`
//! renderings of JSON values that the tool layer quotes back to callers.
//!
//! Python's default separators are `", "` and `": "`; floats use `float.__repr__`; strings
//! escape only `"`, `\`, and U+0000..U+001F (`\n`, `\r`, `\t`, `\b`, `\f` by name, the rest
//! as lowercase `\u00XX`). Everything else — including U+007F, U+2028 and non-ASCII — is
//! written raw.

use serde_json::{Map, Value};

/// `json.dumps(value, ensure_ascii=False)`.
pub fn dumps(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value);
    out
}

/// `len(json.dumps(value, ensure_ascii=False))` — Python counts code points, not bytes.
pub fn serialized_len(value: &Value) -> usize {
    dumps(value).chars().count()
}

fn write_value(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else {
                out.push_str(&json_float(n.as_f64().unwrap_or(f64::NAN)));
            }
        }
        Value::String(s) => write_str(out, s),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_value(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => write_map(out, map),
    }
}

fn write_map(out: &mut String, map: &Map<String, Value>) {
    out.push('{');
    for (i, (k, v)) in map.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_str(out, k);
        out.push_str(": ");
        write_value(out, v);
    }
    out.push('}');
}

fn write_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `float.__repr__` as `json` emits it (`NaN`/`Infinity` for non-finite values).
pub fn json_float(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    float_repr(x)
}

/// `repr(x)` for a finite float: shortest round-trip digits; scientific notation when the
/// decimal exponent is < -4 or >= 16.
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.into();
    }
    // `{:e}` yields the shortest round-trip digits: "-1.2345e17", "0e0".
    let sci = format!("{x:e}");
    let (neg, sci) = match sci.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, sci.as_str()),
    };
    let (mantissa, exp) = sci.split_once('e').expect("{:e} always has an exponent");
    let exp: i32 = exp.parse().expect("integer exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if !(-4..16).contains(&exp) {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        out.push_str(&format!("{:02}", exp.unsigned_abs()));
        return out;
    }
    let decpt = exp + 1; // digits placed as 0.d1d2… × 10^decpt
    if decpt <= 0 {
        out.push_str("0.");
        out.push_str(&"0".repeat((-decpt) as usize));
        out.push_str(&digits);
    } else if decpt as usize >= digits.len() {
        out.push_str(&digits);
        out.push_str(&"0".repeat(decpt as usize - digits.len()));
        out.push_str(".0");
    } else {
        out.push_str(&digits[..decpt as usize]);
        out.push('.');
        out.push_str(&digits[decpt as usize..]);
    }
    out
}

/// `str(value)` for a value that came out of `json.loads`.
pub fn py_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => py_repr(other),
    }
}

/// `repr(value)` for a value that came out of `json.loads`.
pub fn py_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(u) = n.as_u64() {
                u.to_string()
            } else {
                float_repr(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        Value::String(s) => str_repr(s),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(py_repr).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}: {}", str_repr(k), py_repr(v)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

/// Python type name of a JSON value (`type(value).__name__`).
pub fn py_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `repr(s)` for a `str`. Printability follows `str.isprintable` for ASCII and Latin-1 and
/// the common non-printable ranges (separators, format characters) beyond.
pub fn str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if !is_printable(c) => {
                let cp = c as u32;
                if cp <= 0xff {
                    out.push_str(&format!("\\x{cp:02x}"));
                } else if cp <= 0xffff {
                    out.push_str(&format!("\\u{cp:04x}"));
                } else {
                    out.push_str(&format!("\\U{cp:08x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

fn is_printable(c: char) -> bool {
    !matches!(c,
        '\u{00}'..='\u{1F}' | '\u{7F}'..='\u{A0}' | '\u{AD}' | '\u{1680}' | '\u{2000}'..='\u{200F}'
        | '\u{2028}'..='\u{202F}' | '\u{205F}'..='\u{2064}' | '\u{2066}'..='\u{206F}' | '\u{3000}'
        | '\u{E000}'..='\u{F8FF}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}' | '\u{E0000}'..='\u{10FFFF}')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn floats_match_python_repr() {
        // Expected strings from CPython 3.11 json.dumps.
        let cases = [
            (1e16, "1e+16"),
            (1.0, "1.0"),
            (1e-5, "1e-05"),
            (0.1, "0.1"),
            (123456789012345678.0, "1.2345678901234568e+17"),
            (-0.0, "-0.0"),
            (1.5e300, "1.5e+300"),
            (2e-7, "2e-07"),
            (12345.678, "12345.678"),
            (1e22, "1e+22"),
            (9999999999999998.0, "9999999999999998.0"),
            (0.0001, "0.0001"),
            (0.0, "0.0"),
        ];
        for (x, want) in cases {
            assert_eq!(float_repr(x), want, "{x}");
        }
    }

    #[test]
    fn strings_and_separators() {
        let v = json!({"a": "\u{7f}\u{2028}\u{01}\u{1f} é \\ \" \t", "b": [1, 2.5, null, true]});
        assert_eq!(
            dumps(&v),
            "{\"a\": \"\u{7f}\u{2028}\\u0001\\u001f é \\\\ \\\" \\t\", \"b\": [1, 2.5, null, true]}"
        );
        assert_eq!(dumps(&json!({})), "{}");
        assert_eq!(dumps(&json!([])), "[]");
    }

    #[test]
    fn repr_matches_python() {
        let v = json!([1, "a", "it's", {"k": null, "b": true}, 1.0, "x\"y'z", "\n"]);
        assert_eq!(
            py_repr(&v),
            r#"[1, 'a', "it's", {'k': None, 'b': True}, 1.0, 'x"y\'z', '\n']"#
        );
    }
}
