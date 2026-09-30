//! Python 3.11 `unicodedata` (Unicode 14.0.0) semantics.
//!
//! General category, bidi class, `str.lower()`/`str.upper()` and the `regex` module's
//! `\p{Number}` come from tables generated from CPython by `gen/unicode_tables.py` (Spike D), so
//! they match the reference exactly by construction. Normalization uses `unicode-normalization`
//! (a newer Unicode version), guarded so that code points unassigned in Unicode 14 pass through
//! unchanged, as they do in Python.

use unicode_normalization::UnicodeNormalization;

use crate::unicode_tables as t;

pub use t::UNIDATA_VERSION;

/// `unicodedata.category` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cat {
    Lu,
    Ll,
    Lt,
    Lm,
    Lo,
    Mn,
    Mc,
    Me,
    Nd,
    Nl,
    No,
    Pc,
    Pd,
    Ps,
    Pe,
    Pi,
    Pf,
    Po,
    Sm,
    Sc,
    Sk,
    So,
    Zs,
    Zl,
    Zp,
    Cc,
    Cf,
    Cs,
    Co,
    Cn,
}

impl Cat {
    /// The two-letter name Python returns.
    pub fn as_str(self) -> &'static str {
        use Cat::*;
        match self {
            Lu => "Lu",
            Ll => "Ll",
            Lt => "Lt",
            Lm => "Lm",
            Lo => "Lo",
            Mn => "Mn",
            Mc => "Mc",
            Me => "Me",
            Nd => "Nd",
            Nl => "Nl",
            No => "No",
            Pc => "Pc",
            Pd => "Pd",
            Ps => "Ps",
            Pe => "Pe",
            Pi => "Pi",
            Pf => "Pf",
            Po => "Po",
            Sm => "Sm",
            Sc => "Sc",
            Sk => "Sk",
            So => "So",
            Zs => "Zs",
            Zl => "Zl",
            Zp => "Zp",
            Cc => "Cc",
            Cf => "Cf",
            Cs => "Cs",
            Co => "Co",
            Cn => "Cn",
        }
    }

    /// First letter of the category (`cat.startswith("L")` etc.).
    pub fn major(self) -> char {
        self.as_str().as_bytes()[0] as char
    }
}

/// `unicodedata.bidirectional` values; `None` is Python's empty string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Bidi {
    L,
    R,
    AL,
    EN,
    ES,
    ET,
    AN,
    CS,
    NSM,
    BN,
    B,
    S,
    WS,
    ON,
    LRE,
    LRO,
    RLE,
    RLO,
    PDF,
    LRI,
    RLI,
    FSI,
    PDI,
    None,
}

impl Bidi {
    pub fn as_str(self) -> &'static str {
        use Bidi::*;
        match self {
            L => "L",
            R => "R",
            AL => "AL",
            EN => "EN",
            ES => "ES",
            ET => "ET",
            AN => "AN",
            CS => "CS",
            NSM => "NSM",
            BN => "BN",
            B => "B",
            S => "S",
            WS => "WS",
            ON => "ON",
            LRE => "LRE",
            LRO => "LRO",
            RLE => "RLE",
            RLO => "RLO",
            PDF => "PDF",
            LRI => "LRI",
            RLI => "RLI",
            FSI => "FSI",
            PDI => "PDI",
            None => "",
        }
    }
}

fn lookup_run<V: Copy>(table: &[(u32, V)], cp: u32) -> V {
    // Runs start at 0 and are sorted, so the last start <= cp owns it.
    let i = table.partition_point(|&(start, _)| start <= cp);
    table[i - 1].1
}

/// `unicodedata.category(chr(cp))` for any code point (surrogates included).
pub fn category_of(cp: u32) -> Cat {
    if cp >= 0x110000 {
        return Cat::Cn;
    }
    lookup_run(&t::CATEGORY, cp)
}

/// `unicodedata.category(c)`.
pub fn category(c: char) -> Cat {
    category_of(c as u32)
}

/// `unicodedata.bidirectional(c)`.
pub fn bidirectional(c: char) -> Bidi {
    lookup_run(&t::BIDI, c as u32)
}

/// Whether the `regex` module's `\p{Number}` matches `c` (version recorded in the table header).
pub fn is_regex_number(c: char) -> bool {
    lookup_run(&t::REGEX_NUMBER, c as u32)
}

/// Whether `c` is assigned (category != Cn) in Unicode 14.
pub fn is_assigned(c: char) -> bool {
    category(c) != Cat::Cn
}

fn case_map(table: &[(u32, &'static str)], c: char) -> Option<&'static str> {
    table
        .binary_search_by_key(&(c as u32), |&(cp, _)| cp)
        .ok()
        .map(|i| table[i].1)
}

/// `str.lower()`, including CPython's context-sensitive capital sigma (`handle_capital_sigma`).
pub fn lower(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    if !s.contains('\u{03A3}') {
        for c in s.chars() {
            push_lower(c, &mut out);
        }
        return out;
    }
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c != '\u{03A3}' {
            push_lower(c, &mut out);
            continue;
        }
        // \p{cased}\p{case-ignorable}* U+03A3 !(\p{case-ignorable}* \p{cased})
        let stop_before = chars[..i]
            .iter()
            .rev()
            .map(|&d| sigma_class(d))
            .find(|&k| k != 1);
        let mut final_sigma = stop_before == Some(2);
        if final_sigma && i + 1 < chars.len() {
            let stop_after = chars[i + 1..]
                .iter()
                .map(|&d| sigma_class(d))
                .find(|&k| k != 1);
            final_sigma = stop_after != Some(2);
        }
        out.push(if final_sigma { '\u{03C2}' } else { '\u{03C3}' });
    }
    out
}

fn push_lower(c: char, out: &mut String) {
    match case_map(&t::LOWER, c) {
        Some(m) => out.push_str(m),
        None => out.push(c),
    }
}

/// Final_Sigma scan class: 0 uncased (stops the scan), 1 case-ignorable, 2 cased.
fn sigma_class(c: char) -> u8 {
    lookup_run(&t::SIGMA_CLASS, c as u32)
}

/// `str.upper()`.
pub fn upper(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match case_map(&t::UPPER, c) {
            Some(m) => out.push_str(m),
            None => out.push(c),
        }
    }
    out
}

/// Normalization form for [`normalize`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    Nfc,
    Nfd,
    Nfkc,
    Nfkd,
}

fn normalize_segment(seg: &str, form: Form, out: &mut String) {
    match form {
        Form::Nfc => out.extend(seg.nfc()),
        Form::Nfd => out.extend(seg.nfd()),
        Form::Nfkc => out.extend(seg.nfkc()),
        Form::Nfkd => out.extend(seg.nfkd()),
    }
}

/// `unicodedata.normalize(form, s)` under Unicode 14.
///
/// Code points unassigned in 14.0 are starters without decompositions or compositions in
/// Python, so the string is normalized segment by segment between them and they are copied
/// through unchanged. Normalization stability makes the newer crate agree on everything else.
pub fn normalize(form: Form, s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut start = 0;
    for (i, c) in s.char_indices() {
        if !c.is_ascii() && !is_assigned(c) {
            normalize_segment(&s[start..i], form, &mut out);
            out.push(c);
            start = i + c.len_utf8();
        }
    }
    normalize_segment(&s[start..], form, &mut out);
    out
}

/// `unicodedata.normalize("NFKC", s)`.
pub fn nfkc(s: &str) -> String {
    if s.is_ascii() {
        return s.to_string();
    }
    normalize(Form::Nfkc, s)
}
