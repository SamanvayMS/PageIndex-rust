//! Post-processing `page_index_flash` applies to an `extract_toc` result before the model passes.
//! ref: pageindex/flash/api.py:82-142, 190-231

use pi_pycompat::pystr;
use serde_json::{Map, Value};

use crate::consts::FLAT_TREE_MAX_NODES;
use crate::optimize::optimize_merge_only;
use crate::tree::{Tree, TreeError, write_node_id};

/// `_page_nodes(page_texts)`: one node per page, so every page is reachable; empty when no page
/// has text. ref: api.py:126
pub fn page_nodes(page_texts: &[String]) -> Vec<Value> {
    if !page_texts.iter().any(|t| !pystr::strip(t).is_empty()) {
        return Vec::new();
    }
    let mut nodes: Vec<Value> = (1..=page_texts.len())
        .map(|i| {
            let mut m = Map::new();
            m.insert("title".into(), Value::String(format!("Page {i}")));
            m.insert("node_id".into(), Value::String(String::new()));
            m.insert("start_index".into(), i.into());
            m.insert("end_index".into(), i.into());
            Value::Object(m)
        })
        .collect();
    let mut v = Value::Array(std::mem::take(&mut nodes));
    write_node_id(&mut v, 0);
    match v {
        Value::Array(a) => a,
        _ => unreachable!(),
    }
}

/// `_add_preface(structure)`: pages before a late-starting hierarchy become a Preface node.
/// ref: api.py:137
pub fn add_preface(structure: &mut Vec<Value>) {
    let first_start = structure[0]
        .get("start_index")
        .and_then(Value::as_i64)
        .unwrap_or(1);
    let mut m = Map::new();
    m.insert("title".into(), Value::String("Preface".into()));
    m.insert("start_index".into(), 1.into());
    m.insert("end_index".into(), (first_start - 1).into());
    structure.insert(0, Value::Object(m));
    let mut v = Value::Array(std::mem::take(structure));
    write_node_id(&mut v, 0);
    if let Value::Array(a) = v {
        *structure = a;
    }
}

