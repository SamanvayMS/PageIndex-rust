//! Glyph tables, simple-font encoding resolution and per-font Unicode maps.
//!
//! ref: parser_pdfium_charlevel/glyph_tables.py and font_unicode.py

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;

use crate::cmap::{PyStr, parse_int, parse_tounicode_cmap, to_number};
use crate::pdfobj::PdfDoc;

#[derive(Deserialize)]
struct GlyphTables {
    glyphs: HashMap<String, i64>,
    encodings: HashMap<String, Vec<String>>,
}

static TABLES: LazyLock<GlyphTables> = LazyLock::new(|| {
    serde_json::from_str(pi_data::GLYPH_NAME_TABLE_JSON).expect("glyph_name_table.json")
});

/// ref: glyph_tables.py::_get_unicode_for_glyph
fn unicode_for_glyph(name: &str) -> i64 {
    if let Some(&cp) = TABLES.glyphs.get(name) {
        return cp;
    }
    let chars: Vec<char> = name.chars().collect();
    if chars.first() != Some(&'u') {
        return -1;
    }
    let n = chars.len();
    let hex: String = if n == 7 && chars[1] == 'n' && chars[2] == 'i' {
        chars[3..].iter().collect()
    } else if (5..=7).contains(&n) {
        chars[1..].iter().collect()
    } else {
        return -1;
    };
    if hex == hex.to_uppercase() {
        let v = parse_int(&hex, 16);
        if v >= 0.0 {
            return v as i64;
        }
    }
    -1
}

/// ref: glyph_tables.py::_from_char_code — chr(n & 0xFFFF)
fn from_char_code(n: i64) -> PyStr {
    vec![(n & 0xFFFF) as u32]
}

const TYPE1_SPECIAL: &[u8] = b"/[]{}()";

fn type1_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}

enum Builtin {
    Named(String),
    Array(BTreeMap<i64, String>),
}

fn to_int32(v: f64) -> i64 {
    let v = v as i128;
    ((v + (1 << 31)).rem_euclid(1 << 32) - (1 << 31)) as i64
}

/// ref: font_unicode.py::_type1_builtin_encoding
fn type1_builtin_encoding(font_file: &[u8]) -> Option<Builtin> {
    let end = font_file
        .windows(5)
        .position(|w| w == b"eexec")
        .unwrap_or(font_file.len());
    let head = &font_file[..end];
    let mut toks: Vec<&[u8]> = Vec::new();
    let n = head.len();
    let mut i = 0;
    while i < n {
        let c = head[i];
        if type1_ws(c) {
            i += 1;
        } else if c == 0x25 {
            while i < n && head[i] != b'\r' && head[i] != b'\n' {
                i += 1;
            }
        } else if TYPE1_SPECIAL.contains(&c) {
            toks.push(&head[i..i + 1]);
            i += 1;
        } else {
            let mut j = i;
            while j < n && !type1_ws(head[j]) && !TYPE1_SPECIAL.contains(&head[j]) {
                j += 1;
            }
            toks.push(&head[i..j]);
            i = j;
        }
    }
    let tok = |k: usize| toks.get(k).copied();
    let latin = |b: &[u8]| -> String { b.iter().map(|&c| c as char).collect() };
    let mut result: Option<Builtin> = None;
    let mut p = 0;
    while p < toks.len() {
        if toks[p] != b"/" {
            p += 1;
            continue;
        }
        let name_tok = tok(p + 1);
        p += 2;
        if name_tok != Some(b"Encoding".as_slice()) {
            continue;
        }
        let Some(arg) = tok(p) else {
            result = None;
            break;
        };
        if arg.is_empty() || !arg.iter().all(u8::is_ascii_digit) {
            let name = latin(arg);
            result = if TABLES.encodings.contains_key(&name) {
                Some(Builtin::Named(name))
            } else {
                None
            };
            p += 1;
            continue;
        }
        let f: f64 = latin(arg).parse().unwrap_or(f64::INFINITY);
        let size = if f == f64::INFINITY { 0 } else { to_int32(f) };
        p += 1;
        let mut enc = BTreeMap::new();
        for _ in 0..size.max(0) {
            let mut t = tok(p);
            while let Some(tt) = t {
                if tt == b"dup" || tt == b"def" {
                    break;
                }
                p += 1;
                t = tok(p);
            }
            let Some(tt) = t else { return result };
            if tt == b"def" {
                break;
            }
            p += 1;
            let t = tok(p);
            let mut v = match t {
                Some(t) => parse_int(&latin(t), 10),
                None => 0.0,
            };
            if !v.is_finite() {
                v = 0.0;
            }
            let idx = to_int32(v);
            p += 2;
            let g = tok(p);
            p += 1;
            if let Some(g) = g {
                enc.insert(idx, latin(g));
            }
            p += 1;
        }
        result = Some(Builtin::Array(enc));
    }
    result
}

