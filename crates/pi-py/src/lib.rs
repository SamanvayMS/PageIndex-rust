//! `pageindex_rs`: Python bindings for the Rust indexer and retrieval tools.
//!
//! ```python
//! import pageindex_rs
//! out = pageindex_rs.index("report.pdf", storage=".pageindex")   # {"doc_id", "name", ...}
//! pageindex_rs.call_tool(".pageindex", "get_document_structure", '{"doc_name": "report.pdf"}')
//! ```
//!
//! Heavy work runs with the GIL released. Results cross as JSON (`json.loads`), so dicts keep
//! the Rust key order.

use std::path::{Path, PathBuf};

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use serde_json::{Value, json};

fn err(e: impl std::fmt::Display) -> PyErr {
    PyRuntimeError::new_err(e.to_string())
}

fn to_py<'py>(py: Python<'py>, value: &Value) -> PyResult<Bound<'py, PyAny>> {
    py.import("json")?
        .call_method1("loads", (pi_store::pyjson::dumps(value),))
}

fn runtime() -> PyResult<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(err)
}

fn index_blocking(
    path: PathBuf,
    storage: PathBuf,
    summary: bool,
    optimize: pi_config::Optimize,
    ocr: pi_index::OcrMode,
    config: Option<PathBuf>,
    accept_flat: bool,
) -> anyhow::Result<Value> {
    let cfg = pi_config::Config::load(config.as_deref()).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut opts = pi_index::IndexOptions::from_config(&cfg);
    opts.summary = summary;
    opts.optimize = optimize;
    opts.ocr = ocr;
    let roles = pi_index::LlmRoles::from_config(&cfg.llm)?;
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let doc_name = pi_store::naming::sanitize_filename(&file_name);
    if !doc_name.to_lowercase().ends_with(".pdf") {
        anyhow::bail!("only PDF files are supported: {}", path.display());
    }
    let bytes = std::fs::read(&path)?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let doc = rt.block_on(pi_index::index_document(
        bytes,
        &doc_name,
        &opts,
        Some(roles),
    ))?;
    let api = pi_store::LocalApi::new(&storage);
    let committed = doc.commit(&api, &doc_name, accept_flat)?;
    let t = &doc.timings;
    Ok(json!({
        "doc_id": committed.as_ref().map(|c| c.0.clone()),
        "name": committed.as_ref().map(|c| c.1.clone()),
        "rejected": doc.rejection,
        "description": doc.description,
        "result": Value::Object(doc.result.clone()),
        "timings": {
            "extract_s": t.extract_s, "ocr_s": t.ocr_s, "layout_s": t.layout_s,
            "structure_s": t.structure_s, "post_s": t.post_s,
            "description_s": t.description_s, "total_s": t.total_s,
        },
    }))
}

/// Index one PDF into `storage` (Python-SDK-compatible). Returns `{doc_id, name, rejected,
/// description, result, timings}`; `doc_id` is None when the result was refused.
#[pyfunction]
#[pyo3(signature = (path, storage=PathBuf::from(".pageindex"), summary=false, optimize="merge", ocr="off", config=None, accept_flat=false))]
#[allow(clippy::too_many_arguments)]
fn index<'py>(
    py: Python<'py>,
    path: PathBuf,
    storage: PathBuf,
    summary: bool,
    optimize: &str,
    ocr: &str,
    config: Option<PathBuf>,
    accept_flat: bool,
) -> PyResult<Bound<'py, PyAny>> {
    let optimize = match optimize {
        "full" => pi_config::Optimize::Full,
        "merge" => pi_config::Optimize::Merge,
        "off" => pi_config::Optimize::Off,
        o => {
            return Err(PyValueError::new_err(format!(
                "optimize must be full, merge or off, got {o:?}"
            )));
        }
    };
    let ocr = match ocr {
        "auto" => pi_index::OcrMode::Auto,
        "off" => pi_index::OcrMode::Off,
        o => {
            return Err(PyValueError::new_err(format!(
                "ocr must be auto or off, got {o:?}"
            )));
        }
    };
    let out = py
        .detach(|| index_blocking(path, storage, summary, optimize, ocr, config, accept_flat))
        .map_err(|e| err(format!("{e:#}")))?;
    to_py(py, &out)
}

/// Stage-01 spans: one dict per page (`page`, `viewbox`, `rotation`, `spans`).
#[pyfunction]
fn extract_spans<'py>(py: Python<'py>, path: PathBuf) -> PyResult<Bound<'py, PyAny>> {
    let pages = py
        .detach(|| pi_extract::extract_pdf(&path))
        .map_err(|e| err(format!("{e:#}")))?;
    to_py(py, &serde_json::to_value(pages).map_err(err)?)
}

/// Page triage: one dict per page (`label`, `signals`).
#[pyfunction]
fn triage<'py>(py: Python<'py>, path: PathBuf) -> PyResult<Bound<'py, PyAny>> {
    let pages = py
        .detach(|| pi_triage::triage_pdf(&path, &pi_triage::Thresholds::default()))
        .map_err(|e| err(format!("{e:#}")))?;
    to_py(py, &serde_json::to_value(pages).map_err(err)?)
}

/// Run one retrieval tool (`browse_documents`, `get_document`, `get_document_structure`,
/// `get_page_content`, `remove_document`) on a store; returns the JSON envelope text, as
/// the reference's `agent_tools.call_tool` does.
#[pyfunction]
fn call_tool(py: Python<'_>, storage: PathBuf, name: &str, args_json: &str) -> PyResult<String> {
    let args: Value = serde_json::from_str(args_json)
        .map_err(|e| PyValueError::new_err(format!("args_json: {e}")))?;
    let api = pi_store::LocalApi::new(&storage);
    Ok(py.detach(|| pi_mcp::call_tool(&api, name, Some(&args), None).0))
}

/// Serve the tools over MCP stdio until the client disconnects (blocks).
#[pyfunction]
#[pyo3(signature = (storage=PathBuf::from(".pageindex"), include_management=false))]
fn serve_mcp(py: Python<'_>, storage: PathBuf, include_management: bool) -> PyResult<()> {
    let server = pi_mcp::PageIndexServer::new(
        pi_store::LocalApi::new(Path::new(&storage)),
        pi_mcp::ServerOptions {
            include_management,
            doc_ids: None,
        },
    );
    let rt = runtime()?;
    py.detach(|| rt.block_on(pi_mcp::serve_stdio(server)))
        .map_err(err)
}

#[pymodule]
fn pageindex_rs(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(index, m)?)?;
    m.add_function(wrap_pyfunction!(extract_spans, m)?)?;
    m.add_function(wrap_pyfunction!(triage, m)?)?;
    m.add_function(wrap_pyfunction!(call_tool, m)?)?;
    m.add_function(wrap_pyfunction!(serve_mcp, m)?)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
