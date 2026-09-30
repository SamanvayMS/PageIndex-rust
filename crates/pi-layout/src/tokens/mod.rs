//! Tokenizer subsystem: character-driven line tokenizer, token views, keyword tries and the
//! Jenkins string hash.
//!
//! ref: pageindex/flash/tokens/

pub mod hashing;
pub mod token_types;
pub mod tokenizer;
pub mod tries;

pub use hashing::jenkins_hash;
pub use token_types::{
    Token, TokenAnchor, TokenView, is_char_token, is_trimmable_token, is_word_token,
    token_numeric_value,
};
pub use tokenizer::{clamp_value, is_superscript_adjacent, tokenize_block, tokenize_lines};
pub use tries::{Trie, de_norm, strip_leading_if_in, strip_trie_match, trim_trailing_punct};
