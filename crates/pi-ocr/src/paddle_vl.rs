//! PaddleOCR-VL behind an OpenAI-compatible server (vLLM / `paddleocr genai_server`).
//!
//! PaddleOCR-VL is element-level: one image plus a task prompt per request (model card,
//! `PaddlePaddle/PaddleOCR-VL-1.6`). This engine sends each page with `Spotting:` (text lines
//! with `<|LOC_0..1000|>` location tokens, normalised to 0-1000), finds table regions from the
//! spotted lines, and sends each region crop with `Table Recognition:` (OTSL output).
//! vLLM strips special tokens unless asked not to, so every request carries
//! `skip_special_tokens: false`; without it the LOC tokens vanish and the engine falls back to
//! plain-text lines with approximate boxes.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::engine::{OcrEngine, OcrPage};
use crate::http;
use crate::otsl::table_reply_to_markdown;
use crate::parse::{BlockKind, OcrBlock, OcrLine, parse_reply};
use crate::render::PageImage;

pub const SPOTTING_PROMPT: &str = "Spotting:";
pub const TABLE_PROMPT: &str = "Table Recognition:";
/// Spotting pixel budget from the model card: 2048 * 28 * 28.
pub const SPOTTING_MAX_PIXELS: u64 = 2048 * 28 * 28;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaddleVlConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub timeout_s: u64,
    pub max_retries: u32,
    pub max_tokens: u32,
    /// Page images are rendered to about this many pixels.
    pub max_pixels: u64,
    /// Recognise detected table regions (one extra request per table).
    pub tables: bool,
}

impl PaddleVlConfig {
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            model: model.into(),
            api_key,
            timeout_s: 180,
            max_retries: 4,
            max_tokens: 8192,
            max_pixels: SPOTTING_MAX_PIXELS,
            tables: true,
        }
    }
}

pub struct PaddleVlEngine {
    cfg: PaddleVlConfig,
    http: reqwest::Client,
}

static WARNED_NO_LOC: AtomicBool = AtomicBool::new(false);

impl PaddleVlEngine {
    pub fn new(cfg: PaddleVlConfig) -> Result<Self> {
        let http = http::client(cfg.timeout_s)?;
        Ok(Self { cfg, http })
    }

    /// The request body: user turn = image + task prompt, no system message, special tokens kept.
    pub fn body(&self, png: &[u8], prompt: &str) -> serde_json::Value {
        json!({
            "model": self.cfg.model,
            "temperature": 0,
            "max_tokens": self.cfg.max_tokens,
            "skip_special_tokens": false,
            "messages": [{"role": "user", "content": [
                {"type": "image_url", "image_url": {"url": http::png_data_url(png)}},
                {"type": "text", "text": prompt}
            ]}]
        })
    }

    /// One raw chat reply for `prompt` on `png` (used by `ocr` and by the calibration tool).
    pub async fn ask(&self, png: &[u8], prompt: &str) -> Result<String> {
        http::post_chat_retrying(
            &self.http,
            &self.cfg.base_url,
            self.cfg.api_key.as_deref(),
            &self.body(png, prompt),
            self.cfg.max_retries,
        )
        .await
    }
}

