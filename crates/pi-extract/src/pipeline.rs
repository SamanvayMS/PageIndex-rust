//! Whole-document parse driver. ref: parser_pdfium_charlevel/pipeline.py
#![allow(unsafe_code)]

use std::collections::HashMap;

use anyhow::Result;
use pi_core::PageSpans;

use crate::char_extract::{
    accumulate_type3_extents, apply_type3_sizes, extract_raw_chars, finalize_chars, page_view_rect,
    type3_size_by_font,
};
use crate::merge::merge_text_items;
use crate::model::{RawChar, TextObj};
use crate::pdfium::Document;
use crate::pdfobj::PdfDoc;
use crate::remerge::{remerge_oblique, remerge_rotated, remerge_vertical};
use crate::spans::page_spans;
use crate::unicode_apply::FontMapCache;

struct Pass1 {
    chars: Vec<RawChar>,
    objects: Vec<TextObj>,
    view_box: Option<[f64; 4]>,
    rotation: u16,
}

/// ref: pipeline.py::_page_pass1
fn page_pass1(
    doc: &mut Document,
    pdf: Option<&mut PdfDoc>,
    idx: usize,
    type3: &mut Vec<(usize, [f64; 2])>,
    cache: &mut FontMapCache,
) -> Result<Pass1> {
    let b = doc.b;
    let page = doc.load_page(idx)?;
    // SAFETY: live page; the text page is closed before returning.
    let tp = unsafe { b.FPDFText_LoadPage(page) };
    let (mut chars, mut objects) = extract_raw_chars(b, page, tp);
    let (med, crop) = match pdf.as_deref() {
        Some(p) => (
            p.inherited_box(idx, "MediaBox"),
            p.inherited_box(idx, "CropBox"),
        ),
        None => (None, None),
    };
    let view_box = Some(page_view_rect(b, page, med, crop));
    // SAFETY: live page.
    let rot = unsafe { b.FPDFPage_GetRotation(page) };
    let rotation = match rot {
        1 => 90,
        2 => 180,
        3 => 270,
        _ => 0,
    };
    if let Some(p) = pdf {
        crate::content_stream::tag_page(p, idx, &mut chars, &mut objects, cache);
    }
    accumulate_type3_extents(&chars, &objects, type3);
    // SAFETY: closing the text page we opened.
    unsafe { b.FPDFText_ClosePage(tp) };
    Ok(Pass1 {
        chars,
        objects,
        view_box,
        rotation,
    })
}

/// ref: pipeline.py::_page_pass2 + _page_spans
fn page_pass2(mut p: Pass1, size_by_font: &HashMap<usize, f64>) -> PageSpans {
    apply_type3_sizes(&mut p.chars, &mut p.objects, size_by_font);
    // Restore paint order: real glyphs by (object page_order, textpage index); generated
    // whitespace glued behind the preceding real glyph.
    let n = p.chars.len();
    let mut keys: Vec<(f64, f64, i32, usize)> = vec![(0.0, 0.0, 0, 0); n];
    let mut last: Option<(f64, f64)> = None;
    let mut lead = Vec::new();
    for (k, c) in p.chars.iter().enumerate() {
        if c.is_gen {
            match last {
                None => lead.push(k),
                Some((po, i)) => keys[k] = (po, i, 1, k),
            }
        } else {
            let lk = (p.objects[c.obj].page_order as f64, c.i);
            last = Some(lk);
            keys[k] = (lk.0, lk.1, 0, k);
        }
    }
    for k in lead {
        keys[k] = (-1.0, -1.0, 1, k);
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        let (x, y) = (keys[a], keys[b]);
        x.0.total_cmp(&y.0)
            .then(x.1.total_cmp(&y.1))
            .then(x.2.cmp(&y.2))
            .then(x.3.cmp(&y.3))
    });
    let chars: Vec<RawChar> = order.into_iter().map(|k| p.chars[k].clone()).collect();
    let fin = finalize_chars(&chars, &p.objects);
    let merged = merge_text_items(&fin, &p.objects, p.view_box.as_ref());
    let merged = remerge_rotated(merged, &p.objects);
    let merged = remerge_vertical(merged, &p.objects);
    let merged = remerge_oblique(merged, &fin, &p.objects);
    PageSpans {
        page: 0,
        viewbox: p.view_box,
        rotation: p.rotation,
        spans: page_spans(&merged, &p.objects),
    }
}

/// Extract spans for every page. ref: pipeline.py::parse_charlevel_meta
pub fn extract_pdf_bytes(bytes: Vec<u8>) -> Result<Vec<PageSpans>> {
    let mut pdf = PdfDoc::load(&bytes).ok();
    let mut doc = Document::open(bytes)?;
    let n = doc.page_count();
    let mut type3 = Vec::new();
    let mut cache = FontMapCache::default();
    let mut pass1 = Vec::with_capacity(n);
    for idx in 0..n {
        pass1.push(page_pass1(
            &mut doc,
            pdf.as_mut(),
            idx,
            &mut type3,
            &mut cache,
        )?);
    }
    let size_by_font = type3_size_by_font(&type3);
    let mut out = Vec::with_capacity(n);
    for (idx, p) in pass1.into_iter().enumerate() {
        let mut ps = page_pass2(p, &size_by_font);
        ps.page = idx as u32 + 1;
        out.push(ps);
    }
    drop(doc);
    Ok(out)
}

/// Extract spans from a PDF file.
pub fn extract_pdf(path: &std::path::Path) -> Result<Vec<PageSpans>> {
    extract_pdf_bytes(std::fs::read(path)?)
}
