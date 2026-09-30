//! Port of `pageindex/naming.py` (naming v1): the stored document name.

use md5::{Digest, Md5};
use unicode_normalization::UnicodeNormalization;

use crate::consts::MAX_NAME_BYTES;

/// `_CONTROLS`. // ref: naming.py:9
fn is_control(c: char) -> bool {
    matches!(c, '\u{00}'..='\u{1F}' | '\u{7F}'..='\u{9F}' | '\u{2028}' | '\u{2029}')
}

/// `_ILLEGAL`. // ref: naming.py:10
fn is_illegal(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
}

/// `_WHITESPACE` (the JavaScript set). // ref: naming.py:14
fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// `_RESERVED.fullmatch` (case-insensitive). // ref: naming.py:12
fn is_reserved(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let (stem, rest) = match lower.find('.') {
        Some(i) => (&lower[..i], Some(&lower[i..])),
        None => (lower.as_str(), None),
    };
    // `(?:\..*)?$`: `.` excludes "\n", and `$` also matches before a final "\n".
    if let Some(rest) = rest {
        let body = rest.strip_suffix('\n').unwrap_or(rest);
        if body.contains('\n') {
            return false;
        }
    }
    let stem = if rest.is_none() {
        stem.strip_suffix('\n').unwrap_or(stem)
    } else {
        stem
    };
    matches!(stem, "con" | "prn" | "aux" | "nul")
        || ((stem.starts_with("com") || stem.starts_with("lpt"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit())
}

/// `normalize_filename`: NFKC, quote folding, JS whitespace runs to one space, strip " ".
/// // ref: naming.py:20
pub fn normalize_filename(name: &str) -> String {
    let folded: String = name
        .nfkc()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{02BC}' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            c => c,
        })
        .collect();
    let mut out = String::with_capacity(folded.len());
    let mut in_space = false;
    for c in folded.chars() {
        if is_js_space(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out.trim_matches(' ').to_string()
}

/// `_prefix`: the longest whole-character UTF-8 prefix of at most `max_bytes` bytes.
fn prefix(value: &str, max_bytes: isize) -> &str {
    let max = max_bytes.max(0) as usize;
    if value.len() <= max {
        return value;
    }
    let mut end = max;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

/// `os.path.splitext` for a single path component.
fn splitext(name: &str) -> (&str, &str) {
    let sep = name.rfind('/').map(|i| i + 1).unwrap_or(0);
    if let Some(dot) = name.rfind('.')
        && dot > sep
        && name[sep..dot].chars().any(|c| c != '.')
    {
        return (&name[..dot], &name[dot..]);
    }
    (name, "")
}

/// `truncate_filename`. // ref: naming.py:31
pub fn truncate_filename(name: &str, max_bytes: usize, suffix: &str) -> Result<String, String> {
    let (base, ext) = splitext(name);
    let candidate = format!("{base}{suffix}{ext}");
    if candidate.len() <= max_bytes {
        return Ok(candidate);
    }
    let digest = Md5::digest(name.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    let ending = format!("_{}{suffix}", &hex[..8]);
    let budget = max_bytes as isize - ending.len() as isize;
    if budget < 4 {
        return Err("Filename byte limit is too small for its suffix".into());
    }
    let first_bytes = base.chars().next().map_or(1, char::len_utf8) as isize;
    let ext = prefix(ext, budget - first_bytes).trim_end_matches([' ', '.']);
    let base = match prefix(base, budget - ext.len() as isize) {
        "" => "_",
        b => b,
    };
    Ok(format!("{base}{ending}{ext}"))
}

/// `sanitize_filename` with the default byte budget. // ref: naming.py:52
pub fn sanitize_filename(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| if is_control(c) { '_' } else { c })
        .collect();
    let normalized: String = normalize_filename(&replaced)
        .chars()
        .map(|c| if is_illegal(c) { '_' } else { c })
        .collect();
    let mut name = normalized.trim_end_matches([' ', '.']).to_string();
    if name.is_empty() {
        name = "untitled".into();
    }
    if is_reserved(&name) {
        name.insert(0, '_');
    }
    truncate_filename(&name, MAX_NAME_BYTES, "").expect("default budget fits any suffix")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_examples() {
        assert_eq!(sanitize_filename("a/b:c.pdf"), "a_b_c.pdf");
        assert_eq!(
            sanitize_filename("  “Q3”\u{3000}Report .pdf. "),
            "_Q3_ Report .pdf"
        );
        // Expected values from CPython (pageindex.naming @619cbd8).
        let long = sanitize_filename(&format!("{}.pdf", "a".repeat(300)));
        assert_eq!(long, format!("{}_5f3ffb0d.pdf", "a".repeat(167)));
        let accents = sanitize_filename(&format!("{}.pdf", "é".repeat(100)));
        assert_eq!(accents, format!("{}_70814503.pdf", "é".repeat(83)));
        let ext = sanitize_filename(&format!("x.{}", "e".repeat(300)));
        assert_eq!(ext, format!("x_89aefc49.{}", "e".repeat(169)));
        assert_eq!(sanitize_filename("CON.pdf"), "_CON.pdf");
        assert_eq!(sanitize_filename("com1"), "_com1");
        assert_eq!(sanitize_filename("..."), "untitled");
        assert_eq!(sanitize_filename("ﬁle.pdf"), "file.pdf");
    }

    #[test]
    fn truncate_long_names() {
        let long = format!("{}.pdf", "a".repeat(300));
        let out = truncate_filename(&long, MAX_NAME_BYTES, "_1").unwrap();
        assert_eq!(out, format!("{}_5f3ffb0d_1.pdf", "a".repeat(165)));
        assert_eq!(splitext(".bashrc"), (".bashrc", ""));
        assert_eq!(splitext("a.b.c"), ("a.b", ".c"));
    }
}
