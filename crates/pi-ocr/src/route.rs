//! Combining text-layer spans with OCR output according to the triage label (plan §9.2).

use pi_core::{PageSpans, Rect, Span};
use pi_triage::{PageLabel, word_stats};
use serde::{Deserialize, Serialize};

use crate::spans::OcrSpans;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageSource {
    TextLayer,
    Ocr,
    TextLayerPlusOcrRegions,
}

fn area(r: &Rect) -> f64 {
    (r.right - r.left).max(0.0) * (r.top - r.bottom).max(0.0)
}

fn intersection(a: &Rect, b: &Rect) -> f64 {
    let w = a.right.min(b.right) - a.left.max(b.left);
    let h = a.top.min(b.top) - a.bottom.max(b.bottom);
    if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
}

pub fn iou(a: &Rect, b: &Rect) -> f64 {
    let i = intersection(a, b);
    let u = area(a) + area(b) - i;
    if u > 0.0 { i / u } else { 0.0 }
}

fn joined(spans: &[Span]) -> String {
    spans
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Text quality for picking between a garbled text layer and OCR: word ratio weighted by
/// how much text there is (an empty result never wins).
pub fn text_score(spans: &[Span]) -> f64 {
    let (ratio, letters) = word_stats(&joined(spans));
    if letters == 0 {
        0.0
    } else {
        ratio * (letters as f64).ln_1p()
    }
}

/// Merge OCR region spans into a text page: drop OCR spans that duplicate text-layer spans
/// (IoU > 0.5, or more than half of the OCR span covered by one text span).
pub fn merge_region_spans(text: &[Span], ocr: &[Span]) -> Vec<Span> {
    let mut out = text.to_vec();
    for o in ocr {
        let dup = text.iter().any(|t| {
            iou(&t.bbox, &o.bbox) > 0.5 || intersection(&t.bbox, &o.bbox) > 0.5 * area(&o.bbox)
        });
        if !dup {
            out.push(o.clone());
        }
    }
    out
}

/// Route one page. `ocr` is the full-page OCR for scanned/garbled pages, or the region OCR
/// (all regions concatenated) for mixed pages.
pub fn combine(
    text: PageSpans,
    label: PageLabel,
    ocr: Option<&OcrSpans>,
) -> (PageSpans, PageSource) {
    let Some(ocr) = ocr else {
        return (text, PageSource::TextLayer);
    };
    match label {
        PageLabel::Scanned => (
            PageSpans {
                spans: ocr.spans.clone(),
                ..text
            },
            PageSource::Ocr,
        ),
        PageLabel::Garbled => {
            if text_score(&ocr.spans) > text_score(&text.spans) {
                (
                    PageSpans {
                        spans: ocr.spans.clone(),
                        ..text
                    },
                    PageSource::Ocr,
                )
            } else {
                (text, PageSource::TextLayer)
            }
        }
        PageLabel::Mixed => {
            let spans = merge_region_spans(&text.spans, &ocr.spans);
            (
                PageSpans { spans, ..text },
                PageSource::TextLayerPlusOcrRegions,
            )
        }
        PageLabel::Text | PageLabel::Graphic => (text, PageSource::TextLayer),
    }
}
