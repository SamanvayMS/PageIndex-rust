//! OCR blocks → `pi_core::Span`s (plus layout hints and markdown tables).

use pi_core::{Rect, Span, SpanSource};
use serde::{Deserialize, Serialize};

use crate::engine::OcrPage;
use crate::parse::BlockKind;
use crate::render::PageImage;
use crate::table::html_table_to_markdown;

/// Converting pixel line boxes into span geometry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanCalibration {
    /// font size = line-box height (pt) × this factor. Calibrate by rendering born-digital pages
    /// and comparing OCR line boxes with text-layer font sizes (docs/spikes/E-ocr-endpoint.md).
    pub font_size_factor: f64,
    /// Baseline offset above the line-box bottom, as a fraction of the box height (descenders).
    pub baseline_offset: f64,
    /// Confidence assigned when the engine gives none.
    pub default_confidence: f32,
}

impl Default for SpanCalibration {
    fn default() -> Self {
        Self {
            font_size_factor: 0.8,
            baseline_offset: 0.2,
            default_confidence: 0.9,
        }
    }
}

/// A layout-model title hint (future `HeadingKind::LayoutTitle` candidate source).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutHint {
    pub page: u32,
    pub bbox: Rect,
    pub level: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrTable {
    pub page: u32,
    pub bbox: Rect,
    pub markdown: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OcrSpans {
    pub spans: Vec<Span>,
    pub hints: Vec<LayoutHint>,
    pub tables: Vec<OcrTable>,
}

fn page_box(img: &PageImage, px: [f64; 4]) -> Rect {
    let pts = [
        (px[0], px[1]),
        (px[2], px[1]),
        (px[0], px[3]),
        (px[2], px[3]),
    ]
    .map(|(x, y)| img.px_to_page(x, y));
    let xs = pts.map(|p| p.0);
    let ys = pts.map(|p| p.1);
    let mn = |v: [f64; 4]| v.into_iter().fold(f64::INFINITY, f64::min);
    let mx = |v: [f64; 4]| v.into_iter().fold(f64::NEG_INFINITY, f64::max);
    Rect::new(mn(xs), mx(xs), mx(ys), mn(ys))
}

/// Convert one OCR result (for `img`) into spans in PDF page space.
pub fn to_spans(ocr: &OcrPage, img: &PageImage, cal: &SpanCalibration) -> OcrSpans {
    let mut out = OcrSpans::default();
    for b in &ocr.blocks {
        let conf = b.confidence.unwrap_or(cal.default_confidence);
        let bbox = page_box(img, b.bbox);
        if b.kind == BlockKind::Title {
            out.hints.push(LayoutHint {
                page: ocr.page,
                bbox,
                level: b.level,
                text: b.text.clone(),
            });
        }
        if b.kind == BlockKind::Table
            && let Some(md) = b
                .markdown
                .clone()
                .or_else(|| b.html.as_deref().and_then(html_table_to_markdown))
        {
            out.tables.push(OcrTable {
                page: ocr.page,
                bbox,
                markdown: md,
            });
        }
        let lines: Vec<([f64; 4], &str)> = if b.lines.is_empty() {
            // One span per text line of the block, stacked evenly inside the block box.
            let ls: Vec<&str> = b.text.lines().filter(|l| !l.trim().is_empty()).collect();
            let n = ls.len().max(1) as f64;
            let h = (b.bbox[3] - b.bbox[1]) / n;
            ls.into_iter()
                .enumerate()
                .map(|(k, t)| {
                    (
                        [
                            b.bbox[0],
                            b.bbox[1] + h * k as f64,
                            b.bbox[2],
                            b.bbox[1] + h * (k as f64 + 1.0),
                        ],
                        t,
                    )
                })
                .collect()
        } else {
            b.lines.iter().map(|l| (l.bbox, l.text.as_str())).collect()
        };
        for (px, text) in lines {
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let r = page_box(img, px);
            let h = (r.top - r.bottom).max(0.1);
            let fs = h * cal.font_size_factor;
            let base = r.bottom + h * cal.baseline_offset;
            out.spans.push(Span {
                bbox: Rect::new(r.left, r.right, base + fs, base),
                text: text.to_string(),
                font_name: None,
                font_size: fs,
                bold: false,
                italic: false,
                skew: 0.0,
                source: SpanSource::Ocr { confidence: conf },
            });
        }
    }
    out
}
