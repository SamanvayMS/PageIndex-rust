//! `utils.count_tokens` (utils.py:85), which is `litellm.token_counter(model=model, text=text)`.
//!
//! litellm (1.9x, the version the reference venv pins) picks the encoding like this:
//! - `model=None`: the default encoding, `cl100k_base`;
//! - otherwise `_fix_model_name`: a name outside litellm's OpenAI chat list becomes
//!   `gpt-3.5-turbo` (so `openai/...`-prefixed names and custom names count with `cl100k_base`);
//!   a listed name containing `gpt-4o` uses `o200k_base`, else `tiktoken.encoding_for_model`.
//! - special tokens count as ordinary text (`disallowed_special=()`), and the text is encoded in
//!   chunks of 1024 code points whose counts are summed.
//!
//! Not ported: litellm's HuggingFace tokenizers for Claude, Llama and Cohere model names (they
//! need network downloads); those names count with `cl100k_base` here. See KNOWN_DIFFS.

use std::sync::OnceLock;

use tiktoken_rs::CoreBPE;

use crate::consts::{
    EXTRAPOLATION_SAMPLES, TIKTOKEN_ENCODE_CHUNK_SIZE_CHARS, TOKEN_COUNTER_MAX_EXACT_CHARS,
};

/// Names in litellm's `open_ai_chat_completion_models` whose tiktoken encoding is `o200k_base`
/// (every other listed name, and every unlisted one, is `cl100k_base`). Generated from the
/// reference venv's litellm; see crates/pi-llm/gen/llm_fixture.py.
const O200K_MODELS: &[&str] = &[
    "chatgpt-4o-latest",
    "gpt-4.1",
    "gpt-4.1-2025-04-14",
    "gpt-4.1-mini",
    "gpt-4.1-mini-2025-04-14",
    "gpt-4.1-nano",
    "gpt-4.1-nano-2025-04-14",
    "gpt-4o",
    "gpt-4o-2024-05-13",
    "gpt-4o-2024-08-06",
    "gpt-4o-2024-11-20",
    "gpt-4o-audio-preview",
    "gpt-4o-audio-preview-2024-12-17",
    "gpt-4o-audio-preview-2025-06-03",
    "gpt-4o-mini",
    "gpt-4o-mini-2024-07-18",
    "gpt-4o-mini-audio-preview",
    "gpt-4o-mini-audio-preview-2024-12-17",
    "gpt-4o-mini-realtime-preview",
    "gpt-4o-mini-realtime-preview-2024-12-17",
    "gpt-4o-mini-search-preview",
    "gpt-4o-mini-search-preview-2025-03-11",
    "gpt-4o-mini-transcribe",
    "gpt-4o-mini-transcribe-2025-03-20",
    "gpt-4o-mini-transcribe-2025-12-15",
    "gpt-4o-mini-tts",
    "gpt-4o-mini-tts-2025-03-20",
    "gpt-4o-mini-tts-2025-12-15",
    "gpt-4o-realtime-preview",
    "gpt-4o-realtime-preview-2024-12-17",
    "gpt-4o-realtime-preview-2025-06-03",
    "gpt-4o-search-preview",
    "gpt-4o-search-preview-2025-03-11",
    "gpt-4o-transcribe",
    "gpt-4o-transcribe-diarize",
    "gpt-5",
    "gpt-5-2025-08-07",
    "gpt-5-chat",
    "gpt-5-chat-latest",
    "gpt-5-codex",
    "gpt-5-mini",
    "gpt-5-mini-2025-08-07",
    "gpt-5-nano",
    "gpt-5-nano-2025-08-07",
    "gpt-5-pro",
    "gpt-5-pro-2025-10-06",
    "gpt-5-search-api",
    "gpt-5-search-api-2025-10-14",
    "gpt-5.1",
    "gpt-5.1-2025-11-13",
    "gpt-5.1-chat-latest",
    "gpt-5.1-codex",
    "gpt-5.1-codex-max",
    "gpt-5.1-codex-mini",
    "gpt-5.2",
    "gpt-5.2-2025-12-11",
    "gpt-5.2-chat-latest",
    "gpt-5.2-codex",
    "gpt-5.2-pro",
    "gpt-5.2-pro-2025-12-11",
    "gpt-5.3-chat-latest",
    "gpt-5.3-codex",
    "gpt-5.4",
    "gpt-5.4-2026-03-05",
    "gpt-5.4-mini",
    "gpt-5.4-mini-2026-03-17",
    "gpt-5.4-nano",
    "gpt-5.4-nano-2026-03-17",
    "gpt-5.4-pro",
    "gpt-5.4-pro-2026-03-05",
    "gpt-5.5",
    "gpt-5.5-2026-04-23",
    "gpt-5.5-cyber",
    "gpt-5.5-pro",
    "gpt-5.5-pro-2026-04-23",
    "gpt-5.6",
    "gpt-5.6-cyber",
    "gpt-5.6-luna",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "o1",
    "o1-2024-12-17",
    "o1-pro",
    "o1-pro-2025-03-19",
    "o3",
    "o3-2025-04-16",
    "o3-deep-research",
    "o3-deep-research-2025-06-26",
    "o3-mini",
    "o3-mini-2025-01-31",
    "o3-pro",
    "o3-pro-2025-06-10",
    "o4-mini",
    "o4-mini-2025-04-16",
    "o4-mini-deep-research",
    "o4-mini-deep-research-2025-06-26",
];

fn cl100k() -> &'static CoreBPE {
    static ENC: OnceLock<CoreBPE> = OnceLock::new();
    ENC.get_or_init(|| tiktoken_rs::cl100k_base().expect("bundled cl100k_base"))
}

fn o200k() -> &'static CoreBPE {
    static ENC: OnceLock<CoreBPE> = OnceLock::new();
    ENC.get_or_init(|| tiktoken_rs::o200k_base().expect("bundled o200k_base"))
}

/// The encoding litellm's `token_counter` uses for `model`.
fn encoding_for(model: Option<&str>) -> &'static CoreBPE {
    match model {
        Some(m) if O200K_MODELS.contains(&m) => o200k(),
        _ => cl100k(),
    }
}

fn exact_count(enc: &CoreBPE, chars: &[char]) -> usize {
    // litellm `_get_tiktoken_count_function`: encode per 1024-code-point chunk and sum
    chars
        .chunks(TIKTOKEN_ENCODE_CHUNK_SIZE_CHARS)
        .map(|chunk| {
            let s: String = chunk.iter().collect();
            enc.encode_ordinary(&s).len()
        })
        .sum()
}

/// `count_tokens(text, model)`.
pub fn count_tokens(text: &str, model: Option<&str>) -> usize {
    if text.is_empty() {
        return 0;
    }
    let enc = encoding_for(model);
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= TOKEN_COUNTER_MAX_EXACT_CHARS {
        return exact_count(enc, &chars);
    }
    // litellm `_get_extrapolating_count_function` / `_evenly_spaced_samples`
    let total = TOKEN_COUNTER_MAX_EXACT_CHARS;
    let sample_count = EXTRAPOLATION_SAMPLES.min(total);
    let sample_chars = total / sample_count;
    let last_start = chars.len() - sample_chars;
    let denom = (sample_count - 1).max(1);
    let mut sampled = 0usize;
    let mut counted = 0usize;
    for index in 0..sample_count {
        let start = last_start * index / denom;
        let end = (start + sample_chars).min(chars.len());
        sampled += end - start;
        counted += exact_count(enc, &chars[start..end]);
    }
    let est = counted as f64 * chars.len() as f64 / sampled as f64;
    pi_pycompat::pyround::round0(est) as usize
}
