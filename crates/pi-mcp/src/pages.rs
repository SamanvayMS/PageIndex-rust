//! Page specifications: `_expand_pages` and `_format_page_spec`.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::consts::MAX_REQUESTED_PAGES;

/// Page numbers are unbounded Python ints; `u128` covers every realistic spec.
pub type Page = u128;

/// `_PageSpecError`: `code` is `invalid`, `too_many` or `nonpositive`.
/// // ref: pageindex/agent_tools.py:522
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageSpecError {
    pub code: &'static str,
    pub message: String,
}

fn err(code: &'static str, message: String) -> PageSpecError {
    PageSpecError { code, message }
}

/// `\d+(-\d+)?` over ASCII digits (the contract pattern is matched with `re.ASCII`).
fn is_range(s: &str) -> bool {
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    match s.split_once('-') {
        Some((a, b)) => digits(a) && digits(b),
        None => digits(s),
    }
}

/// `_PAGE_SPEC_RE.fullmatch`: `^(\d+(-\d+)?)(,\s*\d+(-\d+)?)*$`, ASCII `\s`.
/// // ref: pageindex/agent_tools.py:531-533
pub fn matches_page_pattern(spec: &str) -> bool {
    spec.split(',').enumerate().all(|(i, part)| {
        let body = if i == 0 {
            part
        } else {
            part.trim_start_matches([' ', '\t', '\n', '\r', '\u{0B}', '\u{0C}'])
        };
        is_range(body)
    })
}

fn parse_page(s: &str) -> Page {
    // Digits only (pattern-checked); saturate beyond u128.
    s.bytes().fold(0u128, |a, b| {
        a.saturating_mul(10).saturating_add((b - b'0') as u128)
    })
}

/// `_expand_pages`: sorted distinct pages of a spec such as `"1-3,7"`.
/// // ref: pageindex/agent_tools.py:536
pub fn expand_pages(pages: &Value) -> Result<Vec<Page>, PageSpecError> {
    let Value::String(spec) = pages else {
        return Err(err(
            "invalid",
            format!(
                "Invalid page specification: {}",
                pi_store::pyjson::py_repr(pages)
            ),
        ));
    };
    if !matches_page_pattern(spec) {
        return Err(err(
            "invalid",
            format!("Invalid page specification '{spec}'"),
        ));
    }
    let too_many = || {
        err(
            "too_many",
            format!(
                "Page specification '{spec}' spans more than {MAX_REQUESTED_PAGES} pages; \
                 request a narrower range"
            ),
        )
    };
    let mut expanded: BTreeSet<Page> = BTreeSet::new();
    for part in spec.split(',') {
        let part = pi_pycompat::pystr::strip(part);
        let (start, end) = match part.split_once('-') {
            Some((a, b)) => {
                let (start, end) = (parse_page(a), parse_page(b));
                if start > end {
                    return Err(err(
                        "invalid",
                        format!("Invalid range '{part}': start must be <= end"),
                    ));
                }
                (start, end)
            }
            None => {
                let p = parse_page(part);
                (p, p)
            }
        };
        if (end - start).saturating_add(1) > MAX_REQUESTED_PAGES {
            return Err(too_many());
        }
        expanded.extend(start..=end);
        if expanded.len() as u128 > MAX_REQUESTED_PAGES {
            return Err(too_many());
        }
    }
    if expanded.first().is_some_and(|&p| p < 1) {
        return Err(err(
            "nonpositive",
            "Invalid page numbers. Page numbers must be positive integers".into(),
        ));
    }
    Ok(expanded.into_iter().collect())
}

/// `_format_page_spec`: compress `[1,2,3,5]` into `"1-3,5"`.
/// // ref: pageindex/agent_tools.py:630
pub fn format_page_spec(pages: &[Page]) -> String {
    let ordered: Vec<Page> = pages
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let Some(&first) = ordered.first() else {
        return String::new();
    };
    let mut ranges = Vec::new();
    let (mut start, mut prev) = (first, first);
    let render = |s: Page, p: Page| {
        if s == p {
            s.to_string()
        } else {
            format!("{s}-{p}")
        }
    };
    for &page in &ordered[1..] {
        if page == prev + 1 {
            prev = page;
            continue;
        }
        ranges.push(render(start, prev));
        start = page;
        prev = page;
    }
    ranges.push(render(start, prev));
    ranges.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn contract_pattern() {
        assert_eq!(expand_pages(&json!("1-3, 7")).unwrap(), vec![1, 2, 3, 7]);
        for bad in [
            "1_0", "+5", "٥", "１", " 1", "1 - 3", "abc", "1,,2", "-3", "", "1,",
        ] {
            assert_eq!(
                expand_pages(&json!(bad)).unwrap_err().code,
                "invalid",
                "{bad:?}"
            );
        }
        assert_eq!(expand_pages(&json!("5-3")).unwrap_err().code, "invalid");
        assert_eq!(expand_pages(&json!("0")).unwrap_err().code, "nonpositive");
        assert_eq!(
            expand_pages(&json!("1-1000000000")).unwrap_err().code,
            "too_many"
        );
        assert_eq!(
            expand_pages(&json!("1-6000,5001-10001")).unwrap_err().code,
            "too_many"
        );
        assert_eq!(expand_pages(&json!("1-6000,1-6000")).unwrap().len(), 6000);
        assert_eq!(expand_pages(&json!(5)).unwrap_err().code, "invalid");
    }

    #[test]
    fn format_spec() {
        assert_eq!(format_page_spec(&[1, 2, 3, 5]), "1-3,5");
        assert_eq!(format_page_spec(&[9, 5]), "5,9");
        assert_eq!(format_page_spec(&[]), "");
    }
}
