//! Reading summary / title replies. ref: pageindex/utils.py:822-874

use pi_optimize::expand::{truthy, unfence};
use pi_pycompat::pystr;
use serde_json::Value;

use crate::pyrepr::py_str;

/// `_reply_json(reply)`: the JSON object in a model reply, or `None` when none of it parses.
/// Repairs (whitespace collapse, trailing commas) are tried only after the reply fails to
/// parse as written. ref: utils.py:822
pub fn reply_json(reply: &str) -> Option<Value> {
    let mut text = pystr::strip(reply);
    if text.is_empty() {
        return None;
    }
    if text.contains("```") {
        text = unfence(text);
    }
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    if end <= start {
        return None;
    }
    let obj = &text[start..=end];
    let collapsed = pystr::split(obj).join(" ");
    let repaired = collapsed.replace(",]", "]").replace(",}", "}");
    [obj, collapsed.as_str(), repaired.as_str()]
        .into_iter()
        .find_map(|c| serde_json::from_str(c).ok())
}

/// `' '.join(str(item).strip() for item in items if str(item).strip())`.
fn join_items(items: &[Value]) -> String {
    items
        .iter()
        .map(|v| pystr::strip(&py_str(v)).to_string())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `parse_summary(reply)`: the `summary` field, or the reply itself when there is no such
/// field. ref: utils.py:847
pub fn parse_summary(reply: &str) -> String {
    if pystr::strip(reply).is_empty() {
        return String::new();
    }
    if let Some(Value::Object(m)) = reply_json(reply)
        && let Some(summary) = m.get("summary")
    {
        let summary = match summary {
            Value::Array(items) => Value::String(join_items(items)),
            other => other.clone(),
        };
        return if truthy(&summary) {
            pystr::strip(&py_str(&summary)).to_string()
        } else {
            String::new()
        };
    }
    pystr::strip(reply).to_string()
}

/// `parse_title(reply)`: the `title` field with whitespace collapsed, or `""`.
/// ref: utils.py:861
pub fn parse_title(reply: &str) -> String {
    let Some(Value::Object(m)) = reply_json(reply) else {
        return String::new();
    };
    let title = match m.get("title") {
        Some(Value::Array(items)) => Value::String(join_items(items)),
        Some(v) => v.clone(),
        None => Value::Null,
    };
    if truthy(&title) {
        pystr::split(&py_str(&title)).join(" ")
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_parsing_matches_reference() {
        assert_eq!(parse_summary(""), "");
        assert_eq!(parse_summary("  plain text  "), "plain text");
        assert_eq!(parse_summary("```json\n{\"summary\": \" s \"}\n```"), "s");
        assert_eq!(parse_summary("{\"summary\": [\"a \", \"\", 3]}"), "a 3");
        assert_eq!(parse_summary("{\"summary\": null}"), "");
        assert_eq!(parse_summary("{\"summary\": 0}"), "");
        assert_eq!(parse_summary("{\"summary\": {\"k\": 1}}"), "{'k': 1}");
        assert_eq!(parse_summary("{\"other\": 1}"), "{\"other\": 1}");
        assert_eq!(parse_summary("{\"summary\": \"x\",}"), "x");
        assert_eq!(parse_summary("{\"summary\": \"a\nb\"}"), "a b");
        assert_eq!(parse_title("{\"title\": \"  A \\n B \"}"), "A B");
        assert_eq!(parse_title("{\"title\": [\"x\", \"y\"]}"), "x y");
        assert_eq!(parse_title("no json"), "");
        assert_eq!(parse_title("{\"title\": \"\"}"), "");
    }
}
