//! Reference constants (VectifyAI/PageIndex @619cbd8 unless noted).

/// Attempts per call in the retry ladder. ref: pageindex/utils.py:205 (`max_retries = 10`)
pub const MAX_RETRIES: u32 = 10;

/// Pause between attempts, in seconds. ref: pageindex/utils.py:227 (`await asyncio.sleep(1)`)
pub const RETRY_DELAY_SECS: u64 = 1;

/// Statuses no retry can fix and every later call hits too. ref: pageindex/utils.py:142
pub const UNRECOVERABLE_STATUS: [u16; 3] = [401, 403, 404];

/// Statuses the ladder raises immediately (unrecoverable + per-prompt 400).
/// ref: pageindex/utils.py:147
pub const NO_RETRY_STATUS: [u16; 4] = [401, 403, 404, 400];

/// `parity/mock_llm.py::STUB_REPLY` (`json.dumps` with default separators).
pub const STUB_REPLY: &str = r#"{"subsections": [], "summary": "stub summary", "title": "stub title", "description": "stub description"}"#;

/// litellm `TIKTOKEN_ENCODE_CHUNK_SIZE_CHARS` default: token counting encodes the text in
/// chunks of this many code points and sums. ref: litellm/constants.py:423 (litellm 1.9x)
pub const TIKTOKEN_ENCODE_CHUNK_SIZE_CHARS: usize = 1024;

/// litellm `TOKEN_COUNTER_MAX_EXACT_CHARS` default; longer texts are extrapolated from samples.
/// ref: litellm/constants.py:429
pub const TOKEN_COUNTER_MAX_EXACT_CHARS: usize = 4_000_000;

/// ref: litellm/litellm_core_utils/token_counter.py:336
pub const EXTRAPOLATION_SAMPLES: usize = 16;
