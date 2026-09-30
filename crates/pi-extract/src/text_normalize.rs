//! Unicode normalization tables, whitespace classes, spacing factors and bidi reordering.
//!
//! ref: pageindex/flash/parser_pdfium_charlevel/text_normalize.py

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::pyuni;

// ref: text_normalize.py:52-56 (span merger thresholds, multiples of the x-scaled font size)
pub const TRACKING_SPACE_FACTOR: f64 = 0.1;
pub const NON_SPACE_GAP_FACTOR: f64 = 0.03;
pub const NEGATIVE_SPACE_FACTOR: f64 = -0.2;
pub const SPACE_IN_FLOW_MIN_FACTOR: f64 = 0.1;
pub const SPACE_IN_FLOW_MAX_FACTOR: f64 = 0.6;

/// ref: text_normalize.py::_WHITESPACE_CODEPOINTS (Unicode WhiteSpace + LineTerminator + FEFF).
pub fn is_whitespace(cp: u32) -> bool {
    matches!(
        cp,
        0x9 | 0xA | 0xB | 0xC | 0xD | 0x20 | 0xA0 | 0x1680 | 0x2000
            ..=0x200A | 0x2028 | 0x2029 | 0x202F | 0x205F | 0x3000 | 0xFEFF
    )
}

/// ref: text_normalize.py::_is_zero_width_diacritic
pub fn is_zero_width_diacritic(cp: u32) -> bool {
    !is_whitespace(cp) && pyuni::category(cp) == "Mn"
}

/// ref: text_normalize.py::_is_invisible_format_mark
pub fn is_invisible_format_mark(cp: u32) -> bool {
    !is_whitespace(cp) && pyuni::category(cp) == "Cf"
}

/// ref: text_normalize.py::_NORMALIZED_UNICODES (per-code-point table, NOT NFKC).
pub static NORMALIZED_UNICODES: LazyLock<HashMap<char, String>> = LazyLock::new(|| {
    let raw: HashMap<String, String> =
        serde_json::from_str(pi_data::NORMALIZED_UNICODES_JSON).expect("normalized_unicodes.json");
    raw.into_iter()
        .filter_map(|(k, v)| {
            let mut it = k.chars();
            match (it.next(), it.next()) {
                (Some(c), None) => Some((c, v)),
                _ => None, // the reference looks up single code points only
            }
        })
        .collect()
});

/// Per-glyph table lookup: `table.get(u, u)` for a single-code-point string, else unchanged.
pub fn normalized_piece(piece: &str) -> String {
    let mut it = piece.chars();
    if let (Some(c), None) = (it.next(), it.next()) {
        if let Some(v) = NORMALIZED_UNICODES.get(&c) {
            return v.clone();
        }
    }
    piece.to_string()
}

/// ref: text_normalize.py::_normalize_unicodes (per code point over a whole string).
pub fn normalize_unicodes(text: &str) -> String {
    if !text.chars().any(|c| NORMALIZED_UNICODES.contains_key(&c)) {
        return text.to_string();
    }
    text.chars()
        .map(|c| {
            NORMALIZED_UNICODES
                .get(&c)
                .cloned()
                .unwrap_or_else(|| c.to_string())
        })
        .collect()
}

/// ref: text_normalize.py::_DROP_CHARS (str.translate table).
pub fn translate_drop_chars(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\u{FFFE}' => {}
            '\t' | '\n' | '\r' => out.push(' '),
            '\u{AD}' => out.push('-'),
            _ => out.push(c),
        }
    }
    out
}