/// ref: font_unicode.py::_simple_font_to_unicode
fn simple_font_to_unicode(
    default_enc: &[String],
    base_encoding_name: Option<&str>,
    differences: &BTreeMap<i64, String>,
    force_glyphs: bool,
) -> BTreeMap<i64, PyStr> {
    let mut encoding: BTreeMap<i64, String> = default_enc
        .iter()
        .enumerate()
        .map(|(k, g)| (k as i64, g.clone()))
        .collect();
    for (k, g) in differences {
        if g == ".notdef" {
            continue;
        }
        encoding.insert(*k, g.clone());
    }
    let mut out = BTreeMap::new();
    for (&charcode, glyph) in &encoding {
        if glyph.is_empty() {
            continue;
        }
        if let Some(&cp) = TABLES.glyphs.get(glyph) {
            out.insert(charcode, from_char_code(cp));
            continue;
        }
        let gl: Vec<char> = glyph.chars().collect();
        let tail: String = gl[1..].iter().collect();
        let mut code: i64 = 0;
        match gl[0] {
            'G' if gl.len() == 3 => {
                let v = parse_int(&tail, 16);
                code = if v.is_nan() { 0 } else { v as i64 };
            }
            'g' if gl.len() == 5 => {
                let v = parse_int(&tail, 16);
                code = if v.is_nan() { 0 } else { v as i64 };
            }
            'C' | 'c' if (3..=4).contains(&gl.len()) => {
                if force_glyphs {
                    let v = parse_int(&tail, 16);
                    code = if v.is_nan() { 0 } else { v as i64 };
                } else {
                    let num = to_number(&tail);
                    if num.is_nan() {
                        if !parse_int(&tail, 16).is_nan() {
                            return simple_font_to_unicode(
                                default_enc,
                                base_encoding_name,
                                differences,
                                true,
                            );
                        }
                        code = 0;
                    } else if num.is_finite() && num.fract() == 0.0 {
                        code = num as i64;
                    }
                }
            }
            'u' => {
                let u = unicode_for_glyph(glyph);
                if u != -1 {
                    code = u;
                }
            }
            _ => {}
        }
        if 0 < code && code <= 0x10FFFF {
            if let Some(base_name) = base_encoding_name.filter(|b| !b.is_empty()) {
                if code == charcode {
                    if let Some(base) = TABLES.encodings.get(base_name) {
                        if (0..base.len() as i64).contains(&charcode)
                            && !base[charcode as usize].is_empty()
                        {
                            let g = TABLES
                                .glyphs
                                .get(&base[charcode as usize])
                                .copied()
                                .unwrap_or(0);
                            out.insert(charcode, from_char_code(g));
                            continue;
                        }
                    }
                }
            }
            out.insert(charcode, vec![code as u32]);
        }
    }
    out
}

static DESC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d+)[\s\x1C-\x1F]+\d+[\s\x1C-\x1F]+R").expect("re"));
static FLAGS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/Flags[\s\x1C-\x1F]+([+-]?\d+)").expect("re"));
static BASEENC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"/BaseEncoding[\s\x1C-\x1F]*/([^\s\x1C-\x1F/\[\]<>()]+)").expect("re")
});
static DIFF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/Differences[\s\x1C-\x1F]*\[").expect("re"));
static DIFF_TOK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/([^\s\x1C-\x1F/\[\]<>()]+)|(\d+)").expect("re"));
static HEX_ESC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"#([0-9a-fA-F]{2})").expect("re"));
static STRIP_PAREN_WS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[()\s\x1C-\x1F]").expect("re"));
static STYLE_SEP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[,_]").expect("re"));
static SYMBOL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)Symbol").expect("re"));
static DINGBATS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)Dingbats|Wingdings").expect("re"));

fn first_int(s: &str) -> Option<i64> {
    s.split_whitespace().next()?.parse().ok()
}

/// Per-font map: (bytes per code, {charcode: unicode}).
pub type FontMap = (u8, HashMap<u64, PyStr>);

