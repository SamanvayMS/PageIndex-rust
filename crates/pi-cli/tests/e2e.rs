//! `pageindex-rs index` end to end: the store it writes is read by the Python reference SDK
//! and served by the pi-mcp tools with the pipeline's own page text.
//!
//! Skipped (passes with a note) without PDFium (`PDFIUM_LIB`), the reference checkout
//! (`PI_REF_DIR`) or, for the Python half, the parity venv (`PI_PARITY_PYTHON`).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

fn ref_dir() -> PathBuf {
    std::env::var_os("PI_REF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/user/PageIndex-rust/parity/.ref"))
}

fn python() -> Option<PathBuf> {
    let path = std::env::var_os("PI_PARITY_PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/user/PageIndex-rust/parity/.venv/bin/python"));
    Command::new(&path)
        .args(["-c", "import pageindex.local_store"])
        .output()
        .is_ok_and(|o| o.status.success())
        .then_some(path)
}

fn index(storage: &Path, report: &Path, trees: &Path, pdf: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_pageindex-rs"))
        .args([
            "index",
            "--no-summary",
            "--optimize",
            "merge",
            "--accept-flat",
        ])
        .arg("--report")
        .arg(report)
        .arg("--tree-out")
        .arg(trees)
        .arg("--storage")
        .arg(storage)
        .arg(pdf)
        .output()
        .unwrap()
}

#[test]
fn index_then_read_from_python_and_mcp() {
    if pi_extract::pdfium::bindings().is_err() {
        eprintln!("skipped: PDFium not available (set PDFIUM_LIB)");
        return;
    }
    let pdf = ref_dir().join("examples/documents/earthmover.pdf");
    if !pdf.is_file() {
        eprintln!("skipped: {} not found", pdf.display());
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let storage = tmp.path().join(".pageindex");
    let report = tmp.path().join("report.jsonl");
    let trees = tmp.path().join("trees");
    let out = index(&storage, &report, &trees, &pdf);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let line: Value =
        serde_json::from_str(std::fs::read_to_string(&report).unwrap().trim()).unwrap();
    assert_eq!(line["doc"], "earthmover.pdf");
    assert_eq!(line["pages"], 12);
    for k in [
        "extract_s",
        "layout_s",
        "structure_s",
        "post_s",
        "total_s",
        "nodes",
    ] {
        assert!(line[k].is_number(), "{k}");
    }
    let doc_id = line["doc_id"].as_str().expect("committed").to_string();
    let tree: Value =
        serde_json::from_str(&std::fs::read_to_string(trees.join("earthmover.json")).unwrap())
            .unwrap();
    assert_eq!(tree["toc_source"], line["toc_source"]);

    // pages.json holds the pipeline's own text.
    let pages: Value = serde_json::from_str(
        &std::fs::read_to_string(storage.join("docs").join(&doc_id).join("pages.json")).unwrap(),
    )
    .unwrap();
    let page1 = pages[0]["markdown"].as_str().unwrap().to_string();
    assert!(!page1.is_empty());

    // pi-mcp serves that text.
    let api = pi_store::LocalApi::new(&storage);
    let (text, is_error) = pi_mcp::call_tool(
        &api,
        "get_page_content",
        Some(&json!({"doc_name": "earthmover.pdf", "pages": "1"})),
        None,
    );
    assert!(!is_error, "{text}");
    let payload: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(payload["content"][0]["text"], page1);
    let (text, is_error) = pi_mcp::call_tool(
        &api,
        "get_document_structure",
        Some(&json!({"doc_name": "earthmover.pdf"})),
        None,
    );
    assert!(!is_error, "{text}");

    // The Python reference SDK lists it and reads structure and page content.
    let Some(py) = python() else {
        eprintln!("skipped Python half: no reference venv");
        return;
    };
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../pi-store/tests/py/interop.py");
    let out = Command::new(py)
        .arg(script)
        .arg("read")
        .arg(&storage)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let seen: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(seen["list_documents"]["total"], 1);
    assert_eq!(
        seen["list_documents"]["documents"][0]["name"],
        "earthmover.pdf"
    );
    let doc = &seen["docs"][&doc_id];
    assert_eq!(
        doc["get_document_structure"].as_array().unwrap().len(),
        tree["structure"].as_array().unwrap().len()
    );
    assert_eq!(doc["get_page_content"][0]["markdown"], page1);
    assert_eq!(doc["get_document"]["pageNum"], 12);
}