impl OcrEngine for PaddleVlEngine {
    async fn ocr(&self, img: &PageImage) -> Result<OcrPage> {
        let (w, h) = (img.width_px as f64, img.height_px as f64);
        let reply = self.ask(&img.png, SPOTTING_PROMPT).await?;
        let Some(lines) = parse_spotting(&reply, w, h) else {
            if !WARNED_NO_LOC.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "warning: PaddleOCR-VL reply has no <|LOC_n|> tokens; the server is stripping special \
                     tokens (needs skip_special_tokens=false). Falling back to approximate line boxes."
                );
            }
            return Ok(OcrPage {
                page: img.page,
                blocks: parse_reply(&reply, w, h),
            });
        };
        let mut blocks: Vec<OcrBlock> = lines
            .iter()
            .map(|l| OcrBlock {
                bbox: l.bbox,
                kind: BlockKind::Text,
                level: None,
                lines: vec![l.clone()],
                text: l.text.clone(),
                html: None,
                markdown: None,
                confidence: None,
                approx_bbox: false,
            })
            .collect();
        if self.cfg.tables {
            for region in table_regions(&lines, w, h) {
                let crop = crop_png(&img.png, region).context("cropping table region")?;
                let reply = self.ask(&crop, TABLE_PROMPT).await?;
                if let Some(md) = table_reply_to_markdown(&reply) {
                    blocks.push(OcrBlock {
                        bbox: region,
                        kind: BlockKind::Table,
                        level: None,
                        lines: Vec::new(),
                        text: String::new(),
                        html: None,
                        markdown: Some(md),
                        confidence: None,
                        approx_bbox: false,
                    });
                }
            }
        }
        Ok(OcrPage {
            page: img.page,
            blocks,
        })
    }

    fn preferred_dpi(&self, w_pt: f64, h_pt: f64) -> Option<f64> {
        let area = (w_pt * h_pt).max(1.0);
        Some((72.0 * (self.cfg.max_pixels as f64 / area).sqrt()).clamp(72.0, 400.0))
    }
}

static LOC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<\|LOC_(\d{1,4})\|>").expect("loc re"));

enum Piece {
    Text(String),
    Loc(f64),
}

/// Text lines with pixel boxes from a `Spotting:` reply; `None` when it carries no LOC tokens.
///
/// Tolerant of the token order: a run of 4 locations is `x0 y0 x1 y1`, a run of 8 a quad
/// (converted to its bounding box); the run attaches to the text before it, or to the text
/// after it when the reply starts with locations.
pub fn parse_spotting(reply: &str, w: f64, h: f64) -> Option<Vec<OcrLine>> {
    let mut pieces = Vec::new();
    let mut last = 0;
    for m in LOC_RE.captures_iter(reply) {
        let whole = m.get(0).expect("match");
        let t = reply[last..whole.start()].trim();
        if !t.is_empty() {
            pieces.push(Piece::Text(t.to_string()));
        }
        pieces.push(Piece::Loc(m[1].parse::<f64>().unwrap_or(0.0).min(1000.0)));
        last = whole.end();
    }
    if !pieces.iter().any(|p| matches!(p, Piece::Loc(_))) {
        return None;
    }
    let t = reply[last..].trim();
    if !t.is_empty() {
        pieces.push(Piece::Text(t.to_string()));
    }
    let loc_first = matches!(pieces.first(), Some(Piece::Loc(_)));
    // Split into the sequence of texts and the sequence of location runs.
    let mut runs: Vec<Vec<f64>> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    let mut cur: Vec<f64> = Vec::new();
    for p in pieces {
        match p {
            Piece::Loc(v) => cur.push(v),
            Piece::Text(t) => {
                if !cur.is_empty() {
                    runs.push(std::mem::take(&mut cur));
                }
                texts.push(t);
            }
        }
    }
    if !cur.is_empty() {
        runs.push(cur);
    }
    let pairs: Vec<(String, Vec<f64>)> = if loc_first {
        runs.into_iter().zip(texts).map(|(r, t)| (t, r)).collect()
    } else {
        texts.into_iter().zip(runs).collect()
    };
    let to_px = |v: f64, size: f64| v / 1000.0 * size;
    let mut out = Vec::new();
    for (text, run) in pairs {
        let bbox = match run.len() {
            n if n >= 8 && n % 8 == 0 => {
                let xs = [run[0], run[2], run[4], run[6]];
                let ys = [run[1], run[3], run[5], run[7]];
                let mn = |v: [f64; 4]| v.into_iter().fold(f64::INFINITY, f64::min);
                let mx = |v: [f64; 4]| v.into_iter().fold(f64::NEG_INFINITY, f64::max);
                [mn(xs), mn(ys), mx(xs), mx(ys)]
            }
            n if n >= 4 => [
                run[0].min(run[2]),
                run[1].min(run[3]),
                run[0].max(run[2]),
                run[1].max(run[3]),
            ],
            _ => continue,
        };
        out.push(OcrLine {
            bbox: [
                to_px(bbox[0], w),
                to_px(bbox[1], h),
                to_px(bbox[2], w),
                to_px(bbox[3], h),
            ],
            text,
        });
    }
    Some(out)
}