const BIDI_BASE_TYPES: &str = "BN BN BN BN BN BN BN BN BN S B S WS B BN BN BN BN BN BN BN BN BN BN BN BN \
BN BN B B B S WS ON ON ET ET ET ON ON ON ON ON ES CS ES CS CS EN EN EN EN \
EN EN EN EN EN EN CS ON ON ON ON ON ON L L L L L L L L L L L L L L L L L L \
L L L L L L L L ON ON ON ON ON ON L L L L L L L L L L L L L L L L L L L L \
L L L L L L ON ON ON ON BN BN BN BN BN BN B BN BN BN BN BN BN BN BN BN BN \
BN BN BN BN BN BN BN BN BN BN BN BN BN BN BN BN CS ON ET ET ET ET ON ON ON \
ON L ON ON BN ON ON ET ET EN EN ON L ON ON ON EN L ON ON ON ON ON L L L L \
L L L L L L L L L L L L L L L L L L L ON L L L L L L L L L L L L L L L L L \
L L L L L L L L L L L L L L ON L L L L L L L L ";

const BIDI_ARABIC_TYPES: &str = "AN AN AN AN AN AN ON ON AL ET ET AL CS AL ON ON NSM NSM NSM NSM NSM NSM \
NSM NSM NSM NSM NSM AL AL ~ AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL \
AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL \
AL AL AL AL AL NSM NSM NSM NSM NSM NSM NSM NSM NSM NSM NSM NSM NSM NSM NSM \
NSM NSM NSM NSM NSM NSM AN AN AN AN AN AN AN AN AN AN ET AN AN AL AL AL \
NSM AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL \
AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL \
AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL \
AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL AL \
AL AL AL NSM NSM NSM NSM NSM NSM NSM AN ON NSM NSM NSM NSM NSM NSM AL AL \
NSM NSM ON NSM NSM NSM NSM AL AL EN EN EN EN EN EN EN EN EN EN AL AL AL AL \
AL AL ";

static BASE_TYPES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let v: Vec<&str> = BIDI_BASE_TYPES.split_whitespace().collect();
    assert_eq!(v.len(), 256);
    v
});

static ARABIC_TYPES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let v: Vec<&str> = BIDI_ARABIC_TYPES
        .split_whitespace()
        .map(|t| if t == "~" { "" } else { t })
        .collect();
    assert_eq!(v.len(), 256);
    v
});

