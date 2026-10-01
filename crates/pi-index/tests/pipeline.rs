//! End-to-end pipeline on reference example PDFs. Skipped (passes with a note) when PDFium
//! (`PDFIUM_LIB`) or the reference checkout is unavailable. No real model is called: the
//! LLM roles use `pi_llm::StubLlm`, OCR a local mock HTTP endpoint.

use std::path::PathBuf;
use std::sync::Arc;

use pi_index::{IndexOptions, LlmRoles, OcrMode, Optimize, index_document};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn docs_dir() -> PathBuf {
    std::env::var_os("PI_REF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/user/PageIndex-rust/parity/.ref"))
        .join("examples/documents")
}

fn pdf(name: &str) -> Option<Vec<u8>> {
    if pi_extract::pdfium::bindings().is_err() {
        eprintln!("skipped: PDFium not available (set PDFIUM_LIB)");
        return None;
    }
    match std::fs::read(docs_dir().join(name)) {
        Ok(b) => Some(b),
        Err(_) => {
            eprintln!("skipped: {name} not found");
            None
        }
    }
}

fn llm_free() -> IndexOptions {
    IndexOptions {
        summary: false,
        optimize: Optimize::Merge,
        description: false,
        ..Default::default()
    }
}

#[tokio::test]
async fn earthmover_llm_free_detects_the_reference_tree() {
    let Some(bytes) = pdf("earthmover.pdf") else {
        return;
    };
    let doc = index_document(bytes, "earthmover.pdf", &llm_free(), None)
        .await
        .unwrap();
    assert_eq!(doc.page_texts.len(), 12);
    assert!(doc.page_texts[0].contains("Earth"));
    // Stages 04-08 detect the outline (no bookmarks in this PDF).
    assert_eq!(doc.toc_source(), "detected");
    assert!(doc.rejection.is_none());
    assert_eq!(doc.result["doc_name"], "earthmover.pdf");
    let stored = doc.stored_structure();
    assert_eq!(stored[0]["node_id"], "0000");
    assert_eq!(stored[0]["title"], "ABSTRACT");
    // Identical to the Python page_index_flash(merge) result when goldens are available.
    let golden = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../parity/golden/earthmover/10_tree_optimized.json");
    if let Ok(text) = std::fs::read_to_string(golden) {
        fn strip(v: &serde_json::Value) -> serde_json::Value {
            match v {
                serde_json::Value::Object(m) => serde_json::Value::Object(
                    m.iter()
                        .filter(|(k, _)| *k != "_same_page")
                        .map(|(k, x)| (k.clone(), strip(x)))
                        .collect(),
                ),
                serde_json::Value::Array(a) => {
                    serde_json::Value::Array(a.iter().map(strip).collect())
                }
                other => other.clone(),
            }
        }
        let g: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            strip(&g["data"]["structure"]),
            strip(&doc.result["structure"])
        );
    }
}

#[tokio::test]
async fn bookmarks_drive_the_tree_and_models_run() {
    let Some(bytes) = pdf("attention-residuals.pdf") else {
        return;
    };
    let doc = index_document(bytes.clone(), "attention-residuals.pdf", &llm_free(), None)
        .await
        .unwrap();
    assert_eq!(doc.toc_source(), "bookmarks");
    assert!(doc.rejection.is_none());
    assert!(doc.result.contains_key("optimize"));

    // Summaries + description through a stub model.
    let stub: Arc<dyn pi_llm::Llm> = Arc::new(pi_llm::StubLlm);
    let roles = LlmRoles {
        summary: Some(stub),
        ..Default::default()
    };
    let opts = IndexOptions {
        summary: true,
        optimize: Optimize::Merge,
        description: true,
        ..Default::default()
    };
    let doc = index_document(bytes, "attention-residuals.pdf", &opts, Some(roles))
        .await
        .unwrap();
    assert!(doc.description.is_some());
    let has_summary = doc
        .structure()
        .iter()
        .any(|n| n.get("summary").is_some() || n.get("prefix_summary").is_some());
    assert!(has_summary);
    // summary without a model is a configuration error, as in the reference.
    let err = index_document(
        pdf("attention-residuals.pdf").unwrap(),
        "a.pdf",
        &opts,
        None,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("summary"));
}

/// A one-shot-per-connection HTTP server answering every chat completion with a fixed
/// OCR reply. Returns its base URL.
async fn mock_ocr_server(reply: Value) -> String {
    let content = reply.to_string();
    mock_chat_server(move |_| content.clone()).await
}

