//! Content-stream show-operator tokenization and per-page operator tagging.
//!
//! ref: parser_pdfium_charlevel/content_stream.py and pipeline.py::_page_pass1 (the
//! content-stream half: vertical tags, Tz tags, font-Unicode repair).

use std::collections::HashSet;

use crate::code_walk::page_show_codes;
use crate::model::{RawChar, TextObj};
use crate::pdfobj::{PdfDoc, decode_pdf_name};
use crate::unicode_apply::{FontMapCache, apply_font_unicode};

pub const WS: [u8; 6] = [0x20, 0x09, 0x0d, 0x0a, 0x0c, 0x00];
pub const DELIM: &[u8] = b"()<>[]{}/%";

pub fn is_ws(b: u8) -> bool {
    WS.contains(&b)
}

pub fn is_delim(b: u8) -> bool {
    DELIM.contains(&b)
}

/// ref: pdf_objects.py::_PDF_STRING_ESCAPE_BYTES
pub fn string_escape(b: u8) -> Option<u32> {
    Some(match b {
        0x6E => 0x0A,
        0x72 => 0x0D,
        0x74 => 0x09,
        0x62 => 0x08,
        0x66 => 0x0C,
        0x28 => 0x28,
        0x29 => 0x29,
        0x5C => 0x5C,
        _ => return None,
    })
}

/// ref: content_stream.py::_OP_OPERAND_COUNTS
fn operand_spec(op: &[u8]) -> Option<(usize, bool)> {
    Some(match op {
        b"w" | b"J" | b"j" | b"M" | b"ri" | b"i" | b"gs" => (1, false),
        b"d" => (2, false),
        b"q" | b"Q" => (0, false),
        b"cm" => (6, false),
        b"m" | b"l" => (2, false),
        b"c" => (6, false),
        b"v" | b"y" => (4, false),
        b"h" => (0, false),
        b"re" => (4, false),
        b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" | b"W" | b"W*"
        | b"BT" | b"ET" => (0, false),
        b"Tc" | b"Tw" | b"Tz" | b"TL" => (1, false),
        b"Tf" => (2, false),
        b"Tr" | b"Ts" => (1, false),
        b"Td" | b"TD" => (2, false),
        b"Tm" => (6, false),
        b"T*" => (0, false),
        b"Tj" | b"TJ" | b"'" => (1, false),
        b"\"" => (3, false),
        b"d0" => (2, false),
        b"d1" => (6, false),
        b"CS" | b"cs" => (1, false),
        b"SC" | b"sc" => (4, true),
        b"SCN" | b"scn" => (33, true),
        b"G" | b"g" => (1, false),
        b"RG" | b"rg" => (3, false),
        b"K" | b"k" => (4, false),
        b"sh" => (1, false),
        b"BI" | b"ID" => (0, false),
        b"EI" => (1, false),
        b"Do" | b"MP" => (1, false),
        b"DP" => (2, false),
        b"BMC" => (1, false),
        b"BDC" => (2, false),
        b"EMC" | b"BX" | b"EX" => (0, false),
        _ => return None,
    })
}

fn is_lex_prefix(op: &[u8]) -> bool {
    matches!(
        op,
        b"BM" | b"BD" | b"true" | b"fa" | b"fal" | b"fals" | b"false" | b"nu" | b"nul" | b"null"
    )
}

fn is_flush_op(op: &[u8]) -> bool {
    matches!(op, b"q" | b"Q" | b"Do" | b"BDC" | b"BMC" | b"EMC")
}

/// Python `float(token)` on an ASCII numeric-looking token; non-finite and invalid -> 0.0.
pub fn py_float_token(tok: &[u8]) -> f64 {
    let s = String::from_utf8_lossy(tok);
    let mut v: f64 = s.parse().unwrap_or_else(|_| {
        // Python also accepts digit-separating underscores.
        let t = s.replace('_', "");
        if !s.contains('_') || s.starts_with('_') || s.ends_with('_') || s.contains("__") {
            0.0
        } else {
            t.parse().unwrap_or(0.0)
        }
    });
    if !v.is_finite() {
        v = 0.0;
    }
    v
}

