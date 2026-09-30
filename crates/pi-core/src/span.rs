//! Positioned text spans: the contract between producers and the layout pipeline.
//!
//! Field set mirrors the reference `Span` (`pageindex/flash/model/span_line.py:50`) as written to
//! `parity/golden/<doc>/01_spans.json` by `parity/dump_reference.py`.

use serde::{Deserialize, Serialize};

use crate::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SpanSource {
    #[default]
    TextLayer,
    Ocr {
        confidence: f32,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Span {
    /// PDF user space, page viewbox applied.
    pub bbox: Rect,
    pub text: String,
    pub font_name: Option<String>,
    #[serde(with = "crate::float")]
    pub font_size: f64,
    pub bold: bool,
    pub italic: bool,
    /// `f64::INFINITY` for cardinal rotation, as in the reference.
    #[serde(with = "crate::float")]
    pub skew: f64,
    #[serde(default)]
    pub source: SpanSource,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageSpans {
    /// 1-based page number (the reference's `page_index`).
    pub page: u32,
    /// `(x0, y0, x1, y1)` view box, or `None` when unavailable.
    pub viewbox: Option<[f64; 4]>,
    pub rotation: u16,
    pub spans: Vec<Span>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_reference_dump_shape() {
        let js = r#"{"page":1,"viewbox":[0,0,612,792],"rotation":0,"spans":[
            {"bbox":[10,20,700,690],"text":"Hi","font_name":"Times","font_size":9.5,
             "bold":false,"italic":true,"skew":"inf"}]}"#;
        let p: PageSpans = serde_json::from_str(js).unwrap();
        assert_eq!(p.spans[0].source, SpanSource::TextLayer);
        assert!(p.spans[0].skew.is_infinite());
    }

    /// Non-finite floats must also decode from an owned `serde_json::Value` (no borrowing).
    #[test]
    fn non_finite_from_value() {
        let v = serde_json::json!({"bbox":[0,1,1,0],"text":"x","font_name":null,
            "font_size":"nan","bold":false,"italic":false,"skew":"-inf"});
        let s: Span = serde_json::from_value(v).unwrap();
        assert!(s.font_size.is_nan() && s.skew == f64::NEG_INFINITY);
    }
}
