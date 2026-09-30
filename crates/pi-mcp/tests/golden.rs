//! Golden tests: Rust `call_tool` output must equal the Python reference's
//! `agent_tools.call_tool(client, name, args)` output byte for byte, on the same store.
//! Fixtures come from `gen/gen_fixtures.py`.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use pi_mcp::consts::tool_contract;
use pi_mcp::{AGENT_INSTRUCTIONS, call_tool, local_description, local_schema, tool_names};
use pi_store::pyjson::dumps;
use pi_store::{DocStore, LocalApi};

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

// ── recipe expansion (mirrors gen/gen_fixtures.py) ──

fn add_summaries(nodes: &mut Value, repeat: usize) {
    if let Value::Array(items) = nodes {
        for node in items {
            let title = node["title"].as_str().unwrap().to_string();
            let map = node.as_object_mut().unwrap();
            map.insert("summary".into(), json!(format!("{title}. ").repeat(repeat)));
            if let Some(children) = map.get_mut("nodes") {
                add_summaries(children, repeat);
            }
        }
    }
}

fn expand_tree(spec: &Value) -> Value {
    if let Some(p) = spec.get("$flat") {
        let count = p["count"].as_u64().unwrap();
        let len = p["summary_len"].as_u64().unwrap() as usize;
        return Value::Array(
            (0..count)
                .map(|i| {
                    json!({
                        "title": format!("Chapter {i}"), "node_id": format!("{i:04}"),
                        "start_index": i + 1, "end_index": i + 1,
                        "summary": "s".repeat(len), "text": "T",
                    })
                })
                .collect(),
        );
    }
    if let Some(p) = spec.get("$summaries") {
        let mut tree = p["tree"].clone();
        add_summaries(&mut tree, p["repeat"].as_u64().unwrap() as usize);
        return tree;
    }
    spec.clone()
}

fn expand_pages(spec: &Value) -> Value {
    let texts: Vec<String> = spec
        .as_array()
        .unwrap()
        .iter()
        .map(|item| match item {
            Value::String(s) => s.clone(),
            other => other["$repeat"]
                .as_str()
                .unwrap()
                .repeat(other["n"].as_u64().unwrap() as usize),
        })
        .collect();
    pi_store::api::pages_record(&texts)
}

fn build_store(path: &Path, docs: &Value) {
    let store = DocStore::new(path);
    for doc in docs.as_array().unwrap() {
        let meta: Map<String, Value> = doc["meta"].as_object().unwrap().clone();
        let id = meta["id"].as_str().unwrap().to_string();
        let pages = match doc.get("pages_raw") {
            Some(raw) => raw.clone(),
            None => expand_pages(&doc["pages"]),
        };
        store
            .save_document(&id, &meta, &expand_tree(&doc["tree"]), &pages)
            .unwrap();
        if doc.get("missing_tree") == Some(&Value::Bool(true)) {
            std::fs::remove_file(path.join("docs").join(&id).join("tree.json")).unwrap();
        }
    }
}

