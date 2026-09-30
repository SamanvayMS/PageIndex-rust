//! Transform matrices, text-object collection and char→object mapping.
//!
//! ref: pageindex/flash/parser_pdfium_charlevel/geometry.py
#![allow(unsafe_code)]

use std::collections::HashMap;

use pdfium_render::prelude::*;
use pi_pycompat::pyround;

use crate::model::TextObj;

type Mtx = [f64; 6];
const IDENT: Mtx = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// ref: geometry.py::_obj_rotation
pub fn obj_rotation(a: f64, b: f64, c: f64, d: f64) -> i32 {
    let xs = a.hypot(b);
    let ys = c.hypot(d);
    if xs < 1e-9 || ys < 1e-9 {
        return 0;
    }
    let eps = 1e-3;
    if b.abs() < eps * xs && c.abs() < eps * ys {
        return if a >= 0.0 { 0 } else { 180 };
    }
    if a.abs() < eps * xs && d.abs() < eps * ys {
        return if b > 0.0 { 90 } else { 270 };
    }
    -1
}

/// ref: geometry.py::_xf_point
fn xf_point(m: &Mtx, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// ref: geometry.py::_compose_mtx (apply m1 first, then m2)
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

fn fs_matrix(m: &FS_MATRIX) -> Mtx {
    [
        m.a as f64, m.b as f64, m.c as f64, m.d as f64, m.e as f64, m.f as f64,
    ]
}

fn zero_matrix() -> FS_MATRIX {
    FS_MATRIX {
        a: 0.0,
        b: 0.0,
        c: 0.0,
        d: 0.0,
        e: 0.0,
        f: 0.0,
    }
}

/// Decode a PDFium name buffer the way the reference does:
/// `bytes(buf[:n]).decode("latin-1").rstrip("\x00")`.
pub fn latin1_name(buf: &[u8], n: usize) -> String {
    let s: String = buf[..n.min(buf.len())].iter().map(|&b| b as char).collect();
    s.trim_end_matches('\0').to_string()
}

/// ref: geometry.py::_collect_text_objs
pub fn collect_text_objs(b: &dyn PdfiumLibraryBindings, page: FPDF_PAGE) -> Vec<TextObj> {
    // (raw object, ancestor matrix, inside a Form XObject)
    let mut raw: Vec<(FPDF_PAGEOBJECT, Mtx, bool)> = Vec::new();
    fn walk(
        b: &dyn PdfiumLibraryBindings,
        page: FPDF_PAGE,
        parent: Option<FPDF_PAGEOBJECT>,
        anc: Mtx,
        in_form: bool,
        depth: u32,
        out: &mut Vec<(FPDF_PAGEOBJECT, Mtx, bool)>,
    ) {
        // SAFETY: handles come from PDFium for a live page.
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
                let typ = b.FPDFPageObj_GetType(obj);
                if typ == FPDF_PAGEOBJ_TEXT as i32 {
                    out.push((obj, anc, in_form));
                } else if typ == FPDF_PAGEOBJ_FORM as i32 && depth < 10 {
                    let mut m = zero_matrix();
                    b.FPDFPageObj_GetMatrix(obj, &mut m);
                    walk(
                        b,
                        page,
                        Some(obj),
                        compose(&fs_matrix(&m), &anc),
                        true,
                        depth + 1,
                        out,
                    );
                }
            }
        }
    }
    walk(b, page, None, IDENT, false, 0, &mut raw);

    let mut objects = Vec::new();
    let mut name_buf = [0u8; 256];
    for (obj, anc, in_form) in raw {
        // SAFETY: `obj` is a live text object of `page`.
        unsafe {
            let font = b.FPDFTextObj_GetFont(obj);
            if font.is_null() {
                continue;
            }
            let mut sz: f32 = 0.0;
            b.FPDFTextObj_GetFontSize(obj, &mut sz);
            let fs_raw = sz as f64;
            let mut m = zero_matrix();
            b.FPDFPageObj_GetMatrix(obj, &mut m);
            let c = compose(&fs_matrix(&m), &anc);
            let (ma, mb, mc, md) = (c[0], c[1], c[2], c[3]);
            let sx = (ma * ma + mb * mb).sqrt();
            let scale_x = if sx == 0.0 { 1.0 } else { sx };
            let sy = (mc * mc + md * md).sqrt();
            let scale_y = if sy == 0.0 { 1.0 } else { sy };
            let (mut bl, mut bb, mut br, mut bt) = (0f32, 0f32, 0f32, 0f32);
            if b.FPDFPageObj_GetBounds(obj, &mut bl, &mut bb, &mut br, &mut bt) == 0 {
                continue;
            }
            let (bl, bb, br, bt) = (bl as f64, bb as f64, br as f64, bt as f64);
            let (x00, y00) = xf_point(&anc, bl, bb);
            let (x01, y01) = xf_point(&anc, bl, bt);
            let (x10, y10) = xf_point(&anc, br, bb);
            let (x11, y11) = xf_point(&anc, br, bt);
            let ol = x00.min(x01).min(x10).min(x11);
            let or = x00.max(x01).max(x10).max(x11);
            let ob = y00.min(y01).min(y10).min(y11);
            let ot = y00.max(y01).max(y10).max(y11);
            let ink_h = (ot - ob).max(0.0);
            // ref: the Form-XObject branch and the normal-text branch both yield Tfs * scale.
            let fs_eff =
                if (in_form && fs_raw > 0.0 && scale_y > 0.0) || (fs_raw >= 1.5 && scale_y > 0.0) {
                    fs_raw * scale_y
                } else if scale_y >= 1.5 {
                    scale_y
                } else if fs_raw >= 1.5 {
                    fs_raw
                } else {
                    ink_h.max(1.0)
                };
            let fs_eff = pyround::g6(fs_eff);
            let n = b.FPDFFont_GetBaseFontName(font, name_buf.as_mut_ptr() as *mut _, 256);
            let font_name = if n > 1 {
                latin1_name(&name_buf, n)
            } else {
                String::new()
            };
            let weight = b.FPDFFont_GetWeight(font);
            objects.push(TextObj {
                font,
                font_key: font as usize,
                fs_raw,
                scale_x,
                scale_y,
                fs_eff,
                l: ol,
                r: or,
                b: ob,
                t: ot,
                area: ((or - ol) * (ot - ob)).max(0.0),
                font_name,
                weight,
                rot: obj_rotation(ma, mb, mc, md),
                mtx: [ma, mb, mc, md],
                page_order: objects.len(),
                vertical: false,
                tz: 1.0,
            });
        }
    }
    objects
}

