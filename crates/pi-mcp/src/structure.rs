//! Structure formatting and splitting for `get_document_structure`.

use serde_json::{Map, Value};

use crate::consts::STRUCTURE_KEY_ORDER;
use crate::pyval::truthy;
use pi_store::pyjson::serialized_len;

/// `_format_structure`: drop node `text` and normalise key order, recursively.
/// // ref: pageindex/agent_tools.py:653
pub fn format_structure(node: &Value) -> Value {
    match node {
        Value::Array(items) => Value::Array(items.iter().map(format_structure).collect()),
        Value::Object(map) => {
            let mut stripped: Map<String, Value> = map
                .iter()
                .filter(|(k, _)| k.as_str() != "text")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            if let Some(nodes) = stripped.get_mut("nodes") {
                *nodes = format_structure(nodes);
            }
            let mut ordered = Map::new();
            for key in STRUCTURE_KEY_ORDER {
                if let Some(v) = stripped.get(key) {
                    ordered.insert(key.to_string(), v.clone());
                }
            }
            for (k, v) in stripped {
                if !ordered.contains_key(&k) {
                    ordered.insert(k, v);
                }
            }
            Value::Object(ordered)
        }
        other => other.clone(),
    }
}

/// `_split_structure`: chunks of at most ~`budget` serialized characters. An unsplit
/// structure keeps its shape; once split, every chunk is a list of nodes.
/// // ref: pageindex/agent_tools.py:673
pub fn split_structure(structure: &Value, budget: usize) -> Vec<Value> {
    if serialized_len(structure) <= budget {
        return vec![structure.clone()];
    }
    let single;
    let nodes: &[Value] = match structure {
        Value::Array(items) => items,
        other => {
            single = [other.clone()];
            &single
        }
    };
    let mut chunks: Vec<Value> = Vec::new();
    let mut group: Vec<Value> = Vec::new();
    let mut group_size = 0usize;
    for node in nodes {
        let size = serialized_len(node);
        if size > budget {
            if !group.is_empty() {
                chunks.push(Value::Array(std::mem::take(&mut group)));
                group_size = 0;
            }
            chunks.extend(
                split_oversized_node(node, budget)
                    .into_iter()
                    .map(|part| Value::Array(vec![part])),
            );
            continue;
        }
        if !group.is_empty() && group_size + size > budget {
            chunks.push(Value::Array(std::mem::take(&mut group)));
            group_size = 0;
        }
        group.push(node.clone());
        group_size += size;
    }
    if !group.is_empty() {
        chunks.push(Value::Array(group));
    }
    if chunks.is_empty() {
        vec![structure.clone()]
    } else {
        chunks
    }
}

/// `_split_oversized_node`: split a node's children, repeating its other fields in every
/// part. // ref: pageindex/agent_tools.py:705
pub fn split_oversized_node(node: &Value, budget: usize) -> Vec<Value> {
    let Value::Object(map) = node else {
        return vec![node.clone()];
    };
    let Some(children) = map.get("nodes").filter(|c| truthy(c)) else {
        return vec![node.clone()];
    };
    let shell: Map<String, Value> = map
        .iter()
        .filter(|(k, _)| k.as_str() != "nodes")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let shell_size = serialized_len(&Value::Object(shell.clone()));
    let child_budget = (budget as i64 - shell_size as i64).max((budget / 2) as i64) as usize;
    split_structure(children, child_budget)
        .into_iter()
        .map(|chunk| {
            let mut part = shell.clone();
            let nodes = match chunk {
                Value::Array(_) => chunk,
                other => Value::Array(vec![other]),
            };
            part.insert("nodes".into(), nodes);
            Value::Object(part)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chunks_never_change_type() {
        // ref: tests/test_agent_tools.py:319
        let small = json!({"title": "s", "node_id": "0001"});
        let children: Vec<Value> = (0..10)
            .map(|i| json!({"title": format!("c{i}"), "summary": "x".repeat(40)}))
            .collect();
        let big = json!({"title": "b", "node_id": "0002", "nodes": children});
        let chunks = split_structure(&json!([small, small, big]), 200);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(Value::is_array));
        assert_eq!(split_structure(&small, 10_000), vec![small.clone()]);
        assert_eq!(
            split_structure(&json!([small]), 10_000),
            vec![json!([small])]
        );
    }

    #[test]
    fn format_orders_keys_and_strips_text() {
        let node = json!({"summary": "s", "text": "T", "zz": 1, "end_index": 2, "title": "t",
                          "nodes": [{"text": "x", "node_id": "1", "title": "c"}]});
        let out = format_structure(&node);
        assert_eq!(
            pi_store::pyjson::dumps(&out),
            r#"{"title": "t", "end_index": 2, "summary": "s", "nodes": [{"title": "c", "node_id": "1"}], "zz": 1}"#
        );
    }
}