/// `[line.strip() for line in (page_text or "").splitlines() if line.strip()]`
/// (api.py:92): the per-page lines expand's `heading_at_page_start` reads.
pub fn page_lines(page_text: &str) -> Vec<String> {
    splitlines(page_text)
        .into_iter()
        .map(pystr::strip)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Python `str.splitlines()` (no keepends).
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let is_break = matches!(
            c,
            '\n' | '\r'
                | '\u{0B}'
                | '\u{0C}'
                | '\u{1C}'
                | '\u{1D}'
                | '\u{1E}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if is_break {
            out.push(&s[start..i]);
            let mut next = i + c.len_utf8();
            if c == '\r'
                && let Some(&(j, '\n')) = it.peek()
            {
                it.next();
                next = j + 1;
            }
            start = next;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// The part of `page_index_flash` between `extract_toc` and the model passes, on a result dict
/// (`doc_name`, `doc_title`, `structure`, ..., `page_texts`, `toc_source`).
pub struct Prepared {
    /// The result with `page_texts` popped.
    pub result: Map<String, Value>,
    pub pages: Vec<String>,
    /// A flat page tree too large to index: returned as is, no model passes.
    pub refused: bool,
}

/// Fallbacks (`_page_nodes` / `_add_preface`), then pop `page_texts`, then the flat-tree
/// refusal. ref: api.py:191-209
pub fn prepare(mut result: Map<String, Value>) -> Prepared {
    let structure_empty = match result.get("structure") {
        Some(Value::Array(a)) => a.is_empty(),
        _ => true,
    };
    if structure_empty {
        let texts = page_texts_of(&result);
        let structure = page_nodes(&texts);
        let source = if structure.is_empty() {
            "unreadable"
        } else {
            "pages"
        };
        result.insert("structure".into(), Value::Array(structure));
        result.insert("toc_source".into(), Value::String(source.into()));
    } else if let Some(Value::Array(structure)) = result.get_mut("structure")
        && structure[0]
            .get("start_index")
            .and_then(Value::as_i64)
            .unwrap_or(1)
            > 1
    {
        add_preface(structure);
    }
    let pages = page_texts_of(&result);
    result.shift_remove("page_texts");
    let n = result
        .get("structure")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let refused = result.get("toc_source").and_then(Value::as_str) == Some("pages")
        && n > FLAT_TREE_MAX_NODES;
    Prepared {
        result,
        pages,
        refused,
    }
}

/// `result.get("page_texts") or []` (a `None` page becomes `""`).
fn page_texts_of(result: &Map<String, Value>) -> Vec<String> {
    match result.get("page_texts") {
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| v.as_str().unwrap_or("").to_string())
            .collect(),
        _ => Vec::new(),
    }
}

/// `page_index_flash(summary=False, optimize="merge")` post-processing as
/// `parity/dump_reference.py::optimized` runs it: fallbacks, then the deterministic merge with
/// its before/after report. Like the dump (and unlike `page_index_flash`, which strips it on its
/// way out) the `_same_page` bookkeeping key is left on the nodes; call
/// [`crate::tree::strip_internal_keys_value`] for the public shape.
pub fn postprocess_merge(result: Map<String, Value>) -> Result<Map<String, Value>, TreeError> {
    let Prepared {
        mut result,
        pages,
        refused,
    } = prepare(result);
    if refused {
        return Ok(result);
    }
    let structure = result
        .get("structure")
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    if structure.as_array().is_some_and(|a| !a.is_empty()) {
        let mut tree = Tree::from_value(&structure)?;
        let outcome = optimize_merge_only(&mut tree, Some(pages.len() as i64));
        result.insert("structure".into(), tree.to_value());
        result.insert("optimize".into(), Value::Object(outcome.report()));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn splitlines_matches_python() {
        assert_eq!(
            splitlines("a\nb\r\nc\rd\u{2028}e\x0c"),
            vec!["a", "b", "c", "d", "e"]
        );
        assert_eq!(splitlines(""), Vec::<&str>::new());
        assert_eq!(splitlines("\n\n"), vec!["", ""]);
        assert_eq!(splitlines("x\u{85}y"), vec!["x", "y"]);
        assert_eq!(
            page_lines("  Title \n\n body\u{1f}\n"),
            vec!["Title", "body"]
        );
    }

    #[test]
    fn page_nodes_and_preface() {
        assert!(page_nodes(&["  ".into(), "\n".into()]).is_empty());
        let nodes = page_nodes(&["a".into(), "".into()]);
        assert_eq!(
            Value::Array(nodes),
            json!([
                {"title": "Page 1", "node_id": "0000", "start_index": 1, "end_index": 1},
                {"title": "Page 2", "node_id": "0001", "start_index": 2, "end_index": 2}
            ])
        );
        let mut s = vec![
            json!({"title": "A", "node_id": "0000", "start_index": 3, "end_index": 5,
                                "nodes": [{"title": "B", "node_id": "0001", "start_index": 4, "end_index": 5}]}),
        ];
        add_preface(&mut s);
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            r#"[{"title":"Preface","start_index":1,"end_index":2,"node_id":"0000"},{"title":"A","node_id":"0001","start_index":3,"end_index":5,"nodes":[{"title":"B","node_id":"0002","start_index":4,"end_index":5}]}]"#
        );
    }

    #[test]
    fn prepare_refuses_large_flat_trees() {
        let texts: Vec<Value> = (0..11).map(|i| Value::String(format!("p{i}"))).collect();
        let mut r = Map::new();
        r.insert("doc_name".into(), "x.pdf".into());
        r.insert("structure".into(), json!([]));
        r.insert("page_texts".into(), Value::Array(texts));
        r.insert("toc_source".into(), "detected".into());
        let out = postprocess_merge(r).unwrap();
        assert_eq!(out["toc_source"], "pages");
        assert!(!out.contains_key("page_texts") && !out.contains_key("optimize"));
        assert_eq!(out["structure"].as_array().unwrap().len(), 11);

        let mut r = Map::new();
        r.insert("structure".into(), json!([]));
        r.insert("page_texts".into(), json!(["", " "]));
        r.insert("toc_source".into(), "detected".into());
        let out = postprocess_merge(r).unwrap();
        assert_eq!(out["toc_source"], "unreadable");
        assert!(!out.contains_key("optimize"));

        let mut r = Map::new();
        r.insert("structure".into(), json!([]));
        r.insert("page_texts".into(), json!(["a", "b"]));
        r.insert("toc_source".into(), "detected".into());
        let out = postprocess_merge(r).unwrap();
        assert_eq!(out["toc_source"], "pages");
        assert_eq!(out["optimize"]["merges"], 0);
        assert_eq!(out["optimize"]["before"]["total_pages"], 2);
    }
}
