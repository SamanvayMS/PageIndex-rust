//! Raw PDF object access standing in for the reference's PyPDF2 adapter.
//!
//! ref: parser_pdfium_charlevel/pdf_objects.py (`_PdfDoc`, `_pdf_typed`, `_pdf_obj_str`).
//! Values are rendered to the same PDF-syntax strings PyPDF2 3.0.1 produces, because the
//! reference regex-scans those strings. Reals: lopdf keeps them as f32, so they are rendered
//! from the f32's shortest round-trip decimal (exact for tokens with <= 7 significant digits;
//! see parity/KNOWN_DIFFS.md).

use std::collections::HashMap;

use lopdf::{Dictionary, Document, Object, ObjectId};

/// `(type, value-string)` as returned by `_PdfDoc.xref_get_key`.
pub type Typed = (&'static str, String);

fn null() -> Typed {
    ("null", "null".into())
}

pub struct PdfDoc {
    doc: Document,
    pages: Vec<ObjectId>,
    virtual_objs: HashMap<i64, Object>,
}

/// Render a real the way `str(FloatObject(token))` would for a <= 7-significant-digit token.
pub fn real_str(v: f32) -> String {
    let s = format!("{v}");
    // Decimal str keeps the token's form; integral tokens like "612" render without ".0"
    // only if the source had no fraction, which f32 cannot tell. Consumers parse it back
    // with float(), so the numeric value is what matters.
    s
}

/// PyPDF2 decodes #XX escapes in names and then UTF-8 (falling back to latin-1-ish).
fn name_str(raw: &[u8]) -> String {
    let dec = decode_pdf_name(raw);
    match String::from_utf8(dec.clone()) {
        Ok(s) => s,
        Err(_) => dec.iter().map(|&b| b as char).collect(),
    }
}

/// ref: pdf_objects.py::_decode_pdf_name
pub fn decode_pdf_name(raw: &[u8]) -> Vec<u8> {
    if !raw.contains(&b'#') {
        return raw.to_vec();
    }
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'#' && i + 2 < raw.len() {
            if let Ok(s) = std::str::from_utf8(&raw[i + 1..i + 3]) {
                if let Ok(v) = u8::from_str_radix(s, 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(raw[i]);
        i += 1;
    }
    out
}

/// `str(TextStringObject)`: UTF-16BE with BOM, else PDFDocEncoding (approximated as latin-1).
fn string_str(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let units: Vec<u16> = bytes[2..]
            .chunks(2)
            .map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    bytes.iter().map(|&b| b as char).collect()
}

impl PdfDoc {
    pub fn load(bytes: &[u8]) -> anyhow::Result<Self> {
        let doc = Document::load_mem(bytes)?;
        let pages = doc.get_pages().into_values().collect();
        Ok(Self {
            doc,
            pages,
            virtual_objs: HashMap::new(),
        })
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// `page.indirect_reference.idnum`
    pub fn page_xref(&self, idx: usize) -> i64 {
        self.pages[idx].0 as i64
    }

    pub fn register_virtual(&mut self, obj: Object) -> i64 {
        let vid = -(self.virtual_objs.len() as i64 + 1);
        self.virtual_objs.insert(vid, obj);
        vid
    }

    /// `IndirectObject(xref, 0, reader).get_object()` (generation hard-coded to 0).
    pub fn resolve_xref(&self, xref: i64) -> Option<&Object> {
        if xref < 0 {
            return self.virtual_objs.get(&xref);
        }
        self.doc.get_object((xref as u32, 0)).ok()
    }

    fn deref<'a>(&'a self, o: &'a Object) -> Option<&'a Object> {
        match o {
            Object::Reference(id) => self.doc.get_object(*id).ok(),
            other => Some(other),
        }
    }

    fn as_dict<'a>(&'a self, o: &'a Object) -> Option<&'a Dictionary> {
        match self.deref(o)? {
            Object::Dictionary(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }

    /// ref: pdf_objects.py::_PdfDoc.xref_get_key
    pub fn xref_get_key(&self, xref: i64, key: &str) -> Typed {
        let Some(mut cur) = self.resolve_xref(xref) else {
            return null();
        };
        let parts: Vec<&str> = key.split('/').collect();
        for (k, part) in parts.iter().enumerate() {
            let Some(d) = self.as_dict(cur) else {
                return null();
            };
            let Ok(v) = d.get(part.as_bytes()) else {
                return null();
            };
            if k == parts.len() - 1 {
                return self.typed(v);
            }
            match self.deref(v) {
                Some(o) => cur = o,
                None => return null(),
            }
        }
        self.typed(cur)
    }

    /// ref: pdf_objects.py::_pdf_typed
    pub fn typed(&self, v: &Object) -> Typed {
        match v {
            Object::Reference((id, g)) => ("xref", format!("{id} {g} R")),
            Object::Name(n) => ("name", format!("/{}", name_str(n))),
            Object::Boolean(b) => ("bool", if *b { "true".into() } else { "false".into() }),
            Object::Real(r) => ("real", real_str(*r)),
            Object::Integer(i) => ("int", i.to_string()),
            Object::Dictionary(_) | Object::Stream(_) => ("dict", self.obj_str(v)),
            Object::Array(_) => ("array", self.obj_str(v)),
            Object::String(s, _) => ("string", string_str(s)),
            Object::Null => ("string", "None".into()),
        }
    }

    /// ref: pdf_objects.py::_pdf_tok
    fn tok(&self, v: &Object) -> String {
        match v {
            Object::Reference((id, g)) => format!("{id} {g} R"),
            Object::Name(n) => format!("/{}", name_str(n)),
            Object::Boolean(b) => if *b { "true" } else { "false" }.into(),
            Object::Dictionary(_) | Object::Stream(_) => self.obj_str(v),
            Object::Array(a) => format!(
                "[ {} ]",
                a.iter().map(|x| self.tok(x)).collect::<Vec<_>>().join(" ")
            ),
            Object::Real(r) => real_str(*r),
            Object::Integer(i) => i.to_string(),
            Object::String(s, _) => string_str(s),
            Object::Null => "None".into(),
        }
    }

    /// ref: pdf_objects.py::_pdf_obj_str (indirect refs resolved at top level only).
    pub fn obj_str(&self, v: &Object) -> String {
        let v = self.deref(v).unwrap_or(v);
        match v {
            Object::Dictionary(_) | Object::Stream(_) => {
                let d = match v {
                    Object::Dictionary(d) => d,
                    Object::Stream(s) => &s.dict,
                    _ => unreachable!(),
                };
                let mut parts = vec!["<<".to_string()];
                for (k, val) in d.iter() {
                    parts.push(format!("/{}", name_str(k)));
                    parts.push(self.tok(val));
                }
                parts.push(">>".into());
                parts.join(" ")
            }
            Object::Array(a) => format!(
                "[ {} ]",
                a.iter().map(|x| self.tok(x)).collect::<Vec<_>>().join(" ")
            ),
            other => self.tok(other),
        }
    }

    /// ref: _PdfDoc.xref_object
    pub fn xref_object(&self, xref: i64) -> Option<String> {
        self.resolve_xref(xref).map(|o| self.obj_str(o))
    }

    /// ref: _PdfDoc.xref_stream (decoded stream data)
    pub fn xref_stream(&self, xref: i64) -> Option<Vec<u8>> {
        match self.resolve_xref(xref)? {
            Object::Stream(s) => stream_data(s),
            _ => None,
        }
    }

    /// ref: _PdfPage.read_contents — a stream's data, or an array's streams joined with b" ".
    pub fn read_contents(&self, idx: usize) -> Vec<u8> {
        let Ok(page) = self.doc.get_dictionary(self.pages[idx]) else {
            return Vec::new();
        };
        let Ok(c) = page.get(b"Contents") else {
            return Vec::new();
        };
        match self.deref(c) {
            Some(Object::Stream(s)) => stream_data(s).unwrap_or_default(),
            Some(Object::Array(a)) => {
                let parts: Vec<Vec<u8>> = a
                    .iter()
                    .filter_map(|x| match self.deref(x) {
                        Some(Object::Stream(s)) => stream_data(s),
                        _ => None,
                    })
                    .collect();
                parts.join(&b" "[..])
            }
            _ => Vec::new(),
        }
    }

    /// ref: _PdfPage.get_fonts — (idnum, subtype, basefont, resname, encoding-name)
    pub fn page_fonts(&self, idx: usize) -> Vec<(i64, String, String, String, String)> {
        let mut out = Vec::new();
        let Ok(page) = self.doc.get_dictionary(self.pages[idx]) else {
            return out;
        };
        let Some(res) = page.get(b"Resources").ok().and_then(|r| self.as_dict(r)) else {
            return out;
        };
        let Some(fonts) = res.get(b"Font").ok().and_then(|f| self.as_dict(f)) else {
            return out;
        };
        for (k, r) in fonts.iter() {
            let idnum = match r {
                Object::Reference((id, _)) => *id as i64,
                _ => 0,
            };
            let Some(fd) = self.as_dict(r) else { continue };
            let s = |key: &[u8]| -> String {
                match fd.get(key) {
                    Ok(Object::Name(n)) => name_str(n),
                    Ok(o) => self.tok(o).trim_start_matches('/').to_string(),
                    Err(_) => String::new(),
                }
            };
            let enc = match fd.get(b"Encoding") {
                Ok(Object::Name(n)) => name_str(n),
                _ => String::new(),
            };
            out.push((idnum, s(b"Subtype"), s(b"BaseFont"), name_str(k), enc));
        }
        out
    }

    /// Entries of `<owner>/Resources/<sub>` as `(resname bytes, value)`, the dict resolved.
    pub fn resource_entries(&self, owner: i64, sub: &str) -> Option<Vec<(Vec<u8>, Object)>> {
        let node = self.resolve_xref(owner)?;
        let res = self.as_dict(node)?.get(b"Resources").ok()?;
        let d = self.as_dict(res)?.get(sub.as_bytes()).ok()?;
        let d = self.as_dict(d)?;
        Some(d.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
    }

    /// ref: char_extract.py::_inherited_box — /Parent-inherited box as 4 floats.
    pub fn inherited_box(&self, idx: usize, name: &str) -> Option<[f64; 4]> {
        let mut xref = self.page_xref(idx);
        for _ in 0..32 {
            let (t, v) = self.xref_get_key(xref, name);
            if t != "null" {
                if t != "array" {
                    return None;
                }
                let toks: Vec<&str> = v
                    .trim()
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .split_whitespace()
                    .collect();
                if toks.len() != 4 {
                    return None;
                }
                let mut out = [0.0; 4];
                for (o, t) in out.iter_mut().zip(&toks) {
                    *o = t.parse().ok()?;
                }
                return Some(out);
            }
            let (pt, pv) = self.xref_get_key(xref, "Parent");
            if pt != "xref" {
                return None;
            }
            xref = pv.split_whitespace().next()?.parse().ok()?;
        }
        None
    }
}

fn stream_data(s: &lopdf::Stream) -> Option<Vec<u8>> {
    if s.dict.get(b"Filter").is_ok() {
        s.decompressed_content().ok()
    } else {
        Some(s.content.clone())
    }
}