#[derive(Debug, Clone)]
pub enum Opnd {
    Str(Vec<u32>),
    Arr(Vec<u32>),
    Name(Vec<u8>),
    Num(f64),
    Dict,
    Other,
}

/// A Form XObject paint: (show-op position, xobject name, font at `Do`, Tz at `Do`).
pub type Paint = (usize, Vec<u8>, Option<Vec<u8>>, f64);

/// Output of `_tokenize_show_operators`.
#[derive(Debug, Default)]
pub struct Shows {
    pub flush_ids: Vec<usize>,
    pub fonts: Vec<Option<Vec<u8>>>,
    pub units: Vec<Vec<u32>>,
    pub tzs: Vec<f64>,
    pub paints: Vec<Paint>,
}

/// ref: content_stream.py::_tokenize_show_operators
pub fn tokenize_show_operators(data: &[u8], init_tz: f64) -> Shows {
    let mut out = Shows::default();
    let mut flush_id = 0usize;
    let mut cur_font: Option<Vec<u8>> = None;
    let mut font_stack: Vec<Option<Vec<u8>>> = Vec::new();
    let mut cur_tz = init_tz;
    let mut tz_stack: Vec<f64> = Vec::new();
    let mut opnds: Vec<Opnd> = Vec::new();
    let mut frames: Vec<(bool, Vec<Opnd>)> = Vec::new(); // (is_dict, items)
    let mut non_processed: Vec<Opnd> = Vec::new();
    let mut bi_mark: Option<usize> = None;
    let n = data.len();
    let mut i = 0usize;

    macro_rules! push {
        ($v:expr) => {
            match frames.last_mut() {
                Some(f) => f.1.push($v),
                None => opnds.push($v),
            }
        };
    }

    while i < n {
        let b = data[i];
        if is_ws(b) {
            i += 1;
        } else if b == 0x25 {
            while i < n && data[i] != b'\r' && data[i] != b'\n' {
                i += 1;
            }
        } else if b == 0x28 {
            let mut depth = 0i32;
            let mut s: Vec<u32> = Vec::new();
            while i < n {
                let lb = data[i];
                if lb == 0x5c {
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
                if lb == 0x28 {
                    if depth != 0 {
                        s.push(lb as u32);
                    }
                    depth += 1;
                } else if lb == 0x29 {
                    depth -= 1;
                    if depth == 0 {
                        i += 1;
                        break;
                    }
                    s.push(lb as u32);
                } else {
                    s.push(lb as u32);
                }
                i += 1;
            }
            push!(Opnd::Str(s));
        } else if b == 0x3c {
            if i + 1 < n && data[i + 1] == 0x3c {
                frames.push((true, Vec::new()));
                i += 2;
            } else {
                i += 1;
                let mut nib: Vec<u32> = Vec::new();
                while i < n && data[i] != 0x3e {
                    let h = data[i];
                    match h {
                        b'0'..=b'9' => nib.push((h - b'0') as u32),
                        b'A'..=b'F' => nib.push((h - 0x37) as u32),
                        b'a'..=b'f' => nib.push((h - 0x57) as u32),
                        _ => {}
                    }
                    i += 1;
                }
                i += 1;
                if nib.len() % 2 == 1 {
                    nib.pop();
                }
                push!(Opnd::Str(
                    nib.chunks(2).map(|p| (p[0] << 4) | p[1]).collect()
                ));
            }
        } else if b == 0x3e {
            if i + 1 < n && data[i + 1] == 0x3e {
                i += 2;
                if frames.last().is_some_and(|f| f.0) {
                    frames.pop();
                    push!(Opnd::Dict);
                }
            } else {
                i += 1;
            }
        } else if b == 0x5b {
            frames.push((false, Vec::new()));
            i += 1;
        } else if b == 0x5d {
            i += 1;
            if frames.last().is_some_and(|f| !f.0) {
                let items = frames.pop().expect("frame").1;
                let units: Vec<u32> = items
                    .into_iter()
                    .filter_map(|o| if let Opnd::Str(s) = o { Some(s) } else { None })
                    .flatten()
                    .collect();
                push!(Opnd::Arr(units));
            }
        } else if b == b'{' || b == b'}' {
            i += 1;
        } else if b == 0x2f {
            i += 1;
            let mut j = i;
            while j < n && !is_ws(data[j]) && !is_delim(data[j]) {
                j += 1;
            }
            push!(Opnd::Name(decode_pdf_name(&data[i..j])));
            i = j;
        } else {
            let first = data[i];
            let numeric = first.is_ascii_digit() || matches!(first, b'+' | b'-' | b'.');
            let mut j = i;
            if numeric {
                while j < n && !is_ws(data[j]) && !is_delim(data[j]) {
                    j += 1;
                }
            } else {
                let mut known = false;
                while j < n && !is_ws(data[j]) && !is_delim(data[j]) {
                    let cand = &data[i..j + 1];
                    if known && operand_spec(cand).is_none() && !is_lex_prefix(cand) {
                        break;
                    }
                    j += 1;
                    let cur = &data[i..j];
                    known = operand_spec(cur).is_some() || is_lex_prefix(cur);
                }
            }
            let tok = &data[i..j];
            i = j;
            if tok.is_empty() {
                i += 1;
                continue;
            }
            if numeric {
                push!(Opnd::Num(py_float_token(tok)));
                continue;
            }
            if tok == b"true" || tok == b"false" {
                push!(Opnd::Other);
                continue;
            }
            if tok == b"null" {
                continue;
            }
            if let Some(f) = frames.last_mut() {
                f.1.push(Opnd::Other);
                continue;
            }
            if tok == b"BI" {
                bi_mark = Some(opnds.len());
                continue;
            }
            let Some((need, variable)) = operand_spec(tok) else {
                continue;
            };
            if tok == b"ID" {
                let mut filt: &[u8] = b"";
                if let Some(bm) = bi_mark {
                    for o in &opnds[bm.min(opnds.len())..] {
                        if let Opnd::Name(v) = o {
                            if matches!(
                                v.as_slice(),
                                b"DCTDecode"
                                    | b"DCT"
                                    | b"ASCII85Decode"
                                    | b"A85"
                                    | b"ASCIIHexDecode"
                                    | b"AHx"
                            ) {
                                filt = v;
                                break;
                            }
                        }
                    }
                }
                let find = |pat: &[u8], from: usize| -> Option<usize> {
                    if from > n {
                        return None;
                    }
                    data[from..]
                        .windows(pat.len())
                        .position(|w| w == pat)
                        .map(|p| p + from)
                };
                let mut k = i + 1;
                match filt {
                    b"DCTDecode" | b"DCT" => {
                        if let Some(m) = find(b"\xff\xd9", k) {
                            k = m + 2;
                        }
                    }
                    b"ASCII85Decode" | b"A85" => {
                        if let Some(m) = find(b"~>", k) {
                            k = m + 2;
                        }
                    }
                    b"ASCIIHexDecode" | b"AHx" => {
                        if let Some(m) = find(b">", k) {
                            k = m + 1;
                        }
                    }
                    _ => {}
                }
                let mut found = false;
                while k + 1 < n {
                    if data[k] == 0x45
                        && data[k + 1] == 0x49
                        && (k + 2 >= n || matches!(data[k + 2], b' ' | b'\n' | b'\r'))
                    {
                        i = k + 2;
                        found = true;
                        break;
                    }
                    k += 1;
                }
                if !found {
                    i = n;
                }
                if let Some(bm) = bi_mark.take() {
                    opnds.truncate(bm.min(opnds.len()));
                    opnds.push(Opnd::Other);
                    while opnds.len() > 1 {
                        non_processed.push(opnds.remove(0));
                    }
                    opnds.clear();
                } else {
                    non_processed.append(&mut opnds);
                }
                continue;
            }
            if !variable && opnds.len() != need {
                while opnds.len() > need {
                    non_processed.push(opnds.remove(0));
                }
                while opnds.len() < need {
                    match non_processed.pop() {
                        Some(o) => opnds.insert(0, o),
                        None => break,
                    }
                }
                if opnds.len() < need {
                    opnds.clear();
                    continue;
                }
            }
            if is_flush_op(tok) {
                flush_id += 1;
                if tok == b"q" {
                    font_stack.push(cur_font.clone());
                    tz_stack.push(cur_tz);
                } else if tok == b"Q" {
                    if let Some(f) = font_stack.pop() {
                        cur_font = f;
                    }
                    if let Some(t) = tz_stack.pop() {
                        cur_tz = t;
                    }
                } else if tok == b"Do" {
                    if let Some(Opnd::Name(nm)) = opnds.first() {
                        out.paints.push((
                            out.flush_ids.len(),
                            nm.clone(),
                            cur_font.clone(),
                            cur_tz,
                        ));
                    }
                }
            } else if tok == b"Tf" {
                cur_font = match opnds.first() {
                    Some(Opnd::Name(nm)) => Some(nm.clone()),
                    _ => None,
                };
            } else if tok == b"Tz" {
                if let Some(Opnd::Num(v)) = opnds.first() {
                    cur_tz = v / 100.0;
                }
            } else if matches!(tok, b"Tj" | b"TJ" | b"'" | b"\"") {
                let slot = if tok == b"\"" {
                    opnds.get(2)
                } else {
                    opnds.first()
                };
                let units: Vec<u32> = match (tok, slot) {
                    (b"TJ", Some(Opnd::Arr(u))) | (_, Some(Opnd::Str(u))) => u.clone(),
                    _ => Vec::new(),
                };
                if !units.is_empty() {
                    out.flush_ids.push(flush_id);
                    out.fonts.push(cur_font.clone());
                    out.tzs.push(cur_tz);
                    out.units.push(units);
                }
            }
            opnds.clear();
        }
    }
    out
}

/// ref: content_stream.py::_page_vertical_resource_names
fn page_vertical_resource_names(pdf: &PdfDoc, idx: usize) -> HashSet<Vec<u8>> {
    let mut names = HashSet::new();
    for (xref, _subtype, _base, resname, enc) in pdf.page_fonts(idx) {
        let rn: Vec<u8> = resname
            .chars()
            .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' })
            .collect();
        if enc == "V" || enc.ends_with("-V") {
            names.insert(rn);
            continue;
        }
        let (t, v) = pdf.xref_get_key(xref, "Encoding");
        if t == "xref" {
            let Some(id) = v
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<i64>().ok())
            else {
                continue;
            };
            let (wt, wv) = pdf.xref_get_key(id, "WMode");
            if wt == "int" || wt == "real" {
                let w: f64 = wv
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(f64::NAN);
                if w.fract() == 0.0 && w != 0.0 {
                    names.insert(rn);
                }
            }
        }
    }
    names
}

