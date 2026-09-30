//! End-to-end OCR routing against a mock OpenAI-compatible endpoint.
//!
//! Needs PDFium (`PDFIUM_LIB`) and the synthetic scan made by `parity/make_scans.py`
//! (`parity/scans/earthmover__scan150.pdf`); skips when either is missing.

use std::path::PathBuf;

use pi_ocr::{OcrOptions, OpenAiVisionConfig, OpenAiVisionEngine, PageSource, ocr_and_route};
use pi_triage::{PageLabel, Thresholds, triage_pdf_bytes};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn scan_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../parity/scans/earthmover__scan150.pdf")
}

/// Minimal HTTP/1.1 server answering chat-completions with a fixed JSON block layout sized to
/// the image dimensions announced in the request text.
async fn serve(listener: TcpListener) {
    loop {
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let mut tmp = [0u8; 65536];
            let body_start;
            loop {
                let n = sock.read(&mut tmp).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                buf.extend_from_slice(&tmp[..n]);
                if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    body_start = p + 4;
                    break;
                }
            }
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
            assert!(head.contains("authorization: bearer test-key"));
            let content = &req["messages"][1]["content"];
            assert!(
                content[1]["image_url"]["url"]
                    .as_str()
                    .unwrap()
                    .starts_with("data:image/png;base64,")
            );
            let text = content[0]["text"].as_str().unwrap();
            let dims = text
                .split("Image is ")
                .nth(1)
                .unwrap()
                .trim_end_matches(" px.");
            let (w, h): (f64, f64) = {
                let mut it = dims.split('x').map(|v| v.parse::<f64>().unwrap());
                (it.next().unwrap(), it.next().unwrap())
            };
            let blocks = serde_json::json!({"blocks": [
                {"bbox": [0.1*w, 0.05*h, 0.9*w, 0.08*h], "kind": "title", "level": 1, "text": "1. INTRODUCTION"},
                {"bbox": [0.1*w, 0.10*h, 0.9*w, 0.16*h], "kind": "text", "text": "first line\nsecond line",
                 "lines": [{"bbox": [0.1*w, 0.10*h, 0.9*w, 0.125*h], "text": "first line"},
                           {"bbox": [0.1*w, 0.135*h, 0.9*w, 0.16*h], "text": "second line"}]},
                {"bbox": [0.1*w, 0.5*h, 0.9*w, 0.6*h], "kind": "table",
                 "html": "<table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>", "text": "A B 1 2"}
            ]});
            let resp = serde_json::json!({"choices": [{"message": {"role": "assistant", "content": blocks.to_string()}}]});
            let body = resp.to_string();
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
async fn scanned_pages_become_ocr_spans() {
    let path = scan_path();
    if !path.exists() || pi_extract::pdfium::bindings().is_err() {
        eprintln!("skipping: needs {} and PDFium", path.display());
        return;
    }
    let bytes = std::fs::read(&path).unwrap();
    let triage = triage_pdf_bytes(bytes.clone(), &Thresholds::default()).unwrap();
    assert!(triage.iter().all(|t| t.label == PageLabel::Scanned));
    let text_pages = pi_extract::extract_pdf_bytes(bytes.clone()).unwrap();
    assert!(text_pages.iter().all(|p| p.spans.is_empty()));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve(listener));
    let engine = OpenAiVisionEngine::new(OpenAiVisionConfig::new(
        format!("http://{addr}/v1"),
        "mock-ocr",
        Some("test-key".into()),
    ))
    .unwrap();
    let opts = OcrOptions {
        dpi: 72.0,
        concurrency: 4,
        ..Default::default()
    };
    let routed = ocr_and_route(bytes, text_pages, &triage, &engine, &opts)
        .await
        .unwrap();

    assert_eq!(routed.len(), 12);
    for p in &routed {
        assert_eq!(p.source, PageSource::Ocr, "{:?}", p.error);
        let texts: Vec<&str> = p.spans.spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts[..3], ["1. INTRODUCTION", "first line", "second line"]);
        let vb = p.spans.viewbox.unwrap();
        let title = &p.spans.spans[0].bbox;
        let body = &p.spans.spans[1].bbox;
        // pixel y grows downward, PDF y upward: the title sits above the body text
        assert!(title.bottom > body.top, "{title:?} vs {body:?}");
        assert!(
            title.left >= vb[0] - 1.0 && title.right <= vb[2] + 1.0 && title.top <= vb[3] + 1.0
        );
        assert!(matches!(
            p.spans.spans[0].source,
            pi_core::SpanSource::Ocr { .. }
        ));
        assert_eq!(p.hints.len(), 1);
        assert_eq!(p.tables[0].markdown, "| A | B |\n| --- | --- |\n| 1 | 2 |");
    }
}
