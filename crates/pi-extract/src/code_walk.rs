//! Resource-dictionary walking, per-page show-code enumeration and the char/target walk.
//!
//! ref: parser_pdfium_charlevel/code_walk.py

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use lopdf::Object;
use regex::Regex;

use crate::content_stream::tokenize_show_operators;
use crate::pdfobj::{PdfDoc, decode_pdf_name};
use crate::pyuni;
use crate::text_normalize::{is_whitespace, normalize_unicodes};

// ref: code_walk.py:42 — Python `\s` also matches U+001C..U+001F.
static RES_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"/([^\s\x1C-\x1F/\[\]<>()]+)[\s\x1C-\x1F]+(\d+)[\s\x1C-\x1F]+\d+[\s\x1C-\x1F]+R")
        .expect("res re")
});

/// `name.encode("latin-1")`; `None` when a char is outside latin-1 (UnicodeEncodeError).
fn latin1(s: &str) -> Option<Vec<u8>> {
    s.chars().map(|c| u8::try_from(c as u32).ok()).collect()
}

/// ref: code_walk.py::_resource_dict_xrefs — `None` models a propagating exception.
pub fn resource_dict_xrefs(
    pdf: &mut PdfDoc,
    owner: i64,
    sub: &str,
) -> Option<HashMap<Vec<u8>, i64>> {
    let mut val = ("null", String::from("null"));
    let mut cursor = owner;
    for _ in 0..32 {
        val = pdf.xref_get_key(cursor, &format!("Resources/{sub}"));
        if val.0 != "null" {
            break;
        }
        if pdf.xref_get_key(cursor, "Resources").0 != "null" {
            break;
        }
        let (pt, pv) = pdf.xref_get_key(cursor, "Parent");
        if pt != "xref" {
            break;
        }
        cursor = pv.split_whitespace().next()?.parse().ok()?;
    }
    let body = match val.0 {
        "xref" => pdf.xref_object(val.1.split_whitespace().next()?.parse().ok()?)?,
        "dict" => val.1.clone(),
        _ => return Some(HashMap::new()),
    };
    let mut out: HashMap<Vec<u8>, i64> = HashMap::new();
    for m in RES_RE.captures_iter(&body) {
        let name = decode_pdf_name(&latin1(&m[1])?);
        out.insert(name, m[2].parse().ok()?);
    }
    // Direct (inline) dict entries become virtual pseudo-xrefs (errors swallowed).
    if let Some(entries) = pdf.resource_entries(cursor, sub) {
        for (k, raw) in entries {
            if matches!(raw, Object::Reference(_))
                || !matches!(raw, Object::Dictionary(_) | Object::Stream(_))
            {
                continue;
            }
            let name = decode_pdf_name(&k);
            if let std::collections::hash_map::Entry::Vacant(e) = out.entry(name) {
                e.insert(pdf.register_virtual(raw));
            }
        }
    }
    Some(out)
}

/// One show op: (font xref, charcode units, horizontal scale).
pub type ShowCode = (Option<i64>, Vec<u32>, f64);

#[allow(clippy::too_many_arguments)]
fn walk(
    pdf: &mut PdfDoc,
    stream: &[u8],
    fonts_res: &HashMap<Vec<u8>, i64>,
    xobjs_res: &HashMap<Vec<u8>, i64>,
    cur_font: Option<i64>,
    cur_tz: f64,
    visited: &HashSet<i64>,
    depth: u32,
    out: &mut Vec<ShowCode>,
) -> Option<()> {
    if depth > 8 {
        return Some(());
    }
    let shows = tokenize_show_operators(stream, cur_tz);
    let mut di = 0usize;
    for k in 0..=shows.units.len() {
        while di < shows.paints.len() && shows.paints[di].0 == k {
            let (_, xname, font_at_do, tz_at_do) = shows.paints[di].clone();
            di += 1;
            let Some(&xref) = xobjs_res.get(&xname) else {
                continue;
            };
            if visited.contains(&xref) {
                continue;
            }
            let (st, sv) = pdf.xref_get_key(xref, "Subtype");
            if st != "name" || sv.trim_start_matches('/') != "Form" {
                continue;
            }
            let sub_fonts = resource_dict_xrefs(pdf, xref, "Font")?;
            let sub_fonts = if sub_fonts.is_empty() {
                fonts_res.clone()
            } else {
                sub_fonts
            };
            let sub_xobjs = resource_dict_xrefs(pdf, xref, "XObject")?;
            let sub_xobjs = if sub_xobjs.is_empty() {
                xobjs_res.clone()
            } else {
                sub_xobjs
            };
            let inherited = font_at_do.as_ref().and_then(|f| fonts_res.get(f).copied());
            let Some(sub_stream) = pdf.xref_stream(xref) else {
                continue;
            };
            let mut v2 = visited.clone();
            v2.insert(xref);
            walk(
                pdf,
                &sub_stream,
                &sub_fonts,
                &sub_xobjs,
                inherited,
                tz_at_do,
                &v2,
                depth + 1,
                out,
            )?;
        }
        if k < shows.units.len() {
            let f = match &shows.fonts[k] {
                Some(name) => fonts_res.get(name).copied(),
                None => cur_font,
            };
            out.push((f, shows.units[k].clone(), shows.tzs[k]));
        }
    }
    Some(())
}

