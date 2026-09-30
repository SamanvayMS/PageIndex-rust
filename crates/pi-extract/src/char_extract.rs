//! Raw textpage char extraction, Type-3 sizing and page viewport handling.
//!
//! ref: pageindex/flash/parser_pdfium_charlevel/char_extract.py
#![allow(unsafe_code)]

use std::collections::HashMap;

use pdfium_render::prelude::*;
use pi_pycompat::pyround;

use crate::geometry::{build_obj_index, collect_text_objs, find_obj_for_char, latin1_name};
use crate::model::{FinChar, RawChar, TextObj, VPen};
use crate::text_normalize::{is_invisible_format_mark, is_whitespace, is_zero_width_diacritic};

/// ref: char_extract.py::_extract_raw_chars
pub fn extract_raw_chars(
    b: &dyn PdfiumLibraryBindings,
    page: FPDF_PAGE,
    tp: FPDF_TEXTPAGE,
) -> (Vec<RawChar>, Vec<TextObj>) {
    let objects = collect_text_objs(b, page);
    if objects.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let index = build_obj_index(&objects);
    let mut raw_chars = Vec::new();
    let mut name_buf = [0u8; 256];
    let mut name_cache: HashMap<Vec<u8>, String> = HashMap::new();
    let mut last_obj: Option<usize> = None;
    // SAFETY: `tp` is a live text page of `page`; every out-pointer is a valid local.
    unsafe {
        let count = b.FPDFText_CountChars(tp);
        let mut skip_next = false;
        for i in 0..count.max(0) {
            if skip_next {
                skip_next = false;
                continue;
            }
            // c_uint result: never negative, the reference's `< 0` guard is inert.
            let mut cp = b.FPDFText_GetUnicode(tp, i);
            if (0xD800..=0xDBFF).contains(&cp) && i + 1 < count {
                let low = b.FPDFText_GetUnicode(tp, i + 1);
                if (0xDC00..=0xDFFF).contains(&low) {
                    cp = ((cp & 0x3FF) << 10) + (low & 0x3FF) + 0x10000;
                    skip_next = true;
                }
            }
            if (0xD800..=0xDFFF).contains(&cp) {
                cp = 0xFFFD;
            }
            let ch = char::from_u32(cp).unwrap_or('\u{FFFD}').to_string();
            let is_ws = is_whitespace(cp);
            let is_gen = b.FPDFText_IsGenerated(tp, i) == 1;
            if is_gen && !is_ws {
                continue;
            }
            let (mut ox, mut oy) = (0f64, 0f64);
            b.FPDFText_GetCharOrigin(tp, i, &mut ox, &mut oy);
            let (mut cl, mut cr, mut cb, mut ct) = (0f64, 0f64, 0f64, 0f64);
            b.FPDFText_GetCharBox(tp, i, &mut cl, &mut cr, &mut cb, &mut ct);
            let cx = if cr > cl { (cl + cr) / 2.0 } else { ox };
            let cy = if ct > cb { (ct + cb) / 2.0 } else { oy };
            let mut loose = FS_RECTF {
                left: 0.0,
                top: 0.0,
                right: 0.0,
                bottom: 0.0,
            };
            let (ll, lr, cell_top, cell_bot);
            if b.FPDFText_GetLooseCharBox(tp, i, &mut loose) != 0 && loose.right > loose.left {
                ll = loose.left as f64;
                lr = loose.right as f64;
                cell_top = loose.top as f64;
                cell_bot = loose.bottom as f64;
            } else {
                ll = cl;
                lr = cr;
                cell_top = ct;
                cell_bot = cb;
            }
            let obj = find_obj_for_char(b, &objects, &index, cx, cy, 1.0, tp, i)
                .or_else(|| find_obj_for_char(b, &objects, &index, ox, oy, 1.0, tp, i))
                .or_else(|| find_obj_for_char(b, &objects, &index, ox, oy, 5.0, tp, i))
                .or(last_obj);
            let Some(obj) = obj else { continue };
            last_obj = Some(obj);
            let mut flags: i32 = 0;
            let n = b.FPDFText_GetFontInfo(tp, i, name_buf.as_mut_ptr() as *mut _, 256, &mut flags)
                as usize;
            let font_name = if n > 1 {
                let key = name_buf[..n.min(256)].to_vec();
                name_cache
                    .entry(key)
                    .or_insert_with(|| latin1_name(&name_buf, n))
                    .clone()
            } else {
                objects[obj].font_name.clone()
            };
            let top = oy + objects[obj].fs_eff;
            let mut w: f32 = 0.0;
            b.FPDFFont_GetGlyphWidth(objects[obj].font, cp, objects[obj].fs_raw as f32, &mut w);
            raw_chars.push(RawChar {
                i: i as f64,
                ch,
                is_gen,
                is_ws,
                is_mn: is_zero_width_diacritic(cp),
                is_cf: is_invisible_format_mark(cp),
                ox,
                oy,
                left: ll,
                right: lr,
                top,
                bottom: oy,
                box_top: top,
                box_bottom: cb,
                cell_top,
                cell_bot,
                w_raw: w as f64,
                w_synth: None,
                obj,
                font_name,
                drop: false,
            });
        }
    }
    (raw_chars, objects)
}

/// ref: char_extract.py::_accumulate_type3_extents (keyed by FPDF_FONT address).
pub fn accumulate_type3_extents(
    chars: &[RawChar],
    objects: &[TextObj],
    acc: &mut Vec<(usize, [f64; 2])>,
) {
    for c in chars {
        let o = &objects[c.obj];
        if o.fs_raw >= 1.5 || o.scale_y >= 1.5 || c.is_ws {
            continue;
        }
        let top = c.box_top - c.oy;
        let bot = c.box_bottom - c.oy;
        if top <= bot {
            continue;
        }
        match acc.iter_mut().find(|(k, _)| *k == o.font_key) {
            None => acc.push((o.font_key, [top, bot])),
            Some((_, e)) => {
                if top > e[0] {
                    e[0] = top;
                }
                if bot < e[1] {
                    e[1] = bot;
                }
            }
        }
    }
}

