//! Spike A: dump raw per-char PDFium values through pdfium-render's raw bindings.
//!
//! The output mirrors `spikes/pdfium-spike/dump_chars.py`, which uses pypdfium2's raw ctypes API
//! in the same way. Run both against the same PDFium build and the JSON must be identical.
//!
//! Usage: pdfium-spike <libpdfium.so> <file.pdf> [first_page last_page]

use std::ffi::c_void;

use anyhow::{Context, Result, bail};
use pdfium_render::prelude::*;
use serde_json::{Value, json};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        bail!("usage: pdfium-spike <libpdfium.so> <file.pdf> [first_page last_page]");
    }
    // pdfium-render 0.9 keeps its bindings accessor crate-private, so the spike owns the raw
    // bindings object and initialises the library itself.
    let bindings = Pdfium::bind_to_library(&args[1]).context("binding libpdfium")?;
    let b = bindings.as_ref();
    let path = &args[2];

    // SAFETY: raw PDFium calls on handles we own; every handle is closed before return.
    unsafe {
        b.FPDF_InitLibrary();
        let doc = b.FPDF_LoadDocument(path, None);
        if doc.is_null() {
            bail!("FPDF_LoadDocument failed for {path}");
        }
        let n = b.FPDF_GetPageCount(doc);
        let first: i32 = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(0);
        let last: i32 = args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(n - 1);
        let mut pages = Vec::new();
        for pi in first..=last.min(n - 1) {
            let page = b.FPDF_LoadPage(doc, pi);
            let tp = b.FPDFText_LoadPage(page);
            let count = b.FPDFText_CountChars(tp);
            let mut chars = Vec::with_capacity(count.max(0) as usize);
            for i in 0..count {
                let (mut ox, mut oy) = (0f64, 0f64);
                b.FPDFText_GetCharOrigin(tp, i, &mut ox, &mut oy);
                let (mut l, mut r, mut bo, mut t) = (0f64, 0f64, 0f64, 0f64);
                b.FPDFText_GetCharBox(tp, i, &mut l, &mut r, &mut bo, &mut t);
                let mut loose = FS_RECTF { left: 0.0, top: 0.0, right: 0.0, bottom: 0.0 };
                b.FPDFText_GetLooseCharBox(tp, i, &mut loose);
                let mut buf = [0u8; 256];
                let mut flags: i32 = 0;
                let len = b.FPDFText_GetFontInfo(tp, i, buf.as_mut_ptr() as *mut c_void, 256, &mut flags);
                let name = if len > 0 {
                    String::from_utf8_lossy(&buf[..(len as usize).saturating_sub(1).min(255)]).into_owned()
                } else {
                    String::new()
                };
                chars.push(json!([
                    b.FPDFText_GetUnicode(tp, i),
                    b.FPDFText_IsGenerated(tp, i),
                    [ox, oy],
                    [l, r, bo, t],
                    [loose.left, loose.right, loose.bottom, loose.top],
                    b.FPDFText_GetFontSize(tp, i),
                    name,
                    flags,
                ]));
            }
            let mut objs = 0;
            let count_objs = b.FPDFPage_CountObjects(page);
            for k in 0..count_objs {
                if b.FPDFPageObj_GetType(b.FPDFPage_GetObject(page, k)) == FPDF_PAGEOBJ_TEXT as i32 {
                    objs += 1;
                }
            }
            pages.push(json!({
                "page": pi + 1,
                "rotation": b.FPDFPage_GetRotation(page),
                "text_objects_top": objs,
                "chars": chars,
            }));
            b.FPDFText_ClosePage(tp);
            b.FPDF_ClosePage(page);
        }
        b.FPDF_CloseDocument(doc);
        b.FPDF_DestroyLibrary();
        println!("{}", Value::Array(pages));
    }
    Ok(())
}
