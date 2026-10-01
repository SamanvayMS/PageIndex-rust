//! Lenient parsing of OCR engine replies (docs/spikes/E-ocr-endpoint.md).

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    #[default]
    Text,
    Title,
    Table,
    Header,
    Footer,
    Caption,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrLine {
    /// `[x0, y0, x1, y1]` image pixels, origin top-left.
    pub bbox: [f64; 4],
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrBlock {
    pub bbox: [f64; 4],
    pub kind: BlockKind,
    pub level: Option<u32>,
    pub lines: Vec<OcrLine>,
    pub text: String,
    pub html: Option<String>,
    /// Table already converted to markdown by the engine (e.g. from OTSL).
    #[serde(default)]
    pub markdown: Option<String>,
    pub confidence: Option<f32>,
    /// Boxes synthesized from plain text (no geometry from the engine).
    pub approx_bbox: bool,
}

fn as_bbox(v: &Value) -> Option<[f64; 4]> {
    let a = v.as_array()?;
    if a.len() != 4 {
        return None;
    }
    let mut out = [0.0; 4];
    for (o, x) in out.iter_mut().zip(a) {
        *o = x.as_f64()?;
    }
    // normalise to x0<=x1, y0<=y1
    Some([
        out[0].min(out[2]),
        out[1].min(out[3]),
        out[0].max(out[2]),
        out[1].max(out[3]),
    ])
}

fn kind_of(s: &str) -> BlockKind {
    match s.to_ascii_lowercase().as_str() {
        "title" | "heading" | "section_header" | "section-header" | "doc_title" => BlockKind::Title,
        "table" => BlockKind::Table,
        "header" | "page_header" => BlockKind::Header,
        "footer" | "page_footer" => BlockKind::Footer,
        "caption" | "figure_caption" | "table_caption" => BlockKind::Caption,
        _ => BlockKind::Text,
    }
}

/// Extract the outermost JSON object/array from a reply (code fences and prose tolerated).
fn json_payload(reply: &str) -> Option<Value> {
    let s = reply.trim();
    if let Ok(v) = serde_json::from_str(s) {
        return Some(v);
    }
    for (open, close) in [('{', '}'), ('[', ']')] {
        if let (Some(a), Some(b)) = (s.find(open), s.rfind(close))
            && a < b
            && let Ok(v) = serde_json::from_str(&s[a..=b])
        {
            return Some(v);
        }
    }
    None
}

/// Parse a reply into blocks; plain text/markdown falls back to evenly distributed lines
/// (`approx_bbox = true`) with `#` headings as titles.
pub fn parse_reply(reply: &str, width_px: f64, height_px: f64) -> Vec<OcrBlock> {
    if let Some(v) = json_payload(reply) {
        let arr = match &v {
            Value::Array(a) => Some(a.clone()),
            Value::Object(o) => o.get("blocks").and_then(|b| b.as_array()).cloned(),
            _ => None,
        };
        if let Some(arr) = arr {
            let blocks: Vec<OcrBlock> = arr.iter().filter_map(block_from).collect();
            if !blocks.is_empty() || arr.is_empty() {
                return blocks;
            }
        }
    }
    plain_text_blocks(reply, width_px, height_px)
}

fn block_from(b: &Value) -> Option<OcrBlock> {
    let o = b.as_object()?;
    let lines: Vec<OcrLine> = o
        .get("lines")
        .and_then(|l| l.as_array())
        .map(|ls| {
            ls.iter()
                .filter_map(|l| {
                    Some(OcrLine {
                        bbox: as_bbox(l.get("bbox")?)?,
                        text: l.get("text")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let text = o
        .get("text")
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            lines
                .iter()
                .map(|l| l.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        });
    let bbox = o.get("bbox").and_then(as_bbox).or_else(|| {
        let f = lines.first()?.bbox;
        Some(lines.iter().fold(f, |a, l| {
            [
                a[0].min(l.bbox[0]),
                a[1].min(l.bbox[1]),
                a[2].max(l.bbox[2]),
                a[3].max(l.bbox[3]),
            ]
        }))
    })?;
    if text.trim().is_empty() && o.get("html").is_none() {
        return None;
    }
    Some(OcrBlock {
        bbox,
        kind: o
            .get("kind")
            .or_else(|| o.get("type"))
            .and_then(|k| k.as_str())
            .map(kind_of)
            .unwrap_or_default(),
        level: o.get("level").and_then(|l| l.as_u64()).map(|l| l as u32),
        lines,
        text,
        html: o.get("html").and_then(|h| h.as_str()).map(str::to_string),
        markdown: None,
        confidence: o
            .get("confidence")
            .and_then(|c| c.as_f64())
            .map(|c| c as f32),
        approx_bbox: false,
    })
}

fn plain_text_blocks(reply: &str, w: f64, h: f64) -> Vec<OcrBlock> {
    let body = reply
        .trim()
        .trim_start_matches("```markdown")
        .trim_start_matches("```")
        .trim_end_matches("```");
    let lines: Vec<&str> = body
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let n = lines.len() as f64;
    let (top, bottom) = (0.05 * h, 0.95 * h);
    let step = (bottom - top) / n;
    lines
        .iter()
        .enumerate()
        .map(|(k, l)| {
            let hashes = l.chars().take_while(|&c| c == '#').count();
            let (kind, level, text) = if hashes > 0 && l[hashes..].starts_with(' ') {
                (
                    BlockKind::Title,
                    Some(hashes as u32),
                    l[hashes..].trim().to_string(),
                )
            } else {
                (BlockKind::Text, None, l.trim().to_string())
            };
            let y0 = top + step * k as f64;
            let bbox = [0.08 * w, y0, 0.92 * w, y0 + step * 0.8];
            OcrBlock {
                bbox,
                kind,
                level,
                lines: vec![OcrLine {
                    bbox,
                    text: text.clone(),
                }],
                text,
                html: None,
                markdown: None,
                confidence: None,
                approx_bbox: true,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fenced_json() {
        let r = "```json\n{\"blocks\":[{\"bbox\":[10,20,300,40],\"kind\":\"title\",\"level\":1,\"text\":\"Item 7. MD&A\"}]}\n```";
        let b = parse_reply(r, 1000.0, 1000.0);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].kind, BlockKind::Title);
        assert!(!b[0].approx_bbox);
    }

    #[test]
    fn markdown_fallback() {
        let b = parse_reply(
            "# Annual Report\nRevenue grew 5%.\n## Risks",
            1000.0,
            1000.0,
        );
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].kind, BlockKind::Title);
        assert_eq!(b[2].level, Some(2));
        assert!(b.iter().all(|x| x.approx_bbox));
        assert!(b[0].bbox[1] < b[1].bbox[1]);
    }
}
