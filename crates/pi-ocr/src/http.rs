//! Shared OpenAI-compatible chat-completions transport for OCR engines.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine as _;

/// `data:image/png;base64,...` URL for an image part.
pub fn png_data_url(png: &[u8]) -> String {
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    )
}

/// POST `{base_url}/chat/completions`; returns `choices[0].message.content`.
pub async fn post_chat(
    http: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
    body: &serde_json::Value,
) -> Result<String> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let mut req = http.post(&url).json(body);
    if let Some(k) = api_key {
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
    let v: serde_json::Value = serde_json::from_str(&text).context("OCR response is not JSON")?;
    Ok(v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string())
}

fn retryable(e: &anyhow::Error) -> bool {
    let s = e.to_string();
    !(s.contains(" 400 ") || s.contains(" 401 ") || s.contains(" 403 ") || s.contains(" 404 "))
}

/// `post_chat` with exponential backoff on transport errors, 429 and 5xx.
pub async fn post_chat_retrying(
    http: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
    body: &serde_json::Value,
    max_retries: u32,
) -> Result<String> {
    let mut attempt = 0;
    loop {
        match post_chat(http, base_url, api_key, body).await {
            Ok(r) => return Ok(r),
            Err(e) if attempt < max_retries && retryable(&e) => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(500 * (1 << attempt.min(6)))).await;
            }
            Err(e) => return Err(e),
        }
    }
}

pub fn client(timeout_s: u64) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_s))
        .build()?)
}