/// A one-shot-per-connection HTTP server answering each chat completion with
/// `reply(request)`. Returns its base URL.
async fn mock_chat_server<F>(reply: F) -> String
where
    F: Fn(&Value) -> String + Send + Sync + 'static,
{
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let reply = std::sync::Arc::new(reply);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let reply = reply.clone();
            tokio::spawn(async move {
                // Read headers, then Content-Length bytes of body.
                let mut buf = Vec::new();
                let mut tmp = [0u8; 65536];
                let header_end;
                loop {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        header_end = p + 4;
                        break;
                    }
                }
                let headers = String::from_utf8_lossy(&buf[..header_end]).to_ascii_lowercase();
                let len: usize = headers
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                while buf.len() < header_end + len {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                let req: Value = serde_json::from_slice(&buf[header_end..]).unwrap_or(Value::Null);
                let body = json!({"choices": [{"message": {"role": "assistant",
                                                           "content": reply(&req)},
                                               "finish_reason": "stop"}]})
                .to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    format!("http://{addr}/v1")
}

#[tokio::test]
async fn scanned_pages_go_through_ocr() {
    if pi_extract::pdfium::bindings().is_err() {
        eprintln!("skipped: PDFium not available");
        return;
    }
    let scan = PathBuf::from("/home/user/PageIndex-rust/parity/scans/earthmover__scan150.pdf");
    let Ok(bytes) = std::fs::read(&scan) else {
        eprintln!("skipped: {} not found", scan.display());
        return;
    };
    let reply = json!({"blocks": [
        {"bbox": [100, 100, 1100, 160], "kind": "title", "level": 1,
         "text": "Mock OCR Heading"},
        {"bbox": [100, 200, 1100, 400], "kind": "text",
         "lines": [{"bbox": [100, 200, 1100, 240], "text": "mock ocr body line one"},
                   {"bbox": [100, 250, 1100, 290], "text": "mock ocr body line two"}]}
    ]});
    let base_url = mock_ocr_server(reply).await;
    let mut ocr = pi_config::Config::default().ocr;
    ocr.base_url = Some(base_url);
    ocr.model = Some("mock-ocr".into());
    ocr.concurrency = 4;
    let opts = IndexOptions {
        ocr: OcrMode::Auto,
        ocr_config: Some(ocr),
        ..llm_free()
    };
    let doc = index_document(bytes.clone(), "scan.pdf", &opts, None)
        .await
        .unwrap();
    assert!(!doc.triage.is_empty());
    assert!(doc.triage.iter().any(|p| p.ocr), "{:?}", doc.triage);
    assert!(
        doc.page_texts
            .iter()
            .any(|t| t.contains("mock ocr body line one"))
    );
    assert!(
        doc.metadata().unwrap()["page_labels"]["scanned"]
            .as_u64()
            .unwrap()
            > 0
    );
    // Without OCR the scan has no text layer and is refused as unreadable.
    let plain = index_document(bytes, "scan.pdf", &llm_free(), None)
        .await
        .unwrap();
    assert_eq!(plain.toc_source(), "unreadable");
}

#[tokio::test]
async fn paddleocr_vl_profile_puts_tables_in_page_texts() {
    if pi_extract::pdfium::bindings().is_err() {
        eprintln!("skipped: PDFium not available");
        return;
    }
    let scan = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../parity/scans/earthmover__scan150.pdf");
    let Ok(bytes) = std::fs::read(&scan) else {
        eprintln!("skipped: {} not found", scan.display());
        return;
    };
    let base_url = mock_chat_server(|req| {
        assert_eq!(req["skip_special_tokens"], false);
        let loc = |v: [u32; 4]| v.iter().map(|n| format!("<|LOC_{n}|>")).collect::<String>();
        match req["messages"][0]["content"][1]["text"].as_str() {
            Some("Spotting:") => {
                let mut s = format!("1. INTRODUCTION{}\n", loc([100, 60, 500, 80]));
                s += &format!("paddle body line one{}\n", loc([100, 100, 900, 120]));
                for (k, (a, b)) in [("Revenue", "1,234"), ("Cost", "(812)"), ("Profit", "422")]
                    .iter()
                    .enumerate()
                {
                    let y = 300 + 30 * k as u32;
                    s += &format!(
                        "{a}{}\n{b}{}\n",
                        loc([100, y, 400, y + 20]),
                        loc([700, y, 800, y + 20])
                    );
                }
                s
            }
            Some("Table Recognition:") => {
                "<fcel>Revenue<fcel>1,234<nl><fcel>Cost<fcel>(812)<nl><fcel>Profit<fcel>422<nl>"
                    .into()
            }
            other => panic!("unexpected prompt {other:?}"),
        }
    })
    .await;
    let mut ocr = pi_config::Config::default().ocr;
    ocr.profile = "paddleocr-vl".into();
    ocr.base_url = Some(base_url);
    ocr.model = Some("PaddleOCR-VL-1.6".into());
    ocr.concurrency = 4;
    let opts = IndexOptions {
        ocr: OcrMode::Auto,
        ocr_config: Some(ocr),
        ..llm_free()
    };
    let doc = index_document(bytes, "scan.pdf", &opts, None)
        .await
        .unwrap();
    assert!(doc.triage.iter().all(|p| p.ocr), "{:?}", doc.triage);
    for t in &doc.page_texts {
        assert!(t.contains("paddle body line one"), "{t}");
        assert!(
            t.contains("| Revenue | 1,234 |\n| --- | --- |\n| Cost | (812) |"),
            "{t}"
        );
    }
}
