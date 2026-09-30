//! Port of `pageindex/local_store.py` (`DocStore`): the `.pageindex/` directory.
//!
//! Layout: `manifest.json` (`{"docs": {id: meta}}`, a cache), `.lock` (flock), and
//! `docs/<id>/{tree.json,pages.json,doc.json}`. `doc.json` is the source of truth for a
//! document's existence.

use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::consts::{DOC_FILE, DOCS_DIR, LOCK_FILE, MANIFEST_FILE, PAGES_FILE, TREE_FILE};
use crate::pyjson;

pub type Meta = Map<String, Value>;

/// `_write_json_atomic`: temp file beside the target, fsync, rename. // ref: local_store.py:15
pub fn write_json_atomic(path: &Path, data: &Value) -> io::Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!("{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        f.write_all(pyjson::dumps(data).as_bytes())?;
        f.flush()?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// `_read_json`: absent, unreadable-by-permission or malformed files read as `None`;
/// other I/O errors propagate. // ref: local_store.py:31
pub fn read_json(path: &Path) -> io::Result<Option<Value>> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e)
            if matches!(
                e.kind(),
                ErrorKind::NotFound
                    | ErrorKind::NotADirectory
                    | ErrorKind::IsADirectory
                    | ErrorKind::PermissionDenied
            ) =>
        {
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    // UnicodeDecodeError and JSONDecodeError are both ValueError in Python.
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Ok(None);
    };
    Ok(serde_json::from_str(text).ok())
}

/// `_is_safe_id`. // ref: local_store.py:43
pub fn is_safe_id(value: &str) -> bool {
    !matches!(value, "" | "." | "..") && !value.contains('/') && !value.contains('\\')
}

fn is_int(v: Option<&Value>) -> Option<i128> {
    match v? {
        Value::Number(n) => n
            .as_i64()
            .map(i128::from)
            .or_else(|| n.as_u64().map(i128::from)),
        _ => None,
    }
}

fn is_str_or_null(v: Option<&Value>) -> bool {
    matches!(v, None | Some(Value::Null) | Some(Value::String(_)))
}

/// `_is_valid_meta`. // ref: local_store.py:52
pub fn is_valid_meta(meta: Option<&Value>, doc_id: &str) -> bool {
    let Some(Value::Object(meta)) = meta else {
        return false;
    };
    if meta.get("id").and_then(Value::as_str) != Some(doc_id) {
        return false;
    }
    matches!(meta.get("name"), Some(Value::String(_)))
        && is_str_or_null(meta.get("description"))
        && matches!(meta.get("status"), Some(Value::String(_)))
        && matches!(meta.get("createdAt"), Some(Value::String(_)))
        && is_int(meta.get("pageNum")).is_some_and(|n| n >= 0)
        && is_str_or_null(meta.get("folderId"))
        && matches!(
            meta.get("metadata"),
            None | Some(Value::Null) | Some(Value::Object(_))
        )
        && is_str_or_null(meta.get("mode"))
}

/// `os.path.expanduser` for the leading `~` / `~/`.
fn expand_user(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if (s == "~" || s.starts_with("~/"))
        && let Some(home) = std::env::var_os("HOME")
    {
        let mut p = PathBuf::from(home);
        if s.len() > 2 {
            p.push(&s[2..]);
        }
        return p;
    }
    path.to_path_buf()
}