fn is_numericish(s: &str) -> bool {
    let n = s.chars().filter(|c| !c.is_whitespace()).count();
    let d = s.chars().filter(|c| c.is_ascii_digit()).count();
    let p = s
        .chars()
        .filter(|c| matches!(c, ',' | '.' | '(' | ')' | '$' | '%' | '-' | '—' | '€' | '£'))
        .count();
    d > 0 && (d + p) * 2 >= n
}

/// Table regions (pixel boxes) from spotted lines: runs of >= 3 consecutive rows that each hold
/// >= 2 cells, at least one numeric, with row gaps under 3 line heights.
pub fn table_regions(lines: &[OcrLine], w: f64, h: f64) -> Vec<[f64; 4]> {
    if lines.len() < 6 {
        return Vec::new();
    }
    let mut hs: Vec<f64> = lines
        .iter()
        .map(|l| l.bbox[3] - l.bbox[1])
        .filter(|v| *v > 0.0)
        .collect();
    if hs.is_empty() {
        return Vec::new();
    }
    hs.sort_by(f64::total_cmp);
    let hmed = hs[hs.len() / 2];
    let mut idx: Vec<usize> = (0..lines.len()).collect();
    let yc = |i: usize| (lines[i].bbox[1] + lines[i].bbox[3]) / 2.0;
    idx.sort_by(|&a, &b| yc(a).total_cmp(&yc(b)));
    let mut rows: Vec<Vec<usize>> = Vec::new();
    for i in idx {
        match rows.last_mut() {
            Some(r) if (yc(i) - yc(r[0])).abs() <= 0.5 * hmed => r.push(i),
            _ => rows.push(vec![i]),
        }
    }
    let tabular = |r: &Vec<usize>| r.len() >= 2 && r.iter().any(|&i| is_numericish(&lines[i].text));
    let union = |rs: &[Vec<usize>]| {
        let mut b = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for &i in rs.iter().flatten() {
            let l = lines[i].bbox;
            b = [
                b[0].min(l[0]),
                b[1].min(l[1]),
                b[2].max(l[2]),
                b[3].max(l[3]),
            ];
        }
        let pad = 0.5 * hmed;
        [
            (b[0] - pad).max(0.0),
            (b[1] - pad).max(0.0),
            (b[2] + pad).min(w),
            (b[3] + pad).min(h),
        ]
    };
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for k in 0..=rows.len() {
        let cont = k < rows.len()
            && tabular(&rows[k])
            && start.is_none_or(|_| {
                let prev_bottom = rows[k - 1]
                    .iter()
                    .map(|&i| lines[i].bbox[3])
                    .fold(f64::NEG_INFINITY, f64::max);
                let top = rows[k]
                    .iter()
                    .map(|&i| lines[i].bbox[1])
                    .fold(f64::INFINITY, f64::min);
                top - prev_bottom <= 3.0 * hmed
            });
        match (cont, start) {
            (true, None) => start = Some(k),
            (true, Some(_)) => {}
            (false, Some(s)) => {
                if k - s >= 3 {
                    out.push(union(&rows[s..k]));
                }
                start = if k < rows.len() && tabular(&rows[k]) {
                    Some(k)
                } else {
                    None
                };
            }
            (false, None) => {}
        }
    }
    out
}

