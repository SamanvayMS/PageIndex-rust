//! PostScript number parsing and ToUnicode CMap interpretation.
//!
//! ref: parser_pdfium_charlevel/cmap_parse.py. Strings are code-point vectors (`PyStr`) because
//! Python `str` values here may hold lone surrogates or values that fail `chr()`.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::content_stream::{is_delim, is_ws, string_escape};
use crate::text_normalize::is_whitespace;

pub type PyStr = Vec<u32>;

/// Error standing in for the ValueError that rejects a whole CMap.
#[derive(Debug)]
pub struct CmapError;

/// ref: cmap_parse.py::_utf16be_units_to_str
pub fn utf16be_units_to_str(units: &[u32]) -> Result<PyStr, CmapError> {
    let mut u = units.to_vec();
    if u.len() % 2 == 1 {
        u.push(0);
    }
    let mut out = Vec::new();
    let mut k = 0;
    while k < u.len() {
        let w1 = (u[k] << 8) | u[k + 1];
        k += 2;
        if (w1 & 0xF800) != 0xD800 {
            out.push(w1);
            continue;
        }
        let mut w2 = 0;
        if k < u.len() {
            w2 = (u[k] << 8) | u[k + 1];
            k += 2;
        }
        out.push(((w1 & 0x3FF) << 10) + (w2 & 0x3FF) + 0x10000);
    }
    // chr() raises for values > 0x10FFFF
    if out.iter().any(|&c| c > 0x10FFFF) {
        return Err(CmapError);
    }
    Ok(out)
}

fn strip_ws(s: &str) -> &str {
    s.trim_matches(|c: char| is_whitespace(c as u32))
}

static NUM_DECIMAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[+-]?(?:[0-9]+\.?[0-9]*|\.[0-9]+)(?:[eE][+-]?[0-9]+)?\z").expect("re")
});
static NUM_INF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[+-]?Infinity\z").expect("re"));
static NUM_HEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^0[xX][0-9a-fA-F]+\z").expect("re"));
static NUM_OCT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^0[oO][0-7]+\z").expect("re"));
static NUM_BIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^0[bB][01]+\z").expect("re"));

