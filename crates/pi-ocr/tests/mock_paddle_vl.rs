//! End-to-end OCR routing against a mock PaddleOCR-VL server (OpenAI-compatible API).
//!
//! Needs PDFium (`PDFIUM_LIB`) and `parity/scans/earthmover__scan150.pdf`
//! (`parity/make_scans.py`); skips when either is missing.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use pi_ocr::{OcrOptions, PaddleVlConfig, PaddleVlEngine, PageSource, ocr_and_route};
use pi_triage::{PageLabel, Thresholds, triage_pdf_bytes};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn scan_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../parity/scans/earthmover__scan150.pdf")
}

fn loc(v: u32) -> String {
    format!("<|LOC_{v}|>")
}

/// Spotting reply: a heading, two prose lines, and a 4-row numeric table (text-first, 4 coords).
fn spotting_reply() -> String {
    let mut s = String::new();
    let mut line = |text: &str, x0: u32, y0: u32, x1: u32, y1: u32| {
        s.push_str(text);
        for v in [x0, y0, x1, y1] {
            s.push_str(&loc(v));
        }
        s.push('\n');
    };
    line("1. INTRODUCTION", 100, 60, 500, 80);
    line("The earth mover's distance is a metric", 100, 100, 900, 120);
    line("between distributions.", 100, 125, 600, 145);
    for (k, (a, b)) in [
        ("Revenue", "1,234"),
        ("Cost of sales", "(812)"),
        ("Gross profit", "422"),
        ("Net income", "97"),
    ]
    .iter()
    .enumerate()
    {
        let y = 300 + 30 * k as u32;
        line(a, 100, y, 400, y + 20);
        line(b, 700, y, 800, y + 20);
    }
    s
}

const OTSL_REPLY: &str = "<fcel>Revenue<fcel>1,234<nl><fcel>Cost of sales<fcel>(812)<nl>\
<fcel>Gross profit<fcel>422<nl><fcel>Net income<fcel>97<nl>";

async fn serve(listener: TcpListener, table_calls: Arc<AtomicUsize>) {
    loop {
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };
        let table_calls = table_calls.clone();
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let mut tmp = [0u8; 65536];
            let body_start = loop {
                let n = sock.read(&mut tmp).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                buf.extend_from_slice(&tmp[..n]);
                if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break p + 4;
                }
            };
            let head = String::from_utf8_lossy(&buf[..body_start]).to_lowercase();
            let len: usize = head
                .lines()
                .find_map(|l| {
                    l.strip_prefix("content-length:")
                        .map(|v| v.trim().parse().unwrap_or(0))
                })
                .unwrap_or(0);
            while buf.len() < body_start + len {
                let n = sock.read(&mut tmp).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            let req: serde_json::Value = serde_json::from_slice(&buf[body_start..]).unwrap();
            // PaddleOCR-VL request contract
            assert_eq!(req["skip_special_tokens"], false);
            assert!(req.get("response_format").is_none());
            let msgs = req["messages"].as_array().unwrap();
            assert_eq!(msgs.len(), 1, "no system message");
            assert_eq!(msgs[0]["role"], "user");
            assert!(
                msgs[0]["content"][0]["image_url"]["url"]
                    .as_str()
                    .unwrap()
                    .starts_with("data:image/png;base64,")
            );
            let content = match msgs[0]["content"][1]["text"].as_str().unwrap() {
                "Spotting:" => spotting_reply(),
                "Table Recognition:" => {
                    table_calls.fetch_add(1, Ordering::SeqCst);
                    OTSL_REPLY.to_string()
                }
                other => panic!("unexpected prompt {other:?}"),
            };
            let body = serde_json::json!({"choices": [{"message": {"role": "assistant", "content": content}}]}).to_string();
            let out = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(out.as_bytes()).await;
        });
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn paddleocr_vl_spotting_and_tables() {
    let path = scan_path();
    if !path.exists() || pi_extract::pdfium::bindings().is_err() {
        eprintln!("skipping: needs {} and PDFium", path.display());
        return;
    }
    let bytes = std::fs::read(&path).unwrap();
    let triage = triage_pdf_bytes(bytes.clone(), &Thresholds::default()).unwrap();
    assert!(triage.iter().all(|t| t.label == PageLabel::Scanned));
    let text_pages = pi_extract::extract_pdf_bytes(bytes.clone()).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let table_calls = Arc::new(AtomicUsize::new(0));
    tokio::spawn(serve(listener, table_calls.clone()));
    let engine = PaddleVlEngine::new(PaddleVlConfig::new(
        format!("http://{addr}/v1"),
        "PaddleOCR-VL-1.6",
        None,
    ))
    .unwrap();
    let opts = OcrOptions {
        concurrency: 4,
        ..Default::default()
    };
    let routed = ocr_and_route(bytes, text_pages, &triage, &engine, &opts)
        .await
        .unwrap();

    assert_eq!(routed.len(), 12);
    assert_eq!(
        table_calls.load(Ordering::SeqCst),
        12,
        "one table region per page"
    );
    for p in &routed {
        assert_eq!(p.source, PageSource::Ocr, "{:?}", p.error);
        let spans = &p.spans.spans;
        assert_eq!(spans.len(), 11);
        assert_eq!(spans[0].text, "1. INTRODUCTION");
        // pixel y grows downward, PDF y upward
        assert!(spans[0].bbox.bottom > spans[1].bbox.top);
        let vb = p.spans.viewbox.unwrap();
        assert!(
            spans
                .iter()
                .all(|s| s.bbox.left >= vb[0] - 1.0 && s.bbox.right <= vb[2] + 1.0)
        );
        assert_eq!(p.tables.len(), 1);
        assert_eq!(
            p.tables[0].markdown,
            "| Revenue | 1,234 |\n| --- | --- |\n| Cost of sales | (812) |\n| Gross profit | 422 |\n| Net income | 97 |"
        );
    }
}