/// Exclusive advisory lock on `<root>/.lock`, released on drop. Uses `flock(2)`, as the
/// reference's `fcntl.flock(handle, fcntl.LOCK_EX)` does, so Rust and Python writers
/// exclude each other.
#[derive(Debug)]
pub struct StoreLock {
    file: File,
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// `DocStore`. // ref: local_store.py:73
#[derive(Debug, Clone)]
pub struct DocStore {
    root: PathBuf,
    docs: PathBuf,
    manifest: PathBuf,
}

impl DocStore {
    pub fn new(storage_dir: impl AsRef<Path>) -> Self {
        let root = expand_user(storage_dir.as_ref());
        DocStore {
            docs: root.join(DOCS_DIR),
            manifest: root.join(MANIFEST_FILE),
            root,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Directory of one document, or `None` for an unsafe id.
    pub fn doc_dir(&self, doc_id: &str) -> Option<PathBuf> {
        is_safe_id(doc_id).then(|| self.docs.join(doc_id))
    }

    fn read_manifest(&self) -> io::Result<Meta> {
        Ok(match read_json(&self.manifest)? {
            Some(Value::Object(mut data)) => match data.remove("docs") {
                Some(Value::Object(docs)) => docs,
                _ => Map::new(),
            },
            _ => Map::new(),
        })
    }

    fn write_manifest(&self, docs: Meta) {
        let mut data = Map::new();
        data.insert("docs".into(), Value::Object(docs));
        // OSError is swallowed: the manifest is a cache. // ref: local_store.py:90-94
        let _ = write_json_atomic(&self.manifest, &Value::Object(data));
    }

    /// `lock()`: cross-process mutex for check-then-write sequences. // ref: local_store.py:97
    pub fn lock(&self) -> io::Result<StoreLock> {
        fs::create_dir_all(&self.root)?;
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(self.root.join(LOCK_FILE))?;
        file.lock()?;
        Ok(StoreLock { file })
    }

    /// `save_document`: tree, pages, then doc.json (the commit point), then the manifest.
    /// // ref: local_store.py:115
    pub fn save_document(
        &self,
        doc_id: &str,
        meta: &Meta,
        tree: &Value,
        pages: &Value,
    ) -> io::Result<()> {
        let doc_dir = self.doc_dir(doc_id).ok_or_else(|| {
            io::Error::new(
                ErrorKind::InvalidInput,
                format!("Invalid doc_id: {}", pyjson::str_repr(doc_id)),
            )
        })?;
        fs::create_dir_all(&doc_dir)?;
        write_json_atomic(&doc_dir.join(TREE_FILE), tree)?;
        write_json_atomic(&doc_dir.join(PAGES_FILE), pages)?;
        write_json_atomic(&doc_dir.join(DOC_FILE), &Value::Object(meta.clone()))?;
        let mut manifest = self.read_manifest()?;
        manifest.insert(doc_id.to_string(), Value::Object(meta.clone()));
        self.write_manifest(manifest);
        Ok(())
    }

    fn read_doc_file(&self, doc_id: &str, name: &str) -> io::Result<Option<Value>> {
        match self.doc_dir(doc_id) {
            Some(dir) if dir.join(DOC_FILE).is_file() => read_json(&dir.join(name)),
            _ => Ok(None),
        }
    }

    /// `get_meta`: doc.json, falling back to the manifest copy. // ref: local_store.py:133
    pub fn get_meta(&self, doc_id: &str) -> io::Result<Option<Meta>> {
        let Some(dir) = self.doc_dir(doc_id) else {
            return Ok(None);
        };
        if !dir.join(DOC_FILE).is_file() {
            return Ok(None);
        }
        let mut meta = read_json(&dir.join(DOC_FILE))?;
        if !is_valid_meta(meta.as_ref(), doc_id) {
            meta = self.read_manifest()?.remove(doc_id);
        }
        Ok(match meta {
            Some(Value::Object(m)) if is_valid_meta(Some(&Value::Object(m.clone())), doc_id) => {
                Some(m)
            }
            _ => None,
        })
    }

    /// `get_tree`: tree.json verbatim. // ref: local_store.py:142
    pub fn get_tree(&self, doc_id: &str) -> io::Result<Option<Value>> {
        self.read_doc_file(doc_id, TREE_FILE)
    }

    /// `get_pages`: pages.json verbatim. // ref: local_store.py:145
    pub fn get_pages(&self, doc_id: &str) -> io::Result<Option<Value>> {
        self.read_doc_file(doc_id, PAGES_FILE)
    }

    /// `list_metas`: every document directory holding a doc.json, validated; rewrites the
    /// manifest when it has drifted. // ref: local_store.py:148
    ///
    /// The reference iterates a `set` of directory names (hash order); this port iterates
    /// them sorted, so the rewritten manifest's key order is deterministic.
    pub fn list_metas(&self) -> io::Result<Vec<Meta>> {
        if !self.docs.is_dir() {
            return Ok(Vec::new());
        }
        let mut dir_names = Vec::new();
        for entry in fs::read_dir(&self.docs)? {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if entry.path().is_dir() && is_safe_id(&name) {
                dir_names.push(name);
            }
        }
        dir_names.sort();
        let cached = self.read_manifest()?;
        let mut fresh = Map::new();
        for name in dir_names {
            let doc_json = self.docs.join(&name).join(DOC_FILE);
            if !doc_json.is_file() {
                continue;
            }
            let mut meta = cached.get(&name).cloned();
            if !is_valid_meta(meta.as_ref(), &name) {
                meta = read_json(&doc_json)?;
            }
            if let Some(meta) = meta.filter(|m| is_valid_meta(Some(m), &name)) {
                fresh.insert(name, meta);
            }
        }
        if fresh != cached {
            self.write_manifest(fresh.clone());
        }
        Ok(fresh
            .into_iter()
            .filter_map(|(_, v)| match v {
                Value::Object(m) => Some(m),
                _ => None,
            })
            .collect())
    }

    /// `delete_document`: true when doc.json existed. // ref: local_store.py:168
    pub fn delete_document(&self, doc_id: &str) -> io::Result<bool> {
        let Some(dir) = self.doc_dir(doc_id) else {
            return Ok(false);
        };
        let doc_json = dir.join(DOC_FILE);
        let existed = match fs::remove_file(&doc_json) {
            Ok(()) => true,
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => false,
            Err(e) => {
                if !doc_json.is_dir() {
                    return Err(e);
                }
                false
            }
        };
        if dir.is_dir() {
            let _ = fs::remove_dir_all(&dir);
        }
        let mut manifest = self.read_manifest()?;
        if manifest.shift_remove(doc_id).is_some_and(|v| !v.is_null()) {
            self.write_manifest(manifest);
        }
        Ok(existed)
    }
}
