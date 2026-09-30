//! OpenAI-compatible LLM client.
//!
//! Everything that talks to a model goes through the [`Llm`] trait, so the pipeline can run
//! against a real endpoint ([`OpenAiClient`]), a recorded fixture ([`ReplayLlm`]), a canned
//! reply ([`StubLlm`]) or any test double. Fixtures use the format of `parity/mock_llm.py`,
//! keyed by [`key_of`].

pub mod client;
pub mod consts;
mod error;
pub mod pyjson;
pub mod replay;
pub mod tokens;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use client::{OpenAiClient, RoleConfig};
pub use error::LlmError;
pub use replay::{RecordingLlm, ReplayLlm, StubLlm};
pub use tokens::count_tokens;

/// One chat message. Field order is `role`, `content`, as the reference builds them; the
/// fixture key depends on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Message {
            role: "user".to_string(),
            content: content.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, LlmError>;

/// A chat-completion backend. `complete` returns the reply text (`""` when the provider
/// returned no content, which every consumer in the reference treats like `None`).
#[async_trait::async_trait]
pub trait Llm: Send + Sync {
    async fn complete(&self, messages: &[Message]) -> Result<String>;
}

#[async_trait::async_trait]
impl<T: Llm + ?Sized> Llm for Arc<T> {
    async fn complete(&self, messages: &[Message]) -> Result<String> {
        (**self).complete(messages).await
    }
}

/// `utils.llm_acompletion(model, prompt)` (utils.py:203): a single user message.
pub async fn complete_prompt(llm: &dyn Llm, prompt: &str) -> Result<String> {
    llm.complete(&[Message::user(prompt)]).await
}

/// `json.dumps(messages, ensure_ascii=False, separators=(",", ":"))`.
pub fn canonical_messages(messages: &[Message]) -> String {
    pyjson::dumps_compact(messages)
}

/// Fixture key: sha256 hex of [`canonical_messages`]. ref: parity/mock_llm.py::key_of
pub fn key_of(messages: &[Message]) -> String {
    let digest = Sha256::digest(canonical_messages(messages).as_bytes());
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}
