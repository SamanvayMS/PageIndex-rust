//! Page rendering to grayscale PNG with a pixel→PDF-page affine map.
#![allow(unsafe_code)]

use anyhow::{Result, bail};
// PDFium render flags (fpdfview.h).
const FPDF_ANNOT: i32 = 0x01;
const FPDF_GRAYSCALE: i32 = 0x08;
use pi_extract::pdfium::Document;

/// A rendered page (or page region) ready for an OCR engine.
#[derive(Debug, Clone)]
pub struct PageImage {
    /// 1-based page number.
    pub page: u32,
    pub png: Vec<u8>,
    pub width_px: u32,
    pub height_px: u32,
    pub dpi: f64,
    /// Affine map from image pixels (x right, y down) to PDF page space:
    /// `page = (a*x + c*y + e, b*x + d*y + f)`.
    pub to_page: [f64; 6],
}

impl PageImage {
    pub fn px_to_page(&self, x: f64, y: f64) -> (f64, f64) {
        let m = &self.to_page;
        (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
    }
}

fn encode_gray_png(gray: &[u8], w: u32, h: u32) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header()?;
        wr.write_image_data(gray)?;
    }
    Ok(out)
}

/// Render page `idx` (0-based) at `dpi`, optionally cropped to a page-space region
/// `[x0, y0, x1, y1]`.
pub fn render_page(
    doc: &mut Document,
    idx: usize,
    dpi: f64,
    region: Option<[f64; 4]>,
) -> Result<PageImage> {
    let b = doc.b;
    let page = doc.load_page(idx)?;
    // SAFETY: live page; bitmap created and destroyed here.
    unsafe {
        let wpt = b.FPDF_GetPageWidthF(page) as f64;
        let hpt = b.FPDF_GetPageHeightF(page) as f64;
        let scale = dpi / 72.0;
        let w = (wpt * scale).round().max(1.0) as i32;
        let h = (hpt * scale).round().max(1.0) as i32;
        if w > 20_000 || h > 20_000 {
            bail!("page {idx} too large to render at {dpi} dpi ({w}x{h})");
        }
        let bmp = b.FPDFBitmap_Create(w, h, 0);
        if bmp.is_null() {
            bail!("FPDFBitmap_Create failed");
        }
        b.FPDFBitmap_FillRect(bmp, 0, 0, w, h, 0xFFFF_FFFF);
        b.FPDF_RenderPageBitmap(bmp, page, 0, 0, w, h, 0, FPDF_ANNOT | FPDF_GRAYSCALE);
        let stride = b.FPDFBitmap_GetStride(bmp) as usize;
        let buf = std::slice::from_raw_parts(
            b.FPDFBitmap_GetBuffer(bmp) as *const u8,
            stride * h as usize,
        );
        // Device (pixel) -> page affine, from PDFium's own transform (handles /Rotate, CropBox).
        let dev = |x: i32, y: i32| {
            let (mut px, mut py) = (0f64, 0f64);
            b.FPDF_DeviceToPage(page, 0, 0, w, h, 0, x, y, &mut px, &mut py);
            (px, py)
        };
        let (o, xa, ya) = (dev(0, 0), dev(w, 0), dev(0, h));
        let full = [
            (xa.0 - o.0) / w as f64,
            (xa.1 - o.1) / w as f64,
            (ya.0 - o.0) / h as f64,
            (ya.1 - o.1) / h as f64,
            o.0,
            o.1,
        ];
        // Pixel crop window for the region (page -> pixel via the inverse affine).
        let (x0, y0, x1, y1) = match region {
            None => (0, 0, w, h),
            Some(r) => {
                let det = full[0] * full[3] - full[1] * full[2];
                let inv = |px: f64, py: f64| {
                    let (dx, dy) = (px - full[4], py - full[5]);
                    (
                        (full[3] * dx - full[2] * dy) / det,
                        (-full[1] * dx + full[0] * dy) / det,
                    )
                };
                let pts = [
                    inv(r[0], r[1]),
                    inv(r[2], r[3]),
                    inv(r[0], r[3]),
                    inv(r[2], r[1]),
                ];
                let xs = pts.map(|p| p.0);
                let ys = pts.map(|p| p.1);
                let cl = |v: f64, hi: i32| (v.clamp(0.0, hi as f64)) as i32;
                let fx =
                    |f: fn(f64, f64) -> f64, v: [f64; 4]| v.into_iter().reduce(f).unwrap_or(0.0);
                (
                    cl(fx(f64::min, xs).floor(), w),
                    cl(fx(f64::min, ys).floor(), h),
                    cl(fx(f64::max, xs).ceil(), w),
                    cl(fx(f64::max, ys).ceil(), h),
                )
            }
        };
        let (cw, ch) = ((x1 - x0).max(1), (y1 - y0).max(1));
        let mut gray = Vec::with_capacity((cw * ch) as usize);
        for y in y0..y0 + ch {
            let row = &buf[y.min(h - 1) as usize * stride..];
            for x in x0..x0 + cw {
                let p = &row[x.min(w - 1) as usize * 4..];
                // BGRx; grayscale render leaves channels equal
                gray.push(((p[0] as u32 + p[1] as u32 + p[2] as u32) / 3) as u8);
            }
        }
        b.FPDFBitmap_Destroy(bmp);
        let to_page = [
            full[0],
            full[1],
            full[2],
            full[3],
            full[4] + full[0] * x0 as f64 + full[2] * y0 as f64,
            full[5] + full[1] * x0 as f64 + full[3] * y0 as f64,
        ];
        Ok(PageImage {
            page: idx as u32 + 1,
            png: encode_gray_png(&gray, cw as u32, ch as u32)?,
            width_px: cw as u32,
            height_px: ch as u32,
            dpi,
            to_page,
        })
    }
}
