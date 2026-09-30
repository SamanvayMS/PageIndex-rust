//! Per-page triage deciding which producer handles each page (plan §9.1).
//!
//! Cheap signals per page, from PDFium (text chars, object kinds and bounds) and lopdf (font
//! dictionaries), then a rule-based label. Thresholds live in [`Thresholds`] so they can be
//! tuned on a corpus and persisted with the document metadata.
#![allow(unsafe_code)]

use pdfium_render::prelude::*;
use pi_extract::pdfium::Document;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageLabel {
    /// Usable text layer: text-layer producer only.
    Text,
    /// Image-only page: OCR the full page.
    Scanned,
    /// Text layer present but unreadable: OCR the page, keep the better text.
    Garbled,
    /// Text plus significant image regions: text layer + OCR of the image regions.
    Mixed,
    /// Mostly vector drawing with little text.
    Graphic,
}

impl PageLabel {
    pub fn needs_ocr(self) -> bool {
        matches!(self, Self::Scanned | Self::Garbled | Self::Mixed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSignals {
    pub page: u32,
    pub width: f64,
    pub height: f64,
    /// Non-whitespace textpage chars.
    pub text_chars: usize,
    /// Non-whitespace chars per 10k pt² of page area.
    pub text_density: f64,
    /// Fraction of the page covered by image objects (union, grid-approximated).
    pub image_coverage: f64,
    /// Page-space boxes `[x0, y0, x1, y1]` of image objects larger than 2% of the page.
    pub image_regions: Vec<[f64; 4]>,
    pub text_objects: usize,
    pub path_objects: usize,
    /// Share of chars that are Private Use, U+FFFD/U+FFFE, or control characters.
    pub bad_char_ratio: f64,
    /// Share of letter-bearing tokens that look like words.
    pub word_ratio: f64,
    pub letters: usize,
    pub has_type3_font: bool,
    pub has_font_without_tounicode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thresholds {
    /// Below this many chars a page counts as having no text layer.
    pub min_text_chars: usize,
    /// Image coverage at which a near-textless page is `scanned`.
    pub scanned_image_coverage: f64,
    /// Image coverage at which a text page is `mixed`.
    pub mixed_image_coverage: f64,
    pub garbled_bad_char_ratio: f64,
    pub garbled_word_ratio: f64,
    /// Minimum letters before the word ratio is trusted.
    pub garbled_min_letters: usize,
    /// Path objects above which a near-textless page is `graphic`.
    pub graphic_min_paths: usize,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            min_text_chars: 20,
            scanned_image_coverage: 0.3,
            mixed_image_coverage: 0.25,
            garbled_bad_char_ratio: 0.1,
            garbled_word_ratio: 0.45,
            garbled_min_letters: 80,
            graphic_min_paths: 150,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageTriage {
    pub label: PageLabel,
    pub signals: PageSignals,
}

/// Rule-based label from signals (plan §9.1 routing table).
pub fn classify(s: &PageSignals, t: &Thresholds) -> PageLabel {
    let has_text = s.text_chars >= t.min_text_chars;
    if !has_text {
        if s.image_coverage >= t.scanned_image_coverage {
            return PageLabel::Scanned;
        }
        if s.path_objects >= t.graphic_min_paths {
            return PageLabel::Graphic;
        }
        return PageLabel::Text; // blank or near-blank page: nothing to OCR
    }
    if s.bad_char_ratio > t.garbled_bad_char_ratio
        || (s.letters >= t.garbled_min_letters && s.word_ratio < t.garbled_word_ratio)
    {
        return PageLabel::Garbled;
    }
    if s.image_coverage >= t.mixed_image_coverage {
        return PageLabel::Mixed;
    }
    PageLabel::Text
}

fn is_bad_char(c: char) -> bool {
    let cp = c as u32;
    (0xE000..=0xF8FF).contains(&cp)
        || (0xF0000..=0x10FFFF).contains(&cp)
        || cp == 0xFFFD
        || cp == 0xFFFE
        || (c.is_control() && !c.is_whitespace())
}

fn is_vowel(c: char) -> bool {
    matches!(c.to_ascii_lowercase(), 'a' | 'e' | 'i' | 'o' | 'u' | 'y')
}

/// Scripts written without inter-word spaces or Latin vowels count as word-like per char run.
fn is_unspaced_script(c: char) -> bool {
    let cp = c as u32;
    (0x3040..=0x30FF).contains(&cp) // kana
        || (0x3400..=0x9FFF).contains(&cp) // CJK
        || (0xAC00..=0xD7AF).contains(&cp) // hangul
        || (0x0590..=0x08FF).contains(&cp) // hebrew / arabic
        || (0x0900..=0x0DFF).contains(&cp) // indic
        || (0x0E00..=0x0EFF).contains(&cp) // thai / lao
}

/// (word ratio, letters) over whitespace tokens. Edge punctuation is stripped; a token is
/// word-like when it is short (<= 4 chars, e.g. "n/a", "U.S."), a number with a short suffix
/// ("110th", "FY25"), or >= 70% letters and contains
/// a vowel; tokens in unspaced scripts always count. Garbled text layers show up as long
/// vowel-less or symbol-heavy tokens.
pub fn word_stats(text: &str) -> (f64, usize) {
    let mut words = 0usize;
    let mut tokens = 0usize;
    let mut letters = 0usize;
    for raw in text.split_whitespace() {
        let tok = raw.trim_matches(|c: char| !c.is_alphanumeric());
        let n = tok.chars().count();
        let l = tok.chars().filter(|c| c.is_alphabetic()).count();
        letters += l;
        if l == 0 {
            continue;
        }
        tokens += 1;
        let numeric_with_suffix = l <= 3 && tok.chars().any(|c| c.is_ascii_digit()); // 110th, Q3, FY25
        if tok.chars().any(is_unspaced_script) || n <= 4 || numeric_with_suffix {
            words += 1;
            continue;
        }
        if n <= 30 && l * 10 >= n * 7 && (tok.chars().any(is_vowel) || !tok.is_ascii()) {
            words += 1;
        }
    }
    (
        if tokens == 0 {
            1.0
        } else {
            words as f64 / tokens as f64
        },
        letters,
    )
}

/// Union coverage of boxes over a page, on a 200x200 grid.
fn coverage(boxes: &[[f64; 4]], w: f64, h: f64) -> f64 {
    const G: usize = 200;
    if boxes.is_empty() || w <= 0.0 || h <= 0.0 {
        return 0.0;
    }
    let mut grid = vec![false; G * G];
    for b in boxes {
        let x0 = ((b[0] / w) * G as f64).floor().clamp(0.0, G as f64) as usize;
        let x1 = ((b[2] / w) * G as f64).ceil().clamp(0.0, G as f64) as usize;
        let y0 = ((b[1] / h) * G as f64).floor().clamp(0.0, G as f64) as usize;
        let y1 = ((b[3] / h) * G as f64).ceil().clamp(0.0, G as f64) as usize;
        for y in y0..y1 {
            for x in x0..x1 {
                grid[y * G + x] = true;
            }
        }
    }
    grid.iter().filter(|&&v| v).count() as f64 / (G * G) as f64
}

type Mtx = [f64; 6];

fn compose(m1: &Mtx, m2: &Mtx) -> Mtx {
    [
        m1[0] * m2[0] + m1[1] * m2[2],
        m1[0] * m2[1] + m1[1] * m2[3],
        m1[2] * m2[0] + m1[3] * m2[2],
        m1[2] * m2[1] + m1[3] * m2[3],
        m1[4] * m2[0] + m1[5] * m2[2] + m2[4],
        m1[4] * m2[1] + m1[5] * m2[3] + m2[5],
    ]
}

struct ObjStats {
    text: usize,
    paths: usize,
    images: Vec<[f64; 4]>,
}

fn walk_objects(
    b: &dyn PdfiumLibraryBindings,
    page: FPDF_PAGE,
    parent: Option<FPDF_PAGEOBJECT>,
    anc: Mtx,
    depth: u32,
    st: &mut ObjStats,
) {
    // SAFETY: handles belong to a live page.
    unsafe {
        let n = match parent {
            Some(p) => b.FPDFFormObj_CountObjects(p),
            None => b.FPDFPage_CountObjects(page),
        };
        for i in 0..n.max(0) {
            let obj = match parent {
                Some(p) => b.FPDFFormObj_GetObject(p, i as _),
                None => b.FPDFPage_GetObject(page, i),
            };
            if obj.is_null() {
                continue;
            }
            let typ = b.FPDFPageObj_GetType(obj) as u32;
            if typ == FPDF_PAGEOBJ_TEXT {
                st.text += 1;
            } else if typ == FPDF_PAGEOBJ_PATH {
                st.paths += 1;
            } else if typ == FPDF_PAGEOBJ_IMAGE {
                let (mut l, mut bo, mut r, mut t) = (0f32, 0f32, 0f32, 0f32);
                if b.FPDFPageObj_GetBounds(obj, &mut l, &mut bo, &mut r, &mut t) != 0 {
                    let pts = [(l, bo), (l, t), (r, bo), (r, t)].map(|(x, y)| {
                        let (x, y) = (x as f64, y as f64);
                        (
                            anc[0] * x + anc[2] * y + anc[4],
                            anc[1] * x + anc[3] * y + anc[5],
                        )
                    });
                    let xs = pts.map(|p| p.0);
                    let ys = pts.map(|p| p.1);
                    let fold = |v: [f64; 4], f: fn(f64, f64) -> f64| {
                        v.into_iter().reduce(f).unwrap_or(0.0)
                    };
                    st.images.push([
                        fold(xs, f64::min),
                        fold(ys, f64::min),
                        fold(xs, f64::max),
                        fold(ys, f64::max),
                    ]);
                }
            } else if typ == FPDF_PAGEOBJ_FORM && depth < 10 {
                let mut m = FS_MATRIX {
                    a: 0.0,
                    b: 0.0,
                    c: 0.0,
                    d: 0.0,
                    e: 0.0,
                    f: 0.0,
                };
                b.FPDFPageObj_GetMatrix(obj, &mut m);
                let mm = [m.a, m.b, m.c, m.d, m.e, m.f].map(|v| v as f64);
                walk_objects(b, page, Some(obj), compose(&mm, &anc), depth + 1, st);
            }
        }
    }
}

/// Font flags per page: (Type3 font present, a font without /ToUnicode and a non-standard
/// encoding). Empty when lopdf cannot read the file.
fn font_flags(bytes: &[u8]) -> Vec<(bool, bool)> {
    let Ok(doc) = lopdf::Document::load_mem(bytes) else {
        return Vec::new();
    };
    let deref = |o: &lopdf::Object| -> Option<lopdf::Dictionary> {
        match o {
            lopdf::Object::Reference(id) => doc.get_dictionary(*id).ok().cloned(),
            lopdf::Object::Dictionary(d) => Some(d.clone()),
            _ => None,
        }
    };
    doc.get_pages()
        .values()
        .map(|&pid| {
            let mut t3 = false;
            let mut no_tu = false;
            if let Ok((Some(res), _)) = doc.get_page_resources(pid)
                && let Some(fonts) = res.get(b"Font").ok().and_then(deref)
            {
                for (_, f) in fonts.iter() {
                    let Some(fd) = deref(f) else { continue };
                    let subtype = fd
                        .get(b"Subtype")
                        .ok()
                        .and_then(|s| s.as_name().ok())
                        .unwrap_or(b"");
                    if subtype == b"Type3" {
                        t3 = true;
                    }
                    let has_tu = fd.get(b"ToUnicode").is_ok();
                    let std_enc = matches!(
                        fd.get(b"Encoding").ok().and_then(|e| e.as_name().ok()),
                        Some(b"WinAnsiEncoding" | b"MacRomanEncoding" | b"StandardEncoding")
                    );
                    if !has_tu && (subtype == b"Type0" || (!std_enc && subtype != b"Type1")) {
                        no_tu = true;
                    }
                }
            }
            (t3, no_tu)
        })
        .collect()
}

/// Compute signals and labels for every page of a PDF.
pub fn triage_pdf_bytes(
    bytes: Vec<u8>,
    thresholds: &Thresholds,
) -> anyhow::Result<Vec<PageTriage>> {
    let fonts = font_flags(&bytes);
    let mut doc = Document::open(bytes)?;
    let b = doc.b;
    let mut out = Vec::new();
    for idx in 0..doc.page_count() {
        let page = doc.load_page(idx)?;
        // SAFETY: live page; text page closed below.
        let (w, h, text) = unsafe {
            let w = b.FPDF_GetPageWidthF(page) as f64;
            let h = b.FPDF_GetPageHeightF(page) as f64;
            let tp = b.FPDFText_LoadPage(page);
            let n = b.FPDFText_CountChars(tp);
            let mut s = String::with_capacity(n.max(0) as usize);
            for i in 0..n.max(0) {
                if let Some(c) = char::from_u32(b.FPDFText_GetUnicode(tp, i)) {
                    s.push(c);
                }
            }
            b.FPDFText_ClosePage(tp);
            (w, h, s)
        };
        let mut st = ObjStats {
            text: 0,
            paths: 0,
            images: Vec::new(),
        };
        walk_objects(b, page, None, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], 0, &mut st);
        let visible: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
        let bad = visible.iter().filter(|c| is_bad_char(**c)).count();
        let (word_ratio, letters) = word_stats(&text);
        let area = (w * h).max(1.0);
        let (t3, no_tu) = fonts.get(idx).copied().unwrap_or((false, false));
        let signals = PageSignals {
            page: idx as u32 + 1,
            width: w,
            height: h,
            text_chars: visible.len(),
            text_density: visible.len() as f64 / area * 10_000.0,
            image_coverage: coverage(&st.images, w, h),
            image_regions: st
                .images
                .iter()
                .filter(|r| (r[2] - r[0]) * (r[3] - r[1]) >= 0.02 * area)
                .copied()
                .collect(),
            text_objects: st.text,
            path_objects: st.paths,
            bad_char_ratio: if visible.is_empty() {
                0.0
            } else {
                bad as f64 / visible.len() as f64
            },
            word_ratio,
            letters,
            has_type3_font: t3,
            has_font_without_tounicode: no_tu,
        };
        out.push(PageTriage {
            label: classify(&signals, thresholds),
            signals,
        });
    }
    Ok(out)
}

pub fn triage_pdf(
    path: &std::path::Path,
    thresholds: &Thresholds,
) -> anyhow::Result<Vec<PageTriage>> {
    triage_pdf_bytes(std::fs::read(path)?, thresholds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig() -> PageSignals {
        PageSignals {
            page: 1,
            width: 612.0,
            height: 792.0,
            text_chars: 2000,
            text_density: 41.0,
            image_coverage: 0.0,
            image_regions: vec![],
            text_objects: 50,
            path_objects: 3,
            bad_char_ratio: 0.0,
            word_ratio: 0.95,
            letters: 1800,
            has_type3_font: false,
            has_font_without_tounicode: false,
        }
    }

    #[test]
    fn labels() {
        let t = Thresholds::default();
        assert_eq!(classify(&sig(), &t), PageLabel::Text);
        let s = PageSignals {
            text_chars: 0,
            image_coverage: 0.97,
            ..sig()
        };
        assert_eq!(classify(&s, &t), PageLabel::Scanned);
        let s = PageSignals {
            word_ratio: 0.1,
            ..sig()
        };
        assert_eq!(classify(&s, &t), PageLabel::Garbled);
        let s = PageSignals {
            image_coverage: 0.4,
            ..sig()
        };
        assert_eq!(classify(&s, &t), PageLabel::Mixed);
        let s = PageSignals {
            text_chars: 5,
            path_objects: 900,
            ..sig()
        };
        assert_eq!(classify(&s, &t), PageLabel::Graphic);
    }

    #[test]
    fn words() {
        assert!(word_stats("The quick brown fox jumps over the lazy dog").0 > 0.9);
        assert!(word_stats("xkqzvbt Wrtplmn 7hq3k9#z Fghjklpq zxcvbnm the").0 < 0.5);
        assert!(word_stats("Total n/a n/a (loss) U.S. 110th").0 > 0.9);
        assert!(word_stats("東京は日本の首都です").0 > 0.9);
    }
}
