//! OCR engines. The default engine calls an OpenAI-compatible chat-completions endpoint with
//! the page image (hosted APIs and self-hosted servers, e.g. a DGX, expose the same API).

use anyhow::Result;
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

    /// Render resolution the engine wants for a page of `w_pt` x `h_pt` points; `None` uses
    /// the configured dpi.
    fn preferred_dpi(&self, _w_pt: f64, _h_pt: f64) -> Option<f64> {
        None
    }
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
        let http = crate::http::client(cfg.timeout_s)?;
        Ok(Self { cfg, http })
    }

    fn body(&self, img: &PageImage) -> serde_json::Value {
        let mut body = json!({
            "model": self.cfg.model,
            "temperature": 0,
            "max_tokens": self.cfg.max_tokens,
            "messages": [
                {"role": "system", "content": self.cfg.prompt},
                {"role": "user", "content": [
                    {"type": "text", "text": format!("Page {}. Image is {}x{} px.", img.page, img.width_px, img.height_px)},
                    {"type": "image_url", "image_url": {"url": crate::http::png_data_url(&img.png)}}
                ]}
            ]
        });
        if self.cfg.json_mode {
            body["response_format"] = json!({"type": "json_object"});
        }
        body
    }

    /// One raw chat reply for `img` (used by `ocr` and by the calibration tool).
    pub async fn call(&self, img: &PageImage) -> Result<String> {
        crate::http::post_chat_retrying(
            &self.http,
            &self.cfg.base_url,
            self.cfg.api_key.as_deref(),
            &self.body(img),
            self.cfg.max_retries,
        )
        .await
    }
}

impl OcrEngine for OpenAiVisionEngine {
    async fn ocr(&self, img: &PageImage) -> Result<OcrPage> {
        let reply = self.call(img).await?;
        Ok(OcrPage {
            page: img.page,
            blocks: parse_reply(&reply, img.width_px as f64, img.height_px as f64),
        })
    }
}
