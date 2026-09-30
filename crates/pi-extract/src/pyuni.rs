//! Python `unicodedata` shims used by the extractor, backed by the Unicode 14 tables generated
//! from Python 3.11 in `pi-pycompat` (docs/spikes/D-unicode.md).

use pi_pycompat::unicode::{self as u, Form};

/// `unicodedata.category(chr(cp))`; lone surrogates are "Cs".
pub fn category(cp: u32) -> &'static str {
    u::category_of(cp).as_str()
}

/// `unicodedata.bidirectional(chr(cp))`.
pub fn bidirectional(cp: u32) -> &'static str {
    match char::from_u32(cp) {
        Some(c) => u::bidirectional(c).as_str(),
        None => "",
    }
}

pub fn nfkc(s: &str) -> String {
    u::normalize(Form::Nfkc, s)
}

pub fn nfkd(s: &str) -> String {
    u::normalize(Form::Nfkd, s)
}

pub fn nfd(s: &str) -> String {
    u::normalize(Form::Nfd, s)
}