/// ref: cmap_parse.py::_to_number
pub fn to_number(text: &str) -> f64 {
    let t = strip_ws(text);
    if t.is_empty() {
        return 0.0;
    }
    if NUM_INF.is_match(t) {
        return if t.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    let radix = |s: &str, r: u32| {
        u128::from_str_radix(s, r)
            .map(|v| v as f64)
            .unwrap_or(f64::INFINITY)
    };
    if NUM_HEX.is_match(t) {
        return radix(&t[2..], 16);
    }
    if NUM_OCT.is_match(t) {
        return radix(&t[2..], 8);
    }
    if NUM_BIN.is_match(t) {
        return radix(&t[2..], 2);
    }
    if NUM_DECIMAL.is_match(t) {
        return t.parse().unwrap_or(f64::NAN);
    }
    f64::NAN
}

/// ref: cmap_parse.py::_parse_int — leading radix digits; NaN when none.
pub fn parse_int(text: &str, radix: u32) -> f64 {
    let t: Vec<char> = text
        .trim_start_matches(|c: char| is_whitespace(c as u32))
        .chars()
        .collect();
    let mut i = 0;
    let mut neg = false;
    if i < t.len() && (t[i] == '+' || t[i] == '-') {
        neg = t[i] == '-';
        i += 1;
    }
    if radix == 16
        && i + 1 < t.len() + 1
        && t.get(i) == Some(&'0')
        && matches!(t.get(i + 1), Some('x' | 'X'))
    {
        i += 2;
    }
    let start = i;
    let mut val: f64 = 0.0;
    while i < t.len() {
        // Python `ch.lower() in digits` on "0-9a-z"[:radix] (ASCII only in practice)
        let lc: Vec<char> = t[i].to_lowercase().collect();
        if lc.len() != 1 {
            break;
        }
        match lc[0].to_digit(36) {
            Some(d) if d < radix && lc[0].is_ascii() => {
                val = val * radix as f64 + d as f64;
                i += 1;
            }
            _ => break,
        }
    }
    if i == start {
        return f64::NAN;
    }
    if neg { -val } else { val }
}

/// ref: cmap_parse.py::_cmap_str_to_int
fn cmap_str_to_int(seq: &[u32]) -> u64 {
    let mut v: u64 = 0;
    for &c in seq {
        v = ((v << 8) | c as u64) & 0xFFFF_FFFF;
    }
    v
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Hex(Vec<u32>),
    Delim(u8),
    Name,
    Num(f64),
    Op(String),
}

fn lex(data: &[u8]) -> Vec<Tok> {
    let mut toks = Vec::new();
    let n = data.len();
    let mut i = 0;
    while i < n {
        let c = data[i];
        if is_ws(c) {
            i += 1;
        } else if c == 0x25 {
            while i < n && data[i] != b'\r' && data[i] != b'\n' {
                i += 1;
            }
        } else if c == 0x3C {
            if i + 1 < n && data[i + 1] == 0x3C {
                i += 2;
                continue;
            }
            let Some(rel) = data[i..].iter().position(|&b| b == b'>') else {
                break;
            };
            let j = i + rel;
            let mut hex: Vec<u8> = data[i + 1..j]
                .iter()
                .copied()
                .filter(|b| b.is_ascii_hexdigit())
                .collect();
            if hex.len() % 2 == 1 {
                hex.pop();
            }
            let bytes: Vec<u32> = hex
                .chunks(2)
                .map(|p| {
                    u32::from_str_radix(std::str::from_utf8(p).unwrap_or("00"), 16).unwrap_or(0)
                })
                .collect();
            toks.push(Tok::Hex(bytes));
            i = j + 1;
        } else if c == 0x3E {
            i += if i + 1 < n && data[i + 1] == 0x3E {
                2
            } else {
                1
            };
        } else if c == b'[' || c == b']' {
            toks.push(Tok::Delim(c));
            i += 1;
        } else if c == 0x2F {
            let mut j = i + 1;
            while j < n && !is_ws(data[j]) && !is_delim(data[j]) {
                j += 1;
            }
            toks.push(Tok::Name);
            i = j;
        } else if c == 0x28 {
            let mut depth = 0;
            let mut s: Vec<u32> = Vec::new();
            while i < n {
                let b = data[i];
                if b == 0x5C {
                    if i + 1 >= n {
                        i += 1;
                        break;
                    }
                    let e = data[i + 1];
                    if let Some(v) = string_escape(e) {
                        s.push(v);
                        i += 2;
                    } else if (0x30..=0x37).contains(&e) {
                        let mut j = i + 1;
                        let mut val = 0u32;
                        while j < n && j - i <= 3 && (0x30..=0x37).contains(&data[j]) {
                            val = (val << 3) | (data[j] - 0x30) as u32;
                            j += 1;
                        }
                        s.push(val);
                        i = j;
                    } else if e == 0x0D || e == 0x0A {
                        i += 2;
                        if e == 0x0D && i < n && data[i] == 0x0A {
                            i += 1;
                        }
                    } else {
                        s.push(e as u32);
                        i += 2;
                    }
                    continue;
                }
                if b == 0x28 {
                    if depth != 0 {
                        s.push(b as u32);
                    }
                    depth += 1;
                } else if b == 0x29 {
                    depth -= 1;
                    if depth == 0 {
                        i += 1;
                        break;
                    }
                    s.push(b as u32);
                } else {
                    s.push(b as u32);
                }
                i += 1;
            }
            toks.push(Tok::Hex(s));
        } else {
            let mut j = i;
            while j < n && !is_ws(data[j]) && !is_delim(data[j]) {
                j += 1;
            }
            let word: String = data[i..j].iter().map(|&b| b as char).collect();
            if data[i].is_ascii_digit() || matches!(data[i], b'+' | b'-' | b'.') {
                // Python float(word): ValueError -> 0.0; inf/nan tokens are kept as-is.
                let v = word
                    .parse::<f64>()
                    .unwrap_or_else(|_| crate::content_stream::py_float_token(word.as_bytes()));
                toks.push(Tok::Num(v));
            } else {
                toks.push(Tok::Op(word));
            }
            i = j;
        }
    }
    toks
}

fn is_int(v: f64) -> bool {
    v.is_finite() && v == v.trunc()
}

/// `chr(int(v))` with Python's error semantics.
fn codepoint_to_string(v: f64) -> Result<PyStr, CmapError> {
    let cp = v.trunc();
    if cp != v || !(0.0..=1_114_111.0).contains(&cp) {
        return Err(CmapError);
    }
    Ok(vec![cp as u32])
}

/// ref: cmap_parse.py::_parse_tounicode_cmap
pub fn parse_tounicode_cmap(data: &[u8]) -> Result<HashMap<u64, PyStr>, CmapError> {
    let toks = lex(data);
    let mut out: HashMap<u64, PyStr> = HashMap::new();
    let map_range_units =
        |out: &mut HashMap<u64, PyStr>, s: u64, e: u64, units: Vec<u32>| -> Result<(), CmapError> {
            let mut units = units;
            let last = units.len() as isize - 1;
            let mut code = s;
            while code <= e {
                out.insert(code, utf16be_units_to_str(&units)?);
                if last < 0 {
                    units = vec![0];
                } else {
                    let lb = last as usize;
                    let cur = units.get(lb).copied().unwrap_or(0);
                    let nxt = cur + 1;
                    if nxt > 0xFF {
                        if lb >= 1 {
                            let mut nu = units[..lb - 1].to_vec();
                            nu.push((units[lb - 1] + 1) & 0xFFFF);
                            nu.push(0);
                            units = nu;
                        } else {
                            units = vec![0, 0];
                        }
                    } else {
                        let mut nu = units[..lb.min(units.len())].to_vec();
                        nu.push(nxt);
                        units = nu;
                    }
                }
                code += 1;
            }
            Ok(())
        };
    let hex = |t: &Tok| matches!(t, Tok::Hex(_));
    let mut k = 0;
    while k < toks.len() {
        match &toks[k] {
            Tok::Op(v) if v == "beginbfchar" => {
                k += 1;
                while k + 1 < toks.len() && hex(&toks[k]) {
                    let Tok::Hex(src) = &toks[k] else {
                        unreachable!()
                    };
                    let src = cmap_str_to_int(src);
                    let Tok::Hex(dst) = &toks[k + 1] else {
                        k += 2;
                        break;
                    };
                    out.insert(src, utf16be_units_to_str(dst)?);
                    k += 2;
                }
            }
            Tok::Op(v) if v == "beginbfrange" => {
                k += 1;
                while k + 1 < toks.len() && hex(&toks[k]) && hex(&toks[k + 1]) {
                    let (Tok::Hex(a), Tok::Hex(b)) = (&toks[k], &toks[k + 1]) else {
                        unreachable!()
                    };
                    let (s, e) = (cmap_str_to_int(a), cmap_str_to_int(b));
                    k += 2;
                    if (e as i64) - (s as i64) > 0xFF_FFFF {
                        if k < toks.len() && toks[k] == Tok::Delim(b'[') {
                            while k < toks.len() && toks[k] != Tok::Delim(b']') {
                                k += 1;
                            }
                            k += 1;
                        } else if k < toks.len() && matches!(toks[k], Tok::Hex(_) | Tok::Num(_)) {
                            k += 1;
                        }
                        break;
                    }
                    if k < toks.len() && toks[k] == Tok::Delim(b'[') {
                        k += 1;
                        let mut code = s;
                        while k < toks.len() && toks[k] != Tok::Delim(b']') {
                            if code <= e {
                                let v = match &toks[k] {
                                    Tok::Hex(d) => utf16be_units_to_str(d)?,
                                    Tok::Num(n) => codepoint_to_string(*n)?,
                                    _ => Vec::new(),
                                };
                                out.insert(code, v);
                            }
                            code += 1;
                            k += 1;
                        }
                        if k < toks.len() {
                            k += 1;
                        }
                    } else if let Some(Tok::Hex(d)) = toks.get(k) {
                        let units = d.clone();
                        k += 1;
                        map_range_units(&mut out, s, e, units)?;
                    } else if let Some(Tok::Num(n)) = toks
                        .get(k)
                        .filter(|t| matches!(t, Tok::Num(n) if is_int(*n)))
                    {
                        let units = vec![((*n as i64) & 0xFFFF) as u32];
                        k += 1;
                        map_range_units(&mut out, s, e, units)?;
                    } else {
                        break;
                    }
                }
            }
            Tok::Op(v) if v == "begincidchar" => {
                k += 1;
                while k + 1 < toks.len() && hex(&toks[k]) && matches!(toks[k + 1], Tok::Num(_)) {
                    let (Tok::Hex(a), Tok::Num(n)) = (&toks[k], &toks[k + 1]) else {
                        unreachable!()
                    };
                    if !is_int(*n) {
                        k += 2;
                        break;
                    }
                    out.insert(cmap_str_to_int(a), codepoint_to_string(*n)?);
                    k += 2;
                }
            }
            Tok::Op(v) if v == "begincidrange" => {
                k += 1;
                while k + 2 < toks.len()
                    && hex(&toks[k])
                    && hex(&toks[k + 1])
                    && matches!(toks[k + 2], Tok::Num(_))
                {
                    let (Tok::Hex(a), Tok::Hex(b), Tok::Num(st)) =
                        (&toks[k], &toks[k + 1], &toks[k + 2])
                    else {
                        unreachable!()
                    };
                    let (s, e, st) = (cmap_str_to_int(a), cmap_str_to_int(b), *st);
                    k += 3;
                    if !is_int(st) || (e as i64) - (s as i64) > 0xFF_FFFF {
                        break;
                    }
                    let mut code = s;
                    while code <= e {
                        out.insert(code, codepoint_to_string(st + (code - s) as f64)?);
                        code += 1;
                    }
                }
            }
            _ => k += 1,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bfchar_and_range() {
        let cmap = b"begincmap 2 beginbfchar <01> <0041> <02> <00420043> endbfchar \
                     1 beginbfrange <10> <12> <0061> endbfrange endcmap";
        let m = parse_tounicode_cmap(cmap).unwrap();
        assert_eq!(m[&1], vec![0x41]);
        assert_eq!(m[&2], vec![0x42, 0x43]);
        assert_eq!(m[&0x12], vec![0x63]);
    }

    #[test]
    fn numbers() {
        assert_eq!(to_number(" 0x1F "), 31.0);
        assert!(to_number("abc").is_nan());
        assert_eq!(parse_int("1Fz", 16), 31.0);
        assert!(parse_int("zz", 16).is_nan());
    }
}