/// ref: code_walk.py::_page_show_codes — every show op in paint order, forms spliced in.
pub fn page_show_codes(pdf: &mut PdfDoc, idx: usize) -> Option<Vec<ShowCode>> {
    let xref = pdf.page_xref(idx);
    let stream = pdf.read_contents(idx);
    let fonts = resource_dict_xrefs(pdf, xref, "Font")?;
    let xobjs = resource_dict_xrefs(pdf, xref, "XObject")?;
    let mut out = Vec::new();
    walk(
        pdf,
        &stream,
        &fonts,
        &xobjs,
        None,
        1.0,
        &HashSet::new(),
        0,
        &mut out,
    )?;
    Some(out)
}

/// ref: code_walk.py::_char_category — (is_ws, is_mn, is_cf)
pub fn char_category(text: &str) -> (bool, bool, bool) {
    let n = text.chars().count();
    for (pos, c) in text.chars().enumerate() {
        let cp = c as u32;
        if pos == 0 && is_whitespace(cp) {
            return (true, false, false);
        }
        let cat = pyuni::category(cp);
        if cat == "Mn" {
            return (false, true, false);
        }
        if cat == "Cf" && pos == n - 1 {
            return (false, false, true);
        }
    }
    (false, false, false)
}

/// Result of `_walk_codes`: (patches, drops, consumed, skips).
pub struct WalkResult {
    pub patches: Vec<(f64, String)>,
    pub drops: Vec<f64>,
    pub consumed: Vec<(f64, usize)>,
    pub skips: Vec<(usize, usize)>,
}

fn slice_eq(text: &[char], pos: usize, pat: &[char]) -> bool {
    // Python `text[pos:pos+len] == pat` (slices clamp at the end).
    let end = (pos + pat.len()).min(text.len());
    pos <= text.len() && text[pos.min(text.len())..end] == *pat
}

/// ref: code_walk.py::_walk_codes. `chars` is (textpage index, char string) in walk order.
pub fn walk_codes(
    chars: &[(f64, String)],
    targets: &[String],
    allow_skips: bool,
) -> Option<WalkResult> {
    // `text` is the concatenation of char strings; positions index code points, and
    // chars[pos] is used as if each char were exactly one code point (as in the reference).
    let text: Vec<char> = chars.iter().flat_map(|(_, s)| s.chars()).collect();
    let tlen = text.len();
    let mut pos = 0usize;
    let mut r = WalkResult {
        patches: Vec::new(),
        drops: Vec::new(),
        consumed: Vec::new(),
        skips: Vec::new(),
    };
    let mut skip_until: isize = -1;
    let mut shift_run = 0;
    let tgts: Vec<Vec<char>> = targets.iter().map(|t| t.chars().collect()).collect();
    let ci = |p: usize| chars.get(p).map(|c| c.0);
    for (i, tok) in tgts.iter().enumerate() {
        if (i as isize) <= skip_until {
            continue;
        }
        if pos >= tlen {
            if allow_skips {
                r.skips.push((i, pos));
                continue;
            }
            return None;
        }
        if slice_eq(&text, pos, tok) {
            for q in pos..pos + tok.len() {
                r.consumed.push((ci(q)?, i));
            }
            pos += tok.len();
            shift_run = 0;
            continue;
        }
        let tok_s: String = tok.iter().collect();
        let norm_table = normalize_unicodes(&tok_s);
        let mut matched = false;
        for cand in [
            norm_table.clone(),
            pyuni::nfkc(&tok_s),
            pyuni::nfkd(&tok_s),
            pyuni::nfd(&tok_s),
        ] {
            let cv: Vec<char> = cand.chars().collect();
            if cand != tok_s && slice_eq(&text, pos, &cv) {
                if norm_table != cand {
                    r.patches.push((ci(pos)?, tok_s.clone()));
                    for q in pos + 1..pos + cv.len() {
                        r.drops.push(ci(q)?);
                    }
                }
                for q in pos..pos + cv.len() {
                    r.consumed.push((ci(q)?, i));
                }
                pos += cv.len();
                matched = true;
                break;
            }
        }
        if matched {
            shift_run = 0;
            continue;
        }
        if allow_skips {
            let mut run = 0usize;
            for skip_len in 1..=tgts.len() - i {
                let anchor =
                    &tgts[(i + skip_len).min(tgts.len())..(i + skip_len + 2).min(tgts.len())];
                if anchor.is_empty() || !anchor.iter().all(|a| a.len() == 1) {
                    break;
                }
                let sv: Vec<char> = anchor.iter().flatten().copied().collect();
                if slice_eq(&text, pos, &sv) {
                    run = skip_len;
                    break;
                }
            }
            if run > 0 {
                for q in i..i + run {
                    r.skips.push((q, pos));
                }
                skip_until = (i + run - 1) as isize;
                shift_run = 0;
                continue;
            }
        }
        if tok.iter().all(|&c| is_whitespace(c as u32)) && !is_whitespace(text[pos] as u32) {
            return None;
        }
        let nxt = tgts.get(i + 1);
        if nxt.is_some_and(|n| slice_eq(&text, pos, n)) || slice_eq(&text, pos + 1, tok) {
            shift_run += 1;
            if shift_run >= 2 {
                return None;
            }
        } else {
            shift_run = 0;
        }
        r.patches.push((ci(pos)?, tok_s));
        r.consumed.push((ci(pos)?, i));
        pos += 1;
    }
    if pos != tlen {
        return None;
    }
    Some(r)
}
