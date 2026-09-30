//! Interop with the Python SDK's local mode (reference @619cbd8): a store written by Rust
//! reads identically in Python and vice versa, files are byte-identical, and the `.lock`
//! excludes Python writers.
//!
//! Needs the parity venv (`PI_PARITY_PYTHON`, default
//! `/home/user/PageIndex-rust/parity/.venv/bin/python`) with `pageindex` importable; each
//! test is skipped (passes with a note) when it is absent.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Map, Value, json};

use pi_store::api::{doc_meta, pages_record, remove_fields};
use pi_store::pyjson::dumps;
use pi_store::{DocStore, LocalApi, NewDocument};

fn python() -> Option<PathBuf> {
    let path = std::env::var_os("PI_PARITY_PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/user/PageIndex-rust/parity/.venv/bin/python"));
    let ok = Command::new(&path)
        .args(["-c", "import pageindex.local_store"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("skipped: no Python reference venv at {}", path.display());
    }
    ok.then_some(path)
}

fn run_py(py: &Path, args: &[&str]) -> Value {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/py/interop.py");
    let out = Command::new(py).arg(script).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "python failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn sample() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The Rust side of `interop.py read`: same calls, same key order.
fn rust_read(store: &Path) -> Value {
    let api = LocalApi::new(store);
    let listing = api.list_documents(10_000, 0, None).unwrap();
    let mut docs = Map::new();
    for doc in listing["documents"].as_array().unwrap() {
        let id = doc["id"].as_str().unwrap();
        let mut structure = api.get_tree(id, true, false).unwrap()["result"].clone();
        structure = remove_fields(&structure, &["text"]);
        let wanted = [1, 2, 3, 5];
        let pages: Vec<Value> = api.get_ocr(id, "page").unwrap()["result"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| wanted.contains(&p["page_index"].as_i64().unwrap()))
            .cloned()
            .collect();
        docs.insert(
            id.to_string(),
            json!({
                "get_document": api.get_document(id).unwrap(),
                "get_document_structure": structure,
                "get_tree": api.get_tree(id, true, true).unwrap(),
                "get_page_content": pages,
                "get_ocr_raw": api.get_ocr(id, "raw").unwrap(),
                "get_ocr_node": api.get_ocr(id, "node").unwrap(),
                "raw_tree": api.raw_tree(id).unwrap(),
                "id_by_name": api.get_document_id(doc["name"].as_str().unwrap()).unwrap(),
            }),
        );
    }
    json!({"list_documents": listing, "docs": docs})
}

fn commit_sample(api: &LocalApi) -> Vec<Value> {
    sample()["docs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|doc| {
            let pages: Vec<String> = doc["pages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_str().unwrap().to_string())
                .collect();
            api.commit_document(NewDocument {
                name: doc["name"].as_str().unwrap(),
                description: doc["description"].as_str(),
                structure: &doc["tree"],
                page_texts: &pages,
                metadata: doc["metadata"].as_object().cloned(),
                mode: "flash",
            })
            .unwrap()
        })
        .collect()
}

#[test]
fn rust_written_store_reads_identically_in_python() {
    let Some(py) = python() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join(".pageindex");
    let committed = commit_sample(&LocalApi::new(&store));
    let names: Vec<&str> = committed
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["3M_2018_10K.pdf", "AMCOR_2022_8K.pdf", "3M_2018_10K_1.pdf"]
    );
    // Stored trees carry no node text.
    let raw = std::fs::read_to_string(
        store
            .join("docs")
            .join(committed[0]["doc_id"].as_str().unwrap())
            .join("tree.json"),
    )
    .unwrap();
    assert!(!raw.contains("\"text\""));

    let from_python = run_py(&py, &["read", store.to_str().unwrap()]);
    assert_eq!(dumps(&rust_read(&store)), dumps(&from_python));
}

#[test]
fn python_written_store_reads_identically_in_rust() {
    let Some(py) = python() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join(".pageindex");
    let sample_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.json");
    let written = run_py(
        &py,
        &[
            "write",
            store.to_str().unwrap(),
            sample_path.to_str().unwrap(),
        ],
    );
    assert_eq!(written[2]["name"], "3M_2018_10K_1.pdf");
    let from_python = run_py(&py, &["read", store.to_str().unwrap()]);
    assert_eq!(dumps(&rust_read(&store)), dumps(&from_python));

    // Rust can add to a Python-written store and delete from it.
    let api = LocalApi::new(&store);
    let texts = vec!["only page".to_string()];
    let added = api
        .commit_document(NewDocument {
            name: "3M_2018_10K.pdf",
            description: None,
            structure: &json!([{"title": "T", "node_id": "0000", "start_index": 1, "end_index": 1}]),
            page_texts: &texts,
            metadata: None,
            mode: "flash",
        })
        .unwrap();
    assert_eq!(added["name"], "3M_2018_10K_2.pdf");
    let id = written[1]["doc_id"].as_str().unwrap();
    api.delete_document(id).unwrap();
    let from_python = run_py(&py, &["read", store.to_str().unwrap()]);
    assert_eq!(dumps(&rust_read(&store)), dumps(&from_python));
    assert_eq!(from_python["list_documents"]["total"], 3);
}

#[test]
fn files_are_byte_identical() {
    let Some(py) = python() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let doc = &sample()["docs"][0];
    let texts: Vec<String> = doc["pages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_string())
        .collect();
    let meta = doc_meta(
        "pi-0123456789abcdef0123456789abcdef",
        doc["name"].as_str().unwrap(),
        doc["description"].as_str(),
        "2026-08-01T10:00:00.123000",
        texts.len(),
        doc["metadata"].as_object().cloned(),
        "flash",
    );
    let tree = remove_fields(&doc["tree"], &["text"]);
    let pages = pages_record(&texts);
    let rust_store = tmp.path().join("rust");
    DocStore::new(&rust_store)
        .save_document(meta["id"].as_str().unwrap(), &meta, &tree, &pages)
        .unwrap();
    let record = tmp.path().join("record.json");
    std::fs::write(
        &record,
        dumps(&json!({"meta": meta, "tree": tree, "pages": pages})),
    )
    .unwrap();
    let py_store = tmp.path().join("python");
    run_py(
        &py,
        &["save", py_store.to_str().unwrap(), record.to_str().unwrap()],
    );
    let doc_dir = Path::new("docs").join(meta["id"].as_str().unwrap());
    for rel in [
        PathBuf::from("manifest.json"),
        doc_dir.join("tree.json"),
        doc_dir.join("pages.json"),
        doc_dir.join("doc.json"),
    ] {
        let a = std::fs::read(rust_store.join(&rel)).unwrap();
        let b = std::fs::read(py_store.join(&rel)).unwrap();
        assert!(a == b, "{} differs", rel.display());
    }
}

#[test]
fn lock_excludes_python() {
    let Some(py) = python() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let store = DocStore::new(tmp.path());
    let guard = store.lock().unwrap();
    assert_eq!(
        run_py(&py, &["trylock", tmp.path().to_str().unwrap()]),
        json!("locked")
    );
    drop(guard);
    assert_eq!(
        run_py(&py, &["trylock", tmp.path().to_str().unwrap()]),
        json!("free")
    );
}
