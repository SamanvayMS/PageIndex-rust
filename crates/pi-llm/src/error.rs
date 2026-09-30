use crate::consts::{NO_RETRY_STATUS, UNRECOVERABLE_STATUS};

/// A failed model call, classified the way the reference's retry ladder classifies it.
#[derive(Debug, Clone, thiserror::Error)]
pub enum LlmError {
    /// The endpoint answered with an HTTP error status. Statuses in `NO_RETRY_STATUS` reach the
    /// caller as this variant (the ladder raises them at once).
    #[error("HTTP {status}: {message}")]
    Status { status: u16, message: String },
    /// `LLMRetriesExhausted` (utils.py:150): every attempt failed; carries the last status.
    #[error("LLM completion failed after {attempts} retries: {message}")]
    RetriesExhausted {
        attempts: u32,
        status: Option<u16>,
        message: String,
    },
    /// A replay fixture had no reply for this prompt (`mock_llm.MockMiss`).
    #[error("no recorded reply for prompt {key}")]
    Miss { key: String },
    /// Anything else (transport failure, unparsable response body, I/O while recording).
    #[error("{0}")]
    Other(String),
}

impl LlmError {
    /// `getattr(exc, "status_code", None)`.
    pub fn status_code(&self) -> Option<u16> {
        match self {
            LlmError::Status { status, .. } => Some(*status),
            LlmError::RetriesExhausted { status, .. } => *status,
            _ => None,
        }
    }

    /// Whether the ladder raises this error without retrying. ref: utils.py:222
    pub fn is_no_retry(&self) -> bool {
        matches!(self, LlmError::Status { status, .. } if NO_RETRY_STATUS.contains(status))
    }

    /// `utils._is_unrecoverable` (utils.py:158): an exhausted ladder is fatal unless its last
    /// error was the per-prompt 400; a raw error is fatal when its status is 401/403/404.
    /// A replay miss is absorbed like any status-less error, as `mock_llm.MockMiss` (a plain
    /// `RuntimeError`) is; replay users check `ReplayLlm::misses()` instead.
    pub fn is_unrecoverable(&self) -> bool {
        match self {
            LlmError::RetriesExhausted { status, .. } => *status != Some(400),
            LlmError::Status { status, .. } => UNRECOVERABLE_STATUS.contains(status),
            LlmError::Miss { .. } | LlmError::Other(_) => false,
        }
    }
}