fn to_u64_map(m: BTreeMap<i64, PyStr>) -> HashMap<u64, PyStr> {
    m.into_iter()
        .filter(|(k, _)| *k >= 0)
        .map(|(k, v)| (k as u64, v))
        .collect()
}

/// ref: font_unicode.py::_font_unicode_map
pub fn font_unicode_map(pdf: &PdfDoc, xref: i64) -> Option<FontMap> {
    let get = |x: i64, k: &str| pdf.xref_get_key(x, k);
    let (t, v) = get(xref, "Subtype");
    let subtype = if t == "name" {
        v.trim_start_matches('/').to_string()
    } else {
        String::new()
    };
    if subtype == "Type0" {
        let (t, v) = get(xref, "Encoding");
        if t != "name" || !matches!(v.trim_start_matches('/'), "Identity-H" | "Identity-V") {
            return None;
        }
        let mut desc_xref: i64 = 0;
        let (mut dt, mut dv) = get(xref, "DescendantFonts");
        if dt == "xref" {
            dv = pdf.xref_object(first_int(&dv)?)?;
            dt = "array";
        }
        if dt == "array" {
            if let Some(c) = DESC_RE.captures(&dv) {
                desc_xref = c[1].parse().ok()?;
            }
        }
        let (mut tt, mut tv) = ("null", String::from("null"));
        if desc_xref != 0 {
            (tt, tv) = get(desc_xref, "ToUnicode");
        }
        if tt != "xref" {
            (tt, tv) = get(xref, "ToUnicode");
        }
        let mut tu: Option<HashMap<u64, PyStr>> = None;
        if tt == "xref" {
            tu = first_int(&tv)
                .and_then(|x| pdf.xref_stream(x))
                .and_then(|d| parse_tounicode_cmap(&d).ok());
        }
        if let Some(m) = tu.filter(|m| !m.is_empty()) {
            return Some((2, m));
        }
        if desc_xref != 0 {
            let (rt, rv) = get(desc_xref, "CIDSystemInfo/Registry");
            let (ot, ov) = get(desc_xref, "CIDSystemInfo/Ordering");
            let reg = if rt != "null" {
                STRIP_PAREN_WS.replace_all(&rv, "").into_owned()
            } else {
                String::new()
            };
            let ord = if ot != "null" {
                STRIP_PAREN_WS.replace_all(&ov, "").into_owned()
            } else {
                String::new()
            };
            if reg == "Adobe" && matches!(ord.as_str(), "GB1" | "CNS1" | "Japan1" | "Korea1") {
                return None;
            }
        }
        return Some((2, HashMap::new()));
    }
    let (t, v) = get(xref, "BaseFont");
    let base_font = if t == "name" {
        v.trim_start_matches('/').to_string()
    } else {
        String::new()
    };
    let mut flags: i64 = 0;
    let mut fd_xref: i64 = 0;
    let mut has_descriptor = false;
    let (t, v) = get(xref, "FontDescriptor");
    if t == "xref" {
        fd_xref = first_int(&v)?;
        has_descriptor = true;
        let (ft, fv) = get(fd_xref, "Flags");
        if ft == "int" {
            flags = fv.parse().ok()?;
        }
    } else if t == "dict" {
        has_descriptor = true;
        if let Some(c) = FLAGS_RE.captures(&v) {
            flags = c[1].parse().ok()?;
        }
    }
    if !has_descriptor && subtype != "Type3" {
        if base_font.is_empty() {
            return None;
        }
        let wo = STYLE_SEP.replace_all(&base_font, "-");
        let wo = wo.split('-').next().unwrap_or("");
        flags = if matches!(wo, "Symbol" | "Dingbats" | "ZapfDingbats") {
            4
        } else {
            32
        };
    }
    let mut file_key: Option<(&str, i64)> = None;
    if fd_xref != 0 {
        for k in ["FontFile", "FontFile2", "FontFile3"] {
            let (ft, fv) = get(fd_xref, k);
            if ft == "xref" {
                file_key = Some((k, first_int(&fv)?));
                break;
            }
        }
    }
    let mut differences: BTreeMap<i64, String> = BTreeMap::new();
    let mut base_encoding_name: Option<String> = None;
    let (t, v) = get(xref, "Encoding");
    let enc_obj = match t {
        "name" => {
            base_encoding_name = Some(v.trim_start_matches('/').to_string());
            None
        }
        "xref" => Some(pdf.xref_object(first_int(&v)?)?),
        "dict" => Some(v),
        _ => None,
    };
    if let Some(enc) = enc_obj {
        if let Some(c) = BASEENC_RE.captures(&enc) {
            base_encoding_name = Some(c[1].to_string());
        }
        if let Some(m) = DIFF_RE.find(&enc) {
            let bytes: Vec<char> = enc.chars().collect();
            // positions are char offsets in Python; convert the byte offset
            let start = enc[..m.end()].chars().count();
            let mut depth = 1;
            let mut j = start;
            while j < bytes.len() && depth > 0 {
                if bytes[j] == '[' {
                    depth += 1;
                } else if bytes[j] == ']' {
                    depth -= 1;
                }
                j += 1;
            }
            let body: String = bytes[start..j.saturating_sub(1).max(start)]
                .iter()
                .collect();
            let mut idx: i64 = 0;
            for c in DIFF_TOK_RE.captures_iter(&body) {
                if let Some(num) = c.get(2) {
                    idx = num.as_str().parse().unwrap_or(idx);
                } else if let Some(name) = c.get(1) {
                    let nv = HEX_ESC_RE.replace_all(name.as_str(), |h: &regex::Captures| {
                        char::from_u32(u32::from_str_radix(&h[1], 16).unwrap_or(0))
                            .unwrap_or('\0')
                            .to_string()
                    });
                    differences.insert(idx, nv.into_owned());
                    idx += 1;
                }
            }
        }
    }
    if !matches!(
        base_encoding_name.as_deref(),
        Some("MacRomanEncoding" | "MacExpertEncoding" | "WinAnsiEncoding")
    ) {
        base_encoding_name = None;
    }
    let default_name: String = match &base_encoding_name {
        Some(b) => b.clone(),
        None => {
            let symbolic = flags & 4 != 0;
            let nonsymbolic = flags & 32 != 0;
            let mut d = "StandardEncoding";
            if subtype == "TrueType" && !nonsymbolic {
                d = "WinAnsiEncoding";
            }
            if symbolic {
                d = "MacRomanEncoding";
                if file_key.is_none() {
                    if SYMBOL_RE.is_match(&base_font) {
                        d = "SymbolSetEncoding";
                    } else if DINGBATS_RE.is_match(&base_font) {
                        d = "ZapfDingbatsEncoding";
                    }
                }
            }
            d.to_string()
        }
    };
    let default_enc = TABLES.encodings.get(&default_name)?;
    let has_encoding = base_encoding_name.is_some() || !differences.is_empty();
    let (t, v) = get(xref, "ToUnicode");
    let mut included: Option<HashMap<u64, PyStr>> = None;
    if t == "xref" {
        included = first_int(&v)
            .and_then(|x| pdf.xref_stream(x))
            .and_then(|d| parse_tounicode_cmap(&d).ok());
    }
    if let Some(inc) = included.filter(|m| !m.is_empty()) {
        let mut fin = inc;
        if has_encoding {
            for (k, g) in simple_font_to_unicode(
                default_enc,
                base_encoding_name.as_deref(),
                &differences,
                false,
            ) {
                if k >= 0 {
                    fin.entry(k as u64).or_insert(g);
                }
            }
        }
        return Some((1, fin));
    }
    let mut fin = simple_font_to_unicode(
        default_enc,
        base_encoding_name.as_deref(),
        &differences,
        false,
    );
    if let Some(("FontFile", fx)) = file_key {
        if matches!(subtype.as_str(), "Type1" | "MMType1") {
            let builtin = pdf.xref_stream(fx).and_then(|d| type1_builtin_encoding(&d));
            if let Some(b) = builtin {
                let skip_same = matches!(&b, Builtin::Named(n) if *n == default_name);
                if !skip_same {
                    let items: Vec<(i64, String)> = match b {
                        Builtin::Named(n) => TABLES.encodings[&n]
                            .iter()
                            .enumerate()
                            .map(|(k, g)| (k as i64, g.clone()))
                            .collect(),
                        Builtin::Array(m) => m.into_iter().collect(),
                    };
                    for (k, name) in items {
                        if has_encoding
                            && (base_encoding_name.is_some() || differences.contains_key(&k))
                        {
                            continue;
                        }
                        if name.is_empty() {
                            continue;
                        }
                        let cp = unicode_for_glyph(&name);
                        if cp != -1 {
                            fin.insert(k, from_char_code(cp));
                        }
                    }
                }
            }
        }
    }
    Some((1, to_u64_map(fin)))
}
