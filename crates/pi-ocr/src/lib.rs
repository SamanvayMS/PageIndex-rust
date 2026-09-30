//! OCR producer for the PageIndex pipeline (plan §9, Track B).
//!
//! Pages that triage routes to OCR are rendered with PDFium, sent to an [`OcrEngine`]
//! (default: an OpenAI-compatible vision endpoint), and converted into `pi_core::Span`s that the
//! layout pipeline consumes unchanged. Tables come back as markdown for `pages.json`, and
//! layout-model titles as hints.

pub mod engine;
pub mod parse;
pub mod render;
pub mod route;
pub mod spans;
pub mod table;

use anyhow::Result;
use futures::stream::{self, StreamExt};
use pi_core::PageSpans;
use pi_extract::pdfium::Document;
use pi_triage::{PageLabel, PageTriage};
use serde::{Deserialize, Serialize};

pub use engine::{OcrEngine, OcrPage, OpenAiVisionConfig, OpenAiVisionEngine};
pub use route::PageSource;
pub use spans::{LayoutHint, OcrSpans, OcrTable, SpanCalibration};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrOptions {
    pub dpi: f64,
    pub concurrency: usize,
    pub calibration: SpanCalibration,
}

impl Default for OcrOptions {
    fn default() -> Self {
        Self {
            dpi: 200.0,
            concurrency: 8,
            calibration: SpanCalibration::default(),
        }
    }
}

/// One page after routing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutedPage {
    pub spans: PageSpans,
    pub source: PageSource,
    pub label: PageLabel,
    pub hints: Vec<LayoutHint>,
    pub tables: Vec<OcrTable>,
    /// OCR failure for this page, if any (the text layer is kept).
    pub error: Option<String>,
}

/// Run OCR where triage asks for it and route every page. PDFium rendering runs first
/// (sequentially: PDFium is not thread-safe); engine calls then run `concurrency` at a time.
pub async fn ocr_and_route<E: OcrEngine>(
    pdf_bytes: Vec<u8>,
    text_pages: Vec<PageSpans>,
    triage: &[PageTriage],
    engine: &E,
    opts: &OcrOptions,
) -> Result<Vec<RoutedPage>> {
    // (page index, images) for every page needing OCR
    let mut jobs: Vec<(usize, Vec<render::PageImage>)> = Vec::new();
    {
        let mut doc = Document::open(pdf_bytes)?;
        for (idx, t) in triage.iter().enumerate() {
            let imgs = match t.label {
                PageLabel::Scanned | PageLabel::Garbled => {
                    vec![render::render_page(&mut doc, idx, opts.dpi, None)?]
                }
                PageLabel::Mixed => t
                    .signals
                    .image_regions
                    .iter()
                    .map(|r| render::render_page(&mut doc, idx, opts.dpi, Some(*r)))
                    .collect::<Result<_>>()?,
                _ => continue,
            };
            if !imgs.is_empty() {
                jobs.push((idx, imgs));
            }
        }
    }
    let results: Vec<(usize, Result<OcrSpans>)> = stream::iter(jobs)
        .map(|(idx, imgs)| async move {
            let mut acc = OcrSpans::default();
            for img in &imgs {
                match engine.ocr(img).await {
                    Ok(p) => {
                        let s = spans::to_spans(&p, img, &opts.calibration);
                        acc.spans.extend(s.spans);
                        acc.hints.extend(s.hints);
                        acc.tables.extend(s.tables);
                    }
                    Err(e) => return (idx, Err(e)),
                }
            }
            (idx, Ok(acc))
        })
        .buffer_unordered(opts.concurrency.max(1))
        .collect()
        .await;
    let mut by_page: std::collections::HashMap<usize, Result<OcrSpans>> =
        results.into_iter().collect();
    let mut out = Vec::with_capacity(text_pages.len());
    for (idx, text) in text_pages.into_iter().enumerate() {
        let label = triage.get(idx).map(|t| t.label).unwrap_or(PageLabel::Text);
        let (ocr, error) = match by_page.remove(&idx) {
            Some(Ok(o)) => (Some(o), None),
            Some(Err(e)) => (None, Some(format!("{e:#}"))),
            None => (None, None),
        };
        let (spans, source) = route::combine(text, label, ocr.as_ref());
        let (hints, tables) = ocr.map(|o| (o.hints, o.tables)).unwrap_or_default();
        out.push(RoutedPage {
            spans,
            source,
            label,
            hints,
            tables,
            error,
        });
    }
    Ok(out)
}
