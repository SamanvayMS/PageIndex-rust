//! Python `unicodedata` shims used by the extractor.
//!
//! TODO(parity): switch to the tables generated from Python 3.11 (Unicode 14.0) in
//! `pi-pycompat` once they land (docs/spikes/D-unicode.md). Until then these use the Rust
//! Unicode crates, which differ only for code points assigned after Unicode 14.

use unicode_bidi::BidiClass;
use unicode_general_category::{GeneralCategory as G, get_general_category};
use unicode_normalization::UnicodeNormalization;

/// `unicodedata.category(chr(cp))`; lone surrogates are "Cs", invalid values "Cn".
pub fn category(cp: u32) -> &'static str {
    let Some(c) = char::from_u32(cp) else {
        return if (0xD800..=0xDFFF).contains(&cp) {
            "Cs"
        } else {
            "Cn"
        };
    };
    match get_general_category(c) {
        G::UppercaseLetter => "Lu",
        G::LowercaseLetter => "Ll",
        G::TitlecaseLetter => "Lt",
        G::ModifierLetter => "Lm",
        G::OtherLetter => "Lo",
        G::NonspacingMark => "Mn",
        G::SpacingMark => "Mc",
        G::EnclosingMark => "Me",
        G::DecimalNumber => "Nd",
        G::LetterNumber => "Nl",
        G::OtherNumber => "No",
        G::ConnectorPunctuation => "Pc",
        G::DashPunctuation => "Pd",
        G::OpenPunctuation => "Ps",
        G::ClosePunctuation => "Pe",
        G::InitialPunctuation => "Pi",
        G::FinalPunctuation => "Pf",
        G::OtherPunctuation => "Po",
        G::MathSymbol => "Sm",
        G::CurrencySymbol => "Sc",
        G::ModifierSymbol => "Sk",
        G::OtherSymbol => "So",
        G::SpaceSeparator => "Zs",
        G::LineSeparator => "Zl",
        G::ParagraphSeparator => "Zp",
        G::Control => "Cc",
        G::Format => "Cf",
        G::Surrogate => "Cs",
        G::PrivateUse => "Co",
        G::Unassigned => "Cn",
        _ => "Cn",
    }
}

/// `unicodedata.bidirectional(chr(cp))` for the classes the extractor tests.
pub fn bidirectional(cp: u32) -> &'static str {
    let Some(c) = char::from_u32(cp) else {
        return "";
    };
    match unicode_bidi::bidi_class(c) {
        BidiClass::R => "R",
        BidiClass::AL => "AL",
        BidiClass::L => "L",
        BidiClass::AN => "AN",
        BidiClass::EN => "EN",
        _ => "other",
    }
}

pub fn nfkc(s: &str) -> String {
    s.nfkc().collect()
}

pub fn nfkd(s: &str) -> String {
    s.nfkd().collect()
}

pub fn nfd(s: &str) -> String {
    s.nfd().collect()
}