/// ref: text_normalize.py::_apply_bidi_reordering (simplified single-line UAX#9).
pub fn apply_bidi_reordering(text: &str, start_level: i32, vertical: bool) -> String {
    if text.is_empty() || vertical {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut types: Vec<&str> = Vec::with_capacity(n);
    let mut num_bidi = 0usize;
    for &c in &chars {
        let cp = c as u32;
        let t: &str = if cp <= 0xFF {
            BASE_TYPES[cp as usize]
        } else if (0x0590..=0x05F4).contains(&cp) {
            "R"
        } else if (0x0600..=0x06FF).contains(&cp) {
            ARABIC_TYPES[(cp & 0xFF) as usize]
        } else if (0x0700..=0x08AC).contains(&cp) {
            "AL"
        } else {
            "L"
        };
        if matches!(t, "R" | "AL" | "AN") {
            num_bidi += 1;
        }
        types.push(t);
    }
    if num_bidi == 0 {
        return text.to_string();
    }
    let mut start_level = start_level;
    if start_level == -1 {
        start_level = if (num_bidi as f64) / (n as f64) < 0.3 && n > 4 {
            0
        } else {
            1
        };
    }
    let mut levels = vec![start_level; n];
    let entry: &str = if start_level & 1 == 1 { "R" } else { "L" };
    let sor = entry;
    let eor = sor;
    // W1
    let mut last = sor;
    for t in types.iter_mut() {
        if *t == "NSM" {
            *t = last;
        } else {
            last = *t;
        }
    }
    // W2
    let mut last = sor;
    for t in types.iter_mut() {
        if *t == "EN" {
            *t = if last == "AL" { "AN" } else { "EN" };
        } else if matches!(*t, "R" | "L" | "AL") {
            last = *t;
        }
    }
    // W3
    for t in types.iter_mut() {
        if *t == "AL" {
            *t = "R";
        }
    }
    // W4
    for i in 1..n.saturating_sub(1) {
        if types[i] == "ES" && types[i - 1] == "EN" && types[i + 1] == "EN" {
            types[i] = "EN";
        }
        if types[i] == "CS" && matches!(types[i - 1], "EN" | "AN") && types[i + 1] == types[i - 1] {
            types[i] = types[i - 1];
        }
    }
    // W5
    for i in 0..n {
        if types[i] == "EN" {
            for j in (0..i).rev() {
                if types[j] != "ET" {
                    break;
                }
                types[j] = "EN";
            }
            for t in types.iter_mut().skip(i + 1) {
                if *t != "ET" {
                    break;
                }
                *t = "EN";
            }
        }
    }
    // W6
    for t in types.iter_mut() {
        if matches!(*t, "WS" | "ES" | "ET" | "CS") {
            *t = "ON";
        }
    }
    // W7
    let mut last = sor;
    for t in types.iter_mut() {
        if *t == "EN" {
            *t = if last == "L" { "L" } else { "EN" };
        } else if matches!(*t, "R" | "L") {
            last = *t;
        }
    }
    // N1/N2
    let mut i = 0usize;
    while i < n {
        if types[i] == "ON" {
            let mut end = i + 1;
            while end < n && types[end] == "ON" {
                end += 1;
            }
            let mut before = if i > 0 { types[i - 1] } else { sor };
            let mut after = if end + 1 < n { types[end + 1] } else { eor };
            if before != "L" {
                before = "R";
            }
            if after != "L" {
                after = "R";
            }
            if before == after {
                for t in types.iter_mut().take(end).skip(i) {
                    *t = before;
                }
            }
            // `index_value = end - 1` then `+= 1`
            i = end - 1;
        }
        i += 1;
    }
    for t in types.iter_mut() {
        if *t == "ON" {
            *t = entry;
        }
    }
    // I1/I2
    for (lv, t) in levels.iter_mut().zip(&types) {
        if *lv % 2 == 0 {
            if *t == "R" {
                *lv += 1;
            } else if matches!(*t, "AN" | "EN") {
                *lv += 2;
            }
        } else if matches!(*t, "L" | "AN" | "EN") {
            *lv += 1;
        }
    }
    let mut highest = -1;
    let mut lowest_odd = 99;
    for &lv in &levels {
        if lv > highest {
            highest = lv;
        }
        if lv < lowest_odd && (lv & 1) == 1 {
            lowest_odd = lv;
        }
    }
    let mut level = highest;
    while level >= lowest_odd {
        let mut start: Option<usize> = None;
        for idx in 0..n {
            if levels[idx] < level {
                if let Some(s) = start.take() {
                    chars[s..idx].reverse();
                }
            } else if start.is_none() {
                start = Some(idx);
            }
        }
        if let Some(s) = start {
            chars[s..n].reverse();
        }
        level -= 1;
    }
    chars
        .into_iter()
        .filter(|&c| c != '<' && c != '>')
        .collect()
}

/// ref: text_normalize.py::_rtl_sign
pub fn rtl_sign(ch: &str) -> f64 {
    match ch.chars().next() {
        Some(c) if matches!(pyuni::bidirectional(c as u32), "R" | "AL") => -1.0,
        _ => 1.0,
    }
}

/// ref: text_normalize.py::_reverse_if_rtl
pub fn reverse_if_rtl(chars: &str) -> String {
    let mut it = chars.chars();
    let first = match it.next() {
        Some(c) => c as u32,
        None => return String::new(),
    };
    if it.next().is_none() {
        return chars.to_string();
    }
    if (0x0590..0x05FF).contains(&first) || (0x0600..0x06FF).contains(&first) {
        chars.chars().rev().collect()
    } else {
        chars.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bidi_passthrough_ltr() {
        assert_eq!(apply_bidi_reordering("Hello <x>", -1, false), "Hello <x>");
    }

    #[test]
    fn bidi_reverses_hebrew() {
        // Pure RTL run is reversed; '<' '>' dropped when bidi text present.
        assert_eq!(
            apply_bidi_reordering("\u{5D0}\u{5D1}\u{5D2}", -1, false),
            "\u{5D2}\u{5D1}\u{5D0}"
        );
    }
}