fn copy_dir(from: &Path, to: &Path) {
    if !from.exists() {
        return;
    }
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn first_diff(a: &str, b: &str) -> String {
    let pos = a
        .chars()
        .zip(b.chars())
        .position(|(x, y)| x != y)
        .unwrap_or(a.chars().count().min(b.chars().count()));
    let ctx = |s: &str| {
        s.chars()
            .skip(pos.saturating_sub(80))
            .take(200)
            .collect::<String>()
    };
    format!("at char {pos}:\n  rust: {}\n  py:   {}", ctx(a), ctx(b))
}

#[test]
fn tool_outputs_match_python() {
    let stores = fixture("stores.json");
    let cases = fixture("cases.json");
    let tmp = tempfile::tempdir().unwrap();
    let mut failures = Vec::new();
    let cases = cases.as_array().unwrap();
    // One template per store; read-only tools share it, remove_document gets a copy.
    let mut templates = std::collections::HashMap::new();
    for (name, docs) in stores.as_object().unwrap() {
        let path = tmp.path().join(format!("template-{name}"));
        build_store(&path, docs);
        templates.insert(name.clone(), path);
    }
    for (i, case) in cases.iter().enumerate() {
        let template = &templates[case["store"].as_str().unwrap()];
        let tool = case["tool"].as_str().unwrap();
        let path: PathBuf = if tool == "remove_document" {
            let path = tmp.path().join(format!("s{i}"));
            copy_dir(template, &path);
            path
        } else {
            template.clone()
        };
        let api = LocalApi::new(&path);
        let doc_ids: Option<Vec<String>> = case["doc_ids"].as_array().map(|ids| {
            ids.iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect()
        });
        let (text, is_error) = call_tool(&api, tool, Some(&case["args"]), doc_ids.as_deref());
        let label = format!("#{i} {tool} {}", dumps(&case["args"]));
        if is_error != case["is_error"].as_bool().unwrap() {
            failures.push(format!("{label}: is_error {is_error}"));
        }
        if let Some(want) = case.get("text").and_then(Value::as_str) {
            if text != want {
                failures.push(format!("{label}: {}", first_diff(&text, want)));
            }
        } else {
            let digest: String = Sha256::digest(text.as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let head = case["head"].as_str().unwrap();
            if !text.starts_with(head) {
                let got: String = text.chars().take(head.chars().count()).collect();
                failures.push(format!("{label}: head {}", first_diff(&got, head)));
            } else if digest != case["sha256"].as_str().unwrap()
                || text.chars().count() as u64 != case["len"].as_u64().unwrap()
            {
                failures.push(format!(
                    "{label}: digest/len mismatch (len {} vs {})",
                    text.chars().count(),
                    case["len"]
                ));
            }
        }
        if let Some(remaining) = case.get("remaining") {
            let listing = api.list_documents(10_000, 0, None).unwrap();
            let names: Vec<Value> = listing["documents"]
                .as_array()
                .unwrap()
                .iter()
                .map(|d| d["name"].clone())
                .collect();
            if &Value::Array(names.clone()) != remaining {
                failures.push(format!("{label}: remaining {names:?}"));
            }
        }
        if tool == "remove_document" {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

#[test]
fn surface_matches_python() {
    let surface = fixture("surface.json");
    assert_eq!(dumps(&tool_contract()), dumps(&surface["tool_contract"]));
    assert_eq!(json!(tool_names(false)), surface["tool_names"]);
    assert_eq!(json!(tool_names(true)), surface["tool_names_management"]);
    for name in tool_names(true) {
        assert_eq!(
            json!(local_description(name).unwrap()),
            surface["local_descriptions"][name],
            "{name}"
        );
        assert_eq!(
            dumps(&local_schema(name).unwrap()),
            dumps(&surface["local_schemas"][name]),
            "{name}"
        );
    }
    assert_eq!(json!(*AGENT_INSTRUCTIONS), surface["agent_instructions"]);
    assert_eq!(json!(pi_mcp::consts::CHAT_HEADER), surface["chat_header"]);
    assert_eq!(
        json!(pi_mcp::consts::CITATION_PROMPT_MARKDOWN),
        surface["citation_prompts"]["markdown"]
    );
    assert_eq!(
        json!(pi_mcp::consts::CITATION_PROMPT_CITE),
        surface["citation_prompts"]["cite"]
    );
    let c = &surface["constants"];
    assert_eq!(
        c["TOOL_RESPONSE_CHAR_LIMIT"],
        json!(pi_mcp::consts::TOOL_RESPONSE_CHAR_LIMIT)
    );
    assert_eq!(c["_CHAR_BUDGET"], json!(pi_mcp::consts::CHAR_BUDGET));
    assert_eq!(
        c["_MAX_REQUESTED_PAGES"],
        json!(pi_mcp::consts::MAX_REQUESTED_PAGES as u64)
    );
    assert_eq!(
        c["STRUCTURE_FIRST_PAGE_THRESHOLD"],
        json!(pi_mcp::consts::STRUCTURE_FIRST_PAGE_THRESHOLD)
    );
}

/// The frozen cloud contract shipped with the reference's tests (read-only reference
/// checkout; skipped when absent).
#[test]
fn contract_matches_cloud_snapshot() {
    let path = std::env::var("PI_REF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/home/user/PageIndex-rust/parity/.ref"))
        .join("tests/data/cloud_mcp_contract.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!("skipped: {} not found", path.display());
        return;
    };
    let snapshot: Value = serde_json::from_str(&text).unwrap();
    let contract = tool_contract();
    let tools = snapshot["tools"].as_object().unwrap();
    assert_eq!(
        tools.keys().collect::<Vec<_>>(),
        contract.as_object().unwrap().keys().collect::<Vec<_>>()
    );
    for (name, spec) in tools {
        assert_eq!(dumps(&contract[name]), dumps(spec), "{name}");
    }
}