/// ref: char_extract.py::_type3_size_by_font
pub fn type3_size_by_font(acc: &[(usize, [f64; 2])]) -> HashMap<usize, f64> {
    acc.iter()
        .filter(|(_, [t, b])| t > b)
        .map(|(k, [t, b])| (*k, pyround::g6(t - b)))
        .collect()
}

/// ref: char_extract.py::_apply_type3_sizes
pub fn apply_type3_sizes(
    chars: &mut [RawChar],
    objects: &mut [TextObj],
    size_by_font: &HashMap<usize, f64>,
) {
    if size_by_font.is_empty() {
        return;
    }
    for c in chars.iter_mut() {
        let o = &mut objects[c.obj];
        if o.fs_raw >= 1.5 || o.scale_y >= 1.5 {
            continue;
        }
        if let Some(&fs) = size_by_font.get(&o.font_key) {
            if fs != 0.0 {
                o.fs_eff = fs;
                c.top = c.oy + fs;
            }
        }
    }
}

/// ref: char_extract.py::_finalize_chars
pub fn finalize_chars(chars: &[RawChar], objects: &[TextObj]) -> Vec<FinChar> {
    let mut out = Vec::with_capacity(chars.len());
    for (k, c) in chars.iter().enumerate() {
        if c.drop {
            continue;
        }
        let o = &objects[c.obj];
        let glyph_w = if let Some(w) = c.w_synth {
            w
        } else if o.fs_raw >= 1.5 || o.scale_y >= 1.5 {
            c.w_raw * o.scale_x
        } else {
            match chars.get(k + 1) {
                Some(n) if n.obj == c.obj && (n.oy - c.oy).abs() < 0.5 && n.ox > c.ox => {
                    n.ox - c.ox
                }
                _ => {
                    let scale = if o.fs_raw > 0.0 {
                        o.fs_eff / o.fs_raw
                    } else {
                        1.0
                    };
                    c.w_raw * scale
                }
            }
        };
        let vpen = o.vertical.then(|| VPen {
            x: (c.left + c.right) / 2.0,
            y: c.cell_top.max(c.cell_bot),
            after: c.cell_top.min(c.cell_bot),
        });
        out.push(FinChar {
            ch: c.ch.clone(),
            is_ws: c.is_ws,
            is_mn: c.is_mn,
            is_cf: c.is_cf,
            ox: c.ox,
            oy: c.oy,
            glyph_w,
            fs: o.fs_eff,
            fs_x: if o.scale_x > 0.0 {
                o.fs_raw * o.scale_x
            } else {
                o.fs_eff
            },
            left: c.ox,
            right: c.right,
            top: c.top,
            bottom: c.bottom,
            font_name: c.font_name.clone(),
            font_key: o.font_key,
            weight: o.weight,
            obj: c.obj,
            vpen,
        });
    }
    out
}

fn norm_box(bx: Option<[f64; 4]>) -> Option<[f64; 4]> {
    let [x0, y0, x1, y1] = bx?;
    let n = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)];
    (n[2] - n[0] > 0.0 && n[3] - n[1] > 0.0).then_some(n)
}

/// ref: char_extract.py::_page_view_rect — CropBox clamped to MediaBox (inherited boxes first,
/// PDFium's non-inheriting getters as fallback, US-Letter last).
pub fn page_view_rect(
    b: &dyn PdfiumLibraryBindings,
    page: FPDF_PAGE,
    med_raw: Option<[f64; 4]>,
    crop_raw: Option<[f64; 4]>,
) -> [f64; 4] {
    let get = |media: bool| -> Option<[f64; 4]> {
        let (mut l, mut bo, mut r, mut t) = (0f32, 0f32, 0f32, 0f32);
        // SAFETY: live page handle.
        let ok = unsafe {
            if media {
                b.FPDFPage_GetMediaBox(page, &mut l, &mut bo, &mut r, &mut t)
            } else {
                b.FPDFPage_GetCropBox(page, &mut l, &mut bo, &mut r, &mut t)
            }
        };
        if ok != 0 {
            Some([l as f64, bo as f64, r as f64, t as f64])
        } else if media {
            // pypdfium2 get_mediabox falls back to US-Letter when absent
            Some([0.0, 0.0, 612.0, 792.0])
        } else {
            None
        }
    };
    let med = norm_box(med_raw)
        .or_else(|| norm_box(get(true)))
        .unwrap_or([0.0, 0.0, 612.0, 792.0]);
    // pypdfium2 get_cropbox falls back to the mediabox when absent
    let crop = norm_box(crop_raw).or_else(|| norm_box(get(false).or_else(|| get(true))));
    let Some(crop) = crop else { return med };
    if crop == med {
        return med;
    }
    let (x0, y0) = (crop[0].max(med[0]), crop[1].max(med[1]));
    let (x1, y1) = (crop[2].min(med[2]), crop[3].min(med[3]));
    if x1 - x0 <= 0.0 || y1 - y0 <= 0.0 {
        return med;
    }
    [x0, y0, x1, y1]
}

/// ref: char_extract.py::_off_page
pub fn off_page(ox: f64, oy: f64, view_box: Option<&[f64; 4]>) -> bool {
    let Some(vb) = view_box else { return false };
    let dx = ox - vb[0];
    let dy = oy - vb[1];
    dx < 0.0 || dx > vb[2] || dy < 0.0 || dy > vb[3]
}