/// ref: geometry.py::_build_obj_index — padded integer-y buckets.
pub fn build_obj_index(objects: &[TextObj]) -> HashMap<i64, Vec<usize>> {
    let mut index: HashMap<i64, Vec<usize>> = HashMap::new();
    for (k, o) in objects.iter().enumerate() {
        let lo = o.b.floor() as i64 - 6;
        let hi = o.t.ceil() as i64 + 6;
        for y in lo..=hi {
            index.entry(y).or_default().push(k);
        }
    }
    index
}

/// ref: geometry.py::_char_render_fs
fn char_render_fs(b: &dyn PdfiumLibraryBindings, tp: FPDF_TEXTPAGE, idx: i32) -> f64 {
    let mut m = zero_matrix();
    // SAFETY: live text page, in-range index.
    if unsafe { b.FPDFText_GetMatrix(tp, idx, &mut m) } != 0 {
        let (c, d) = (m.c as f64, m.d as f64);
        return (c * c + d * d).sqrt();
    }
    0.0
}

/// Python `min(seq, key=...)` over tuple keys: first minimal element wins.
fn min_by_key2(cands: &[usize], key: impl Fn(usize) -> (f64, f64)) -> usize {
    let mut best = cands[0];
    let mut bk = key(best);
    for &c in &cands[1..] {
        let k = key(c);
        if k.0 < bk.0 || (k.0 == bk.0 && k.1 < bk.1) {
            best = c;
            bk = k;
        }
    }
    best
}

/// ref: geometry.py::_find_obj_for_char
#[allow(clippy::too_many_arguments)]
pub fn find_obj_for_char(
    b: &dyn PdfiumLibraryBindings,
    objects: &[TextObj],
    index: &HashMap<i64, Vec<usize>>,
    x: f64,
    y: f64,
    tol: f64,
    tp: FPDF_TEXTPAGE,
    char_idx: i32,
) -> Option<usize> {
    let bucket = pyround::round0(y) as i64;
    let mut cands: Vec<usize> = Vec::new();
    if let Some(list) = index.get(&bucket) {
        for &k in list {
            let o = &objects[k];
            if o.l - tol <= x && x <= o.r + tol && o.b - tol <= y && y <= o.t + tol {
                cands.push(k);
            }
        }
    }
    match cands.len() {
        0 => return None,
        1 => return Some(cands[0]),
        _ => {}
    }
    let render = char_render_fs(b, tp, char_idx);
    if render > 0.0 {
        return Some(min_by_key2(&cands, |k| {
            ((objects[k].fs_eff - render).abs(), objects[k].area)
        }));
    }
    // SAFETY: live text page, in-range index.
    let char_fs = unsafe { b.FPDFText_GetFontSize(tp, char_idx) };
    if char_fs > 0.0 {
        return Some(min_by_key2(&cands, |k| {
            (
                (objects[k].fs_raw - char_fs).abs() / char_fs.max(0.01),
                objects[k].area,
            )
        }));
    }
    Some(min_by_key2(&cands, |k| (objects[k].area, 0.0)))
}
