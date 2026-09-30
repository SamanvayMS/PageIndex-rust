//! pi-llm against CPython values recorded by `gen/llm_fixture.py`, plus the retry ladder
//! against a local fake server.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use pi_llm::client::{parse_content, wire_model, with_retries};
use pi_llm::{
    Llm, LlmError, Message, OpenAiClient, RecordingLlm, ReplayLlm, RoleConfig, StubLlm,
    count_tokens, key_of, pyjson,
};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/llm.json")).unwrap()
}

fn messages(v: &Value) -> Vec<Message> {
    serde_json::from_value(v.clone()).unwrap()
}

#[test]
fn key_matches_python_json_dumps() {
    for case in fixture()["keys"].as_array().unwrap() {
        let m = messages(&case["messages"]);
        assert_eq!(key_of(&m), case["key"].as_str().unwrap(), "{m:?}");
    }
}

#[test]
fn record_line_matches_python_default_separators() {
    let f = fixture();
    let multi = &f["keys"].as_array().unwrap().last().unwrap()["messages"];
    let mut rec = serde_json::Map::new();
    rec.insert("key".into(), "k".into());
    rec.insert("model".into(), "m".into());
    rec.insert("messages".into(), multi.clone());
    rec.insert("reply".into(), "r\u{e9}\n\"x\"".into());
    assert_eq!(
        pyjson::dumps(&Value::Object(rec)),
        f["record_line"].as_str().unwrap()
    );
}

#[test]
fn token_counts_match_litellm() {
    for case in fixture()["tokens"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        let model = case["model"].as_str();
        let want = case["count"].as_u64().unwrap() as usize;
        assert_eq!(
            count_tokens(text, model),
            want,
            "model={model:?} text={text:?}"
        );
    }
}

#[tokio::test]
async fn replay_hits_and_misses() {
    let m = vec![Message::user("hello")];
    let line = format!(
        "{{\"key\": \"{}\", \"model\": \"x\", \"reply\": \"hi\"}}\n",
        key_of(&m)
    );
    let r = ReplayLlm::from_jsonl(&line).unwrap();
    assert_eq!(r.complete(&m).await.unwrap(), "hi");
    assert!(r.unused_keys().is_empty());
    let err = r.complete(&[Message::user("other")]).await.unwrap_err();
    assert!(matches!(err, LlmError::Miss { .. }));
    assert!(!err.is_unrecoverable());
    assert_eq!(r.misses().len(), 1);
}

#[tokio::test]
async fn stub_reply_is_mock_llm_stub() {
    let reply = StubLlm.complete(&[Message::user("x")]).await.unwrap();
    assert_eq!(
        reply,
        r#"{"subsections": [], "summary": "stub summary", "title": "stub title", "description": "stub description"}"#
    );
}

