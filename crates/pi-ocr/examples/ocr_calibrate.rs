//! Calibrate `SpanCalibration::font_size_factor` against a real OCR endpoint.
//!
//! Renders pages of born-digital PDFs, OCRs them, matches OCR lines to text-layer spans on the
//! same baseline, and prints the median of (text-layer font size / OCR line-box height).
//!
//! Usage (env: PI_OCR_BASE_URL, PI_OCR_MODEL, PI_OCR_KEY; PDFIUM_LIB):
//!   cargo run --release -p pi-ocr --example ocr_calibrate -- \
//!     [--profile spans-json|paddleocr-vl] [--pages 3] [--dump-raw] a.pdf b.pdf ...
//!
//! `--dump-raw` prints the raw reply for the first page and exits: use it to confirm that a
//! PaddleOCR-VL server returns `<|LOC_n|>` tokens (it must honour `skip_special_tokens: false`).

use anyhow::{Context, Result, bail};
use pi_extract::pdfium::Document;
use pi_ocr::{
    OcrEngine, OcrPage, OpenAiVisionConfig, OpenAiVisionEngine, PaddleVlConfig, PaddleVlEngine,
    SpanCalibration, render, spans,
};

enum Engine {
    Spans(OpenAiVisionEngine),
    Paddle(PaddleVlEngine),
}

impl Engine {
    fn dpi(&self, w_pt: f64, h_pt: f64) -> f64 {
        match self {
            Engine::Spans(e) => e.preferred_dpi(w_pt, h_pt),
            Engine::Paddle(e) => e.preferred_dpi(w_pt, h_pt),
        }
        .unwrap_or(200.0)
    }

    async fn ocr(&self, img: &render::PageImage) -> Result<OcrPage> {
        match self {
            Engine::Spans(e) => e.ocr(img).await,
            Engine::Paddle(e) => e.ocr(img).await,
        }
    }

    async fn raw(&self, img: &render::PageImage) -> Result<String> {
        match self {
            Engine::Spans(e) => e.call(img).await,
            Engine::Paddle(e) => e.ask(&img.png, pi_ocr::paddle_vl::SPOTTING_PROMPT).await,
        }
    }
}

fn flag_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let pages_per_doc: usize = flag_value(&args, "--pages")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let dump_raw = args.iter().any(|a| a == "--dump-raw");
    let base_url = std::env::var("PI_OCR_BASE_URL").context("PI_OCR_BASE_URL")?;
    let model = std::env::var("PI_OCR_MODEL").context("PI_OCR_MODEL")?;
    let key = std::env::var("PI_OCR_KEY").ok();
    let engine = match flag_value(&args, "--profile").unwrap_or("spans-json") {
        "spans-json" => Engine::Spans(OpenAiVisionEngine::new(OpenAiVisionConfig::new(
            base_url, model, key,
        ))?),
        "paddleocr-vl" => {
            let mut cfg = PaddleVlConfig::new(base_url, model, key);
            // calibration only needs text lines
            cfg.tables = false;
            Engine::Paddle(PaddleVlEngine::new(cfg)?)
        }
        other => bail!("unknown --profile {other:?} (spans-json | paddleocr-vl)"),
    };
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
            let [x0, y0, x1, y1] = tpage.viewbox.unwrap_or([0.0, 0.0, 612.0, 792.0]);
            let dpi = engine.dpi((x1 - x0).abs(), (y1 - y0).abs());
            let img = render::render_page(&mut doc, idx, dpi, None)?;
            if dump_raw {
                eprintln!(
                    "{path} p{} rendered {}x{} px at {dpi:.0} dpi",
                    idx + 1,
                    img.width_px,
                    img.height_px
                );
                println!("{}", engine.raw(&img).await?);
                return Ok(());
            }
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