/// The content-stream half of `_page_pass1`: vertical tags, Tz tags and font-Unicode repair.
pub fn tag_page(
    pdf: &mut PdfDoc,
    idx: usize,
    chars: &mut Vec<RawChar>,
    objects: &mut [TextObj],
    cache: &mut FontMapCache,
) {
    let mut show_fonts: Vec<Option<Vec<u8>>> = Vec::new();
    let mut show_tzs: Vec<f64> = Vec::new();
    let mut vert: HashSet<Vec<u8>> = HashSet::new();
    if idx < pdf.page_count() {
        show_fonts = tokenize_show_operators(&pdf.read_contents(idx), 1.0).fonts;
        vert = page_vertical_resource_names(pdf, idx);
        if let Some(codes) = page_show_codes(pdf, idx) {
            if !codes.is_empty() {
                show_tzs = codes.iter().map(|c| c.2).collect();
                if !chars.is_empty() {
                    apply_font_unicode(chars, objects, &codes, pdf, cache);
                }
            }
        }
    }
    // ref: content_stream.py::_assign_vertical_tags
    if !objects.is_empty()
        && !vert.is_empty()
        && !show_fonts.is_empty()
        && show_fonts.len() == objects.len()
    {
        for (o, f) in objects.iter_mut().zip(&show_fonts) {
            if f.as_ref().is_some_and(|f| vert.contains(f)) {
                o.vertical = true;
            }
        }
    }
    // ref: content_stream.py::_assign_show_tz
    if !objects.is_empty() && !show_tzs.is_empty() && show_tzs.len() == objects.len() {
        for (o, t) in objects.iter_mut().zip(&show_tzs) {
            o.tz = *t;
        }
    }
}
