//! Thin OpenAI-compatible `/chat/completions` client with the reference's retry ladder.
//!
//! ref: pageindex/utils.py:203 `llm_acompletion`. The reference goes through LiteLLM; this
//! client speaks the OpenAI wire format directly, which is what LiteLLM does for `openai/...`
//! models pointed at `OPENAI_BASE_URL`.

use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::Semaphore;

use crate::consts::{MAX_RETRIES, RETRY_DELAY_SECS};
use crate::{Llm, LlmError, Message, Result};

/// Per-role endpoint configuration (indexing lane: summary / expand / OCR ...).
#[derive(Debug, Clone)]
pub struct RoleConfig {
    /// e.g. `https://api.openai.com/v1`; `/chat/completions` is appended.
    pub base_url: String,
    /// Model id. `litellm/` and `openai/` prefixes are stripped as LiteLLM does before it sends
    /// the request (utils.py:113 `_litellm_model`); any other `provider/` prefix is sent as is.
    pub model: String,
    /// Bearer token, already resolved by the caller (e.g. from the env var its config names).
    pub api_key: Option<String>,
    /// Cap on simultaneous HTTP requests for this role.
    pub concurrency: usize,
    /// Per-request timeout.
    pub timeout: Duration,
    /// Attempts in the retry ladder (reference: 10).
    pub max_retries: u32,
    /// Pause between attempts (reference: 1 s).
    pub retry_delay: Duration,
}

impl RoleConfig {
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        RoleConfig {
            base_url: base_url.into(),
            model: model.into(),
            api_key: None,
            concurrency: 64,
            timeout: Duration::from_secs(600),
            max_retries: MAX_RETRIES,
            retry_delay: Duration::from_secs(RETRY_DELAY_SECS),
        }
    }
}

/// The model id as it goes on the wire.
pub fn wire_model(model: &str) -> &str {
    let model = model.strip_prefix("litellm/").unwrap_or(model);
    model.strip_prefix("openai/").unwrap_or(model)
}

pub struct OpenAiClient {
    cfg: RoleConfig,
    http: reqwest::Client,
    gate: Semaphore,
}

impl OpenAiClient {
    pub fn new(cfg: RoleConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .build()
            .map_err(|e| LlmError::Other(format!("building HTTP client: {e}")))?;
        let gate = Semaphore::new(cfg.concurrency.max(1));
        Ok(OpenAiClient { cfg, http, gate })
    }

    pub fn config(&self) -> &RoleConfig {
        &self.cfg
    }

    fn url(&self) -> String {
        format!(
            "{}/chat/completions",
            self.cfg.base_url.trim_end_matches('/')
        )
    }

    /// One attempt.
    async fn attempt(&self, messages: &[Message]) -> Result<String> {
        let _permit = self
            .gate
            .acquire()
            .await
            .map_err(|e| LlmError::Other(e.to_string()))?;
        let body = json!({"model": wire_model(&self.cfg.model), "messages": messages});
        let mut req = self.http.post(self.url()).json(&body);
        if let Some(key) = &self.cfg.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| LlmError::Other(e.to_string()))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| LlmError::Other(e.to_string()))?;
        if !status.is_success() {
            return Err(LlmError::Status {
                status: status.as_u16(),
                message: text.chars().take(500).collect(),
            });
        }
        parse_content(&text)
    }
}

/// `response.choices[0].message.content`; `null` content is returned as `""`.
pub fn parse_content(body: &str) -> Result<String> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| LlmError::Other(format!("unparsable completion response: {e}")))?;
    let msg = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .ok_or_else(|| LlmError::Other("completion response has no choices[0].message".into()))?;
    Ok(match msg.get("content") {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    })
}

/// The retry ladder of `llm_acompletion` (utils.py:211-232), generic over one attempt.
pub async fn with_retries<F, Fut>(
    max_retries: u32,
    delay: Duration,
    mut attempt: F,
) -> Result<String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<String>>,
{
    let max_retries = max_retries.max(1);
    let mut i = 0;
    loop {
        match attempt().await {
            Ok(content) => return Ok(content),
            Err(e) if e.is_no_retry() => return Err(e),
            Err(e) => {
                if i + 1 < max_retries {
                    tokio::time::sleep(delay).await;
                    i += 1;
                } else {
                    return Err(LlmError::RetriesExhausted {
                        attempts: max_retries,
                        status: e.status_code(),
                        message: e.to_string(),
                    });
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl Llm for OpenAiClient {
    async fn complete(&self, messages: &[Message]) -> Result<String> {
        with_retries(self.cfg.max_retries, self.cfg.retry_delay, || {
            self.attempt(messages)
        })
        .await
    }
}
