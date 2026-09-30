//! OCR engines. The default engine calls an OpenAI-compatible chat-completions endpoint with
//! the page image (hosted APIs and self-hosted servers, e.g. a DGX, expose the same API).

use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::parse::{OcrBlock, parse_reply};
use crate::render::PageImage;

/// Engine-independent OCR result for one image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrPage {
    pub page: u32,
    pub blocks: Vec<OcrBlock>,
}

/// An OCR engine: image in, positioned blocks out.
pub trait OcrEngine: Send + Sync {
    fn ocr(&self, img: &PageImage) -> impl std::future::Future<Output = Result<OcrPage>> + Send;
}

/// The instruction sent with every page (profile `spans-json`).
pub const SPANS_JSON_PROMPT: &str = "You are an OCR engine for financial and legal documents. \
Transcribe ALL text on the page image exactly, in reading order, preserving numbers, punctuation and \
capitalization. Return ONLY a JSON object of the form \
{\"blocks\":[{\"bbox\":[x0,y0,x1,y1],\"kind\":\"text|title|table|header|footer|caption\",\"level\":1,\
\"lines\":[{\"bbox\":[x0,y0,x1,y1],\"text\":\"...\"}],\"text\":\"...\",\"html\":\"<table>...</table>\"}]}. \
Coordinates are image pixels with the origin at the top-left. Give each text line its own entry in \
\"lines\". Use kind \"title\" for headings with \"level\" 1 for top-level headings, deeper numbers for \
subheadings. For tables give \"html\" with <table><tr><td> markup (use <th> for header cells) and put the \
plain text in \"text\". Do not summarize, translate or correct the text.";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiVisionConfig {
    /// e.g. `https://api.openai.com/v1` or `http://dgx:8001/v1`
    pub base_url: String,
    pub model: String,
    /// Resolved key (callers read it from the configured env var); `None` sends no auth header.
    pub api_key: Option<String>,
    pub timeout_s: u64,
    pub max_retries: u32,
    /// Send `response_format: {"type": "json_object"}` (disable for servers that reject it).
    pub json_mode: bool,
    pub max_tokens: u32,
    pub prompt: String,
}

impl OpenAiVisionConfig {
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
            json_mode: true,
            max_tokens: 8192,
            prompt: SPANS_JSON_PROMPT.to_string(),
        }
    }
}

pub struct OpenAiVisionEngine {
    cfg: OpenAiVisionConfig,
    http: reqwest::Client,
}

impl OpenAiVisionEngine {
    pub fn new(cfg: OpenAiVisionConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.timeout_s))
            .build()?;
        Ok(Self { cfg, http })
    }

    fn body(&self, img: &PageImage) -> serde_json::Value {
        let b64 = base64::engine::general_purpose::STANDARD.encode(&img.png);
        let mut body = json!({
            "model": self.cfg.model,
            "temperature": 0,
            "max_tokens": self.cfg.max_tokens,
            "messages": [
                {"role": "system", "content": self.cfg.prompt},
                {"role": "user", "content": [
                    {"type": "text", "text": format!("Page {}. Image is {}x{} px.", img.page, img.width_px, img.height_px)},
                    {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{b64}")}}
                ]}
            ]
        });
        if self.cfg.json_mode {
            body["response_format"] = json!({"type": "json_object"});
        }
        body
    }

    async fn call(&self, img: &PageImage) -> Result<String> {
        let url = format!(
            "{}/chat/completions",
            self.cfg.base_url.trim_end_matches('/')
        );
        let mut req = self.http.post(&url).json(&self.body(img));
        if let Some(k) = &self.cfg.api_key {
            req = req.bearer_auth(k);
        }
        let resp = req.send().await.context("OCR request")?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            bail!(
                "OCR endpoint returned {status}: {}",
                text.chars().take(300).collect::<String>()
            );
        }
        let v: serde_json::Value =
            serde_json::from_str(&text).context("OCR response is not JSON")?;
        Ok(v["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    }
}

fn retryable(e: &anyhow::Error) -> bool {
    let s = e.to_string();
    !(s.contains(" 400 ") || s.contains(" 401 ") || s.contains(" 403 ") || s.contains(" 404 "))
}

impl OcrEngine for OpenAiVisionEngine {
    async fn ocr(&self, img: &PageImage) -> Result<OcrPage> {
        let mut attempt = 0;
        loop {
            match self.call(img).await {
                Ok(reply) => {
                    return Ok(OcrPage {
                        page: img.page,
                        blocks: parse_reply(&reply, img.width_px as f64, img.height_px as f64),
                    });
                }
                Err(e) if attempt < self.cfg.max_retries && retryable(&e) => {
                    attempt += 1;
                    tokio::time::sleep(Duration::from_millis(500 * (1 << attempt.min(6)))).await;
                }
                Err(e) => return Err(e),
            }
        }
    }
}
