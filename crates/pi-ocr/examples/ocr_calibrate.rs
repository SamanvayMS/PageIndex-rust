//! Calibrate `SpanCalibration::font_size_factor` against a real OCR endpoint.
//!
//! Renders pages of born-digital PDFs, OCRs them, matches OCR lines to text-layer spans on the
//! same baseline, and prints the median of (text-layer font size / OCR line-box height).
//!
//! Usage (env: PI_OCR_BASE_URL, PI_OCR_MODEL, PI_OCR_KEY; PDFIUM_LIB):
//!   cargo run --release -p pi-ocr --example ocr_calibrate -- [--pages 3] a.pdf b.pdf ...

use anyhow::{Context, Result};
use pi_extract::pdfium::Document;
use pi_ocr::{OcrEngine, OpenAiVisionConfig, OpenAiVisionEngine, SpanCalibration, render, spans};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let pages_per_doc: usize = args
        .iter()
        .position(|a| a == "--pages")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let engine = OpenAiVisionEngine::new(OpenAiVisionConfig::new(
        std::env::var("PI_OCR_BASE_URL").context("PI_OCR_BASE_URL")?,
        std::env::var("PI_OCR_MODEL").context("PI_OCR_MODEL")?,
        std::env::var("PI_OCR_KEY").ok(),
    ))?;
    // Unit calibration so the ratio is font size / raw line-box height.
    let unit = SpanCalibration {
        font_size_factor: 1.0,
        baseline_offset: 0.0,
        default_confidence: 1.0,
    };
    let mut ratios = Vec::new();
    for path in args.iter().filter(|a| a.ends_with(".pdf")) {
        let bytes = std::fs::read(path)?;
        let text = pi_extract::extract_pdf_bytes(bytes.clone())?;
        let mut doc = Document::open(bytes)?;
        for (idx, tpage) in text.iter().enumerate().take(pages_per_doc) {
            let img = render::render_page(&mut doc, idx, 200.0, None)?;
            let ocr = engine.ocr(&img).await?;
            let o = spans::to_spans(&ocr, &img, &unit);
            for os in &o.spans {
                let h = os.bbox.top - os.bbox.bottom;
                // text-layer spans whose baseline lies inside the OCR line box and overlap in x
                let best = tpage
                    .spans
                    .iter()
                    .filter(|t| {
                        t.bbox.bottom >= os.bbox.bottom - 0.2 * h
                            && t.bbox.bottom <= os.bbox.top
                            && t.bbox.right > os.bbox.left
                            && t.bbox.left < os.bbox.right
                    })
                    .max_by(|a, b| a.text.len().cmp(&b.text.len()));
                if let Some(t) = best
                    && h > 0.0
                {
                    ratios.push(t.font_size / h);
                }
            }
            eprintln!(
                "{path} p{}: {} OCR lines, {} matched so far",
                idx + 1,
                o.spans.len(),
                ratios.len()
            );
        }
    }
    ratios.sort_by(f64::total_cmp);
    if ratios.is_empty() {
        println!("no matched lines");
    } else {
        let med = ratios[ratios.len() / 2];
        println!("font_size_factor (median of {}): {med:.3}", ratios.len());
    }
    Ok(())
}
