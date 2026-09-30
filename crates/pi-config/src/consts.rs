//! Defaults, several derived from the reference (VectifyAI/PageIndex @619cbd8).

/// LLM call attempts per prompt. // ref: pageindex/utils.py:168
pub const LLM_MAX_RETRIES: u32 = 10;
/// Summary lane concurrency ("None uses the library defaults (64 and 32)").
/// // ref: pageindex/flash/api.py:169
pub const SUMMARY_CONCURRENCY: usize = 64;
/// Expand lane ceiling. // ref: pageindex/tree_optimize.py:70
pub const EXPAND_CONCURRENCY: usize = 32;
/// The document description is one call per document.
pub const DESCRIPTION_CONCURRENCY: usize = 1;
/// Chat turns are sequential.
pub const CHAT_CONCURRENCY: usize = 1;
/// litellm's default request timeout (the reference sets none of its own).
pub const LLM_TIMEOUT_S: f64 = 600.0;
/// Default key variable for every LLM role.
pub const LLM_KEY_ENV: &str = "PI_LLM_KEY";
/// Summary word cap ("None uses the library default (150)"). // ref: pageindex/flash/api.py:169
pub const SUMMARY_MAX_WORDS: u32 = 150;

/// OCR endpoint defaults. // ref: docs/spikes/E-ocr-endpoint.md (Config)
pub const OCR_DPI: u32 = 200;
pub const OCR_CONCURRENCY: usize = 8;
pub const OCR_JSON_MODE: bool = true;
pub const OCR_PROFILE: &str = "spans-json";
pub const OCR_TIMEOUT_S: f64 = 120.0;
pub const OCR_KEY_ENV: &str = "PI_OCR_KEY";

/// The local index directory. // ref: pageindex/client.py:654
pub const INDEX_ROOT: &str = ".pageindex";

/// LLM roles, in resolution order.
pub const ROLES: [&str; 4] = ["summary", "expand", "description", "chat"];