#[tokio::test]
async fn recording_round_trips_through_replay() {
    let dir = std::env::temp_dir().join(format!("pi-llm-rec-{}", std::process::id()));
    let path = dir.join("rec.jsonl");
    let _ = std::fs::remove_file(&path);
    let rec = RecordingLlm::new(Arc::new(StubLlm), "openai/mock", &path).unwrap();
    let m = vec![Message::user("a \u{e9} \"q\"")];
    rec.complete(&m).await.unwrap();
    rec.complete(&m).await.unwrap(); // second call served from the store, not re-appended
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().count(), 1);
    let replay = ReplayLlm::from_jsonl(&text).unwrap();
    assert_eq!(
        replay.complete(&m).await.unwrap(),
        pi_llm::consts::STUB_REPLY
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn error_classification_matches_reference() {
    let st = |s| LlmError::Status {
        status: s,
        message: String::new(),
    };
    let ex = |s| LlmError::RetriesExhausted {
        attempts: 10,
        status: s,
        message: String::new(),
    };
    for s in [400, 401, 403, 404] {
        assert!(st(s).is_no_retry());
    }
    assert!(!st(429).is_no_retry() && !st(500).is_no_retry());
    assert!(st(401).is_unrecoverable() && !st(400).is_unrecoverable());
    assert!(ex(None).is_unrecoverable() && ex(Some(500)).is_unrecoverable());
    assert!(!ex(Some(400)).is_unrecoverable());
    assert!(!LlmError::Other("x".into()).is_unrecoverable());
}

#[tokio::test]
async fn retry_ladder_counts_attempts() {
    let calls = AtomicUsize::new(0);
    let res = with_retries(10, Duration::ZERO, || {
        calls.fetch_add(1, Ordering::SeqCst);
        async {
            Err::<String, _>(LlmError::Status {
                status: 500,
                message: "boom".into(),
            })
        }
    })
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 10);
    assert!(matches!(
        res,
        Err(LlmError::RetriesExhausted {
            status: Some(500),
            ..
        })
    ));

    let calls = AtomicUsize::new(0);
    let res = with_retries(10, Duration::ZERO, || {
        calls.fetch_add(1, Ordering::SeqCst);
        async {
            Err::<String, _>(LlmError::Status {
                status: 400,
                message: "ctx".into(),
            })
        }
    })
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(matches!(res, Err(LlmError::Status { status: 400, .. })));

    let calls = AtomicUsize::new(0);
    let res = with_retries(10, Duration::ZERO, || {
        let n = calls.fetch_add(1, Ordering::SeqCst);
        async move {
            if n < 3 {
                Err(LlmError::Other("reset".into()))
            } else {
                Ok("ok".to_string())
            }
        }
    })
    .await;
    assert_eq!(res.unwrap(), "ok");
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

#[test]
fn wire_model_and_content_parsing() {
    assert_eq!(wire_model("openai/gpt-5.6-luna"), "gpt-5.6-luna");
    assert_eq!(wire_model("litellm/openai/x"), "x");
    assert_eq!(wire_model("anthropic/claude"), "anthropic/claude");
    assert_eq!(
        parse_content(r#"{"choices":[{"message":{"role":"assistant","content":"hi"}}]}"#).unwrap(),
        "hi"
    );
    assert_eq!(
        parse_content(r#"{"choices":[{"message":{"content":null}}]}"#).unwrap(),
        ""
    );
    assert!(parse_content("not json").is_err());
}

/// A local one-shot-per-connection HTTP server answering each request with the next scripted
/// (status, body); returns the base URL and the request bodies it saw.
async fn fake_server(script: Vec<(u16, String)>) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    tokio::spawn(async move {
        for (status, body) in script {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            loop {
                let n = sock.read(&mut tmp).await.unwrap();
                buf.extend_from_slice(&tmp[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some(h) = text.find("\r\n\r\n") {
                    let len = text[..h]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if buf.len() >= h + 4 + len {
                        seen2.lock().unwrap().push(text[h + 4..].to_string());
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let resp = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            sock.shutdown().await.ok();
        }
    });
    (format!("http://{addr}/v1"), seen)
}

#[tokio::test]
async fn client_retries_server_errors_then_succeeds() {
    let ok = r#"{"choices":[{"message":{"role":"assistant","content":"done"}}]}"#.to_string();
    let (url, seen) = fake_server(vec![(500, "{}".into()), (429, "{}".into()), (200, ok)]).await;
    let mut cfg = RoleConfig::new(url, "openai/mock-model");
    cfg.retry_delay = Duration::ZERO;
    cfg.api_key = Some("sk-test".into());
    let client = OpenAiClient::new(cfg).unwrap();
    let reply = client.complete(&[Message::user("hi")]).await.unwrap();
    assert_eq!(reply, "done");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    let body: Value = serde_json::from_str(&seen[0]).unwrap();
    assert_eq!(body["model"], "mock-model");
    assert_eq!(body["messages"][0]["content"], "hi");
}

#[tokio::test]
async fn client_does_not_retry_unrecoverable_status() {
    let (url, seen) = fake_server(vec![(401, r#"{"error":"bad key"}"#.into())]).await;
    let mut cfg = RoleConfig::new(url, "m");
    cfg.retry_delay = Duration::ZERO;
    let client = OpenAiClient::new(cfg).unwrap();
    let err = client.complete(&[Message::user("hi")]).await.unwrap_err();
    assert!(matches!(err, LlmError::Status { status: 401, .. }));
    assert!(err.is_unrecoverable());
    assert_eq!(seen.lock().unwrap().len(), 1);
}
