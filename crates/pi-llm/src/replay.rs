//! Record / replay / stub backends, compatible with `parity/mock_llm.py`.
//!
//! Fixture format: JSONL of `{"key", "model", "messages", "reply"}`; only `key` and `reply` are
//! needed to replay (`messages` may be omitted to keep committed fixtures small).

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};

use crate::consts::STUB_REPLY;
use crate::{Llm, LlmError, Message, Result, key_of, pyjson};

fn parse_fixture(text: &str) -> Result<HashMap<String, String>> {
    let mut replies = HashMap::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let rec: Value = serde_json::from_str(line)
            .map_err(|e| LlmError::Other(format!("fixture line {}: {e}", n + 1)))?;
        let key = rec
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| LlmError::Other(format!("fixture line {}: no key", n + 1)))?;
        // a recorded `None` content replays as an empty reply
        let reply = rec.get("reply").and_then(Value::as_str).unwrap_or("");
        replies.insert(key.to_string(), reply.to_string());
    }
    Ok(replies)
}

/// Serves replies from a fixture; a miss is an error (never calls out).
#[derive(Debug, Default)]
pub struct ReplayLlm {
    replies: HashMap<String, String>,
    hits: Mutex<HashSet<String>>,
    misses: Mutex<Vec<String>>,
}

impl ReplayLlm {
    pub fn from_jsonl(text: &str) -> Result<Self> {
        Ok(ReplayLlm {
            replies: parse_fixture(text)?,
            ..Default::default()
        })
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let text = std::fs::read_to_string(path.as_ref())
            .map_err(|e| LlmError::Other(format!("{}: {e}", path.as_ref().display())))?;
        Self::from_jsonl(&text)
    }

    pub fn len(&self) -> usize {
        self.replies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.replies.is_empty()
    }

    /// Keys requested but absent from the fixture, in request order.
    pub fn misses(&self) -> Vec<String> {
        self.misses.lock().unwrap().clone()
    }

    /// Fixture keys never requested, sorted.
    pub fn unused_keys(&self) -> Vec<String> {
        let hits = self.hits.lock().unwrap();
        let mut out: Vec<String> = self
            .replies
            .keys()
            .filter(|k| !hits.contains(*k))
            .cloned()
            .collect();
        out.sort();
        out
    }
}

#[async_trait::async_trait]
impl Llm for ReplayLlm {
    async fn complete(&self, messages: &[Message]) -> Result<String> {
        let key = key_of(messages);
        match self.replies.get(&key) {
            Some(reply) => {
                self.hits.lock().unwrap().insert(key);
                Ok(reply.clone())
            }
            None => {
                self.misses.lock().unwrap().push(key.clone());
                Err(LlmError::Miss { key })
            }
        }
    }
}

/// `mock_llm` stub mode: the same canned reply for every prompt.
#[derive(Debug, Default, Clone, Copy)]
pub struct StubLlm;

#[async_trait::async_trait]
impl Llm for StubLlm {
    async fn complete(&self, _messages: &[Message]) -> Result<String> {
        Ok(STUB_REPLY.to_string())
    }
}

/// `mock_llm` record mode: serve what the fixture already has, otherwise call `inner` and
/// append the reply to the fixture file.
pub struct RecordingLlm {
    inner: Arc<dyn Llm>,
    model: String,
    path: PathBuf,
    replies: Mutex<HashMap<String, String>>,
}

impl RecordingLlm {
    pub fn new(
        inner: Arc<dyn Llm>,
        model: impl Into<String>,
        path: impl Into<PathBuf>,
    ) -> Result<Self> {
        let path = path.into();
        let replies = match std::fs::read_to_string(&path) {
            Ok(text) => parse_fixture(&text)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(LlmError::Other(format!("{}: {e}", path.display()))),
        };
        Ok(RecordingLlm {
            inner,
            model: model.into(),
            path,
            replies: Mutex::new(replies),
        })
    }

    fn add(&self, key: String, messages: &[Message], reply: &str) -> Result<()> {
        let mut replies = self.replies.lock().unwrap();
        if replies.contains_key(&key) {
            return Ok(());
        }
        let mut rec = Map::new();
        rec.insert("key".into(), Value::String(key.clone()));
        rec.insert("model".into(), Value::String(self.model.clone()));
        rec.insert(
            "messages".into(),
            serde_json::to_value(messages).map_err(|e| LlmError::Other(e.to_string()))?,
        );
        rec.insert("reply".into(), Value::String(reply.to_string()));
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| LlmError::Other(e.to_string()))?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| LlmError::Other(format!("{}: {e}", self.path.display())))?;
        writeln!(f, "{}", pyjson::dumps(&Value::Object(rec)))
            .map_err(|e| LlmError::Other(e.to_string()))?;
        replies.insert(key, reply.to_string());
        Ok(())
    }
}

#[async_trait::async_trait]
impl Llm for RecordingLlm {
    async fn complete(&self, messages: &[Message]) -> Result<String> {
        let key = key_of(messages);
        if let Some(reply) = self.replies.lock().unwrap().get(&key) {
            return Ok(reply.clone());
        }
        let reply = self.inner.complete(messages).await?;
        self.add(key, messages, &reply)?;
        Ok(reply)
    }
}