/// Crop a pixel box out of an 8-bit PNG (gray or RGB/RGBA), returning a grayscale PNG.
pub fn crop_png(png_bytes: &[u8], b: [f64; 4]) -> Result<Vec<u8>> {
    let dec = png::Decoder::new(std::io::Cursor::new(png_bytes));
    let mut reader = dec.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf)?;
    let (w, h) = (info.width as usize, info.height as usize);
    let ch = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => anyhow::bail!("indexed PNG not supported"),
    };
    let x0 = (b[0].floor().max(0.0) as usize).min(w.saturating_sub(1));
    let y0 = (b[1].floor().max(0.0) as usize).min(h.saturating_sub(1));
    let x1 = (b[2].ceil() as usize).clamp(x0 + 1, w);
    let y1 = (b[3].ceil() as usize).clamp(y0 + 1, h);
    let mut gray = Vec::with_capacity((x1 - x0) * (y1 - y0));
    for y in y0..y1 {
        for x in x0..x1 {
            let p = &buf[(y * w + x) * ch..];
            gray.push(if ch >= 3 {
                ((p[0] as u32 + p[1] as u32 + p[2] as u32) / 3) as u8
            } else {
                p[0]
            });
        }
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, (x1 - x0) as u32, (y1 - y0) as u32);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&gray)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spotting_text_first_four_coords() {
        let r = "Item 7. MD&A<|LOC_100|><|LOC_50|><|LOC_500|><|LOC_80|>\nRevenue grew<|LOC_100|><|LOC_100|><|LOC_600|><|LOC_120|>";
        let l = parse_spotting(r, 1000.0, 2000.0).unwrap();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].text, "Item 7. MD&A");
        assert_eq!(l[0].bbox, [100.0, 100.0, 500.0, 160.0]);
        assert_eq!(l[1].text, "Revenue grew");
    }

    #[test]
    fn spotting_loc_first_quad() {
        let r = "<|LOC_10|><|LOC_20|><|LOC_90|><|LOC_20|><|LOC_90|><|LOC_40|><|LOC_10|><|LOC_40|>Title\
                 <|LOC_10|><|LOC_50|><|LOC_90|><|LOC_50|><|LOC_90|><|LOC_70|><|LOC_10|><|LOC_70|>Body";
        let l = parse_spotting(r, 1000.0, 1000.0).unwrap();
        assert_eq!(l.len(), 2);
        assert_eq!(
            (l[0].text.as_str(), l[0].bbox),
            ("Title", [10.0, 20.0, 90.0, 40.0])
        );
        assert_eq!(l[1].text, "Body");
    }

    #[test]
    fn spotting_without_locs_is_none() {
        assert!(parse_spotting("just text\nmore", 100.0, 100.0).is_none());
    }

    #[test]
    fn finds_a_numeric_table() {
        let mut lines = vec![OcrLine {
            bbox: [50.0, 50.0, 400.0, 70.0],
            text: "Consolidated Statement".into(),
        }];
        for (k, (a, b)) in [
            ("Revenue", "1,234"),
            ("Cost of sales", "(812)"),
            ("Gross profit", "422"),
            ("Net income", "97"),
        ]
        .iter()
        .enumerate()
        {
            let y = 100.0 + 30.0 * k as f64;
            lines.push(OcrLine {
                bbox: [50.0, y, 250.0, y + 20.0],
                text: a.to_string(),
            });
            lines.push(OcrLine {
                bbox: [400.0, y, 480.0, y + 20.0],
                text: b.to_string(),
            });
        }
        lines.push(OcrLine {
            bbox: [50.0, 400.0, 500.0, 420.0],
            text: "Some prose follows here.".into(),
        });
        let r = table_regions(&lines, 600.0, 800.0);
        assert_eq!(r.len(), 1);
        assert!(
            r[0][1] < 100.0 && r[0][3] > 210.0 && r[0][3] < 400.0,
            "{r:?}"
        );
    }

    #[test]
    fn body_shape() {
        let e = PaddleVlEngine::new(PaddleVlConfig::new("http://x/v1", "PaddleOCR-VL-1.6", None))
            .unwrap();
        let b = e.body(b"png", SPOTTING_PROMPT);
        assert_eq!(b["skip_special_tokens"], false);
        assert_eq!(b["messages"].as_array().unwrap().len(), 1);
        assert_eq!(b["messages"][0]["content"][1]["text"], "Spotting:");
        assert!(b.get("response_format").is_none());
    }
}
