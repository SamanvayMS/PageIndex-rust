//! Local / S3 / GCS input sources and the remote mirror of a `.pageindex/` store.
//!
//! * [`Source`] lists and fetches input PDFs from a local path or an object store. Remote
//!   PDFs are streamed to a temporary file that is deleted when the returned
//!   [`FetchedPdf`] drops: source documents are never persisted locally.
//! * [`Mirror`] uploads a committed document's JSON files under the same relative layout
//!   (`docs/<id>/{tree,pages,doc}.json`, then `manifest.json`) to a remote prefix. The
//!   manifest is merged with a conditional put (create-if-absent, or update-if-etag) and
//!   retried on conflict, so concurrent writers never drop each other's entries.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::StreamExt;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt, PutMode, PutOptions, PutPayload, UpdateVersion};
use serde_json::{Map, Value};

use pi_config::{IngestSource, RemoteUri, Scheme};
use pi_store::consts::{DOC_FILE, DOCS_DIR, MANIFEST_FILE, PAGES_FILE, TREE_FILE};
use pi_store::pyjson;

/// Conditional manifest writes retried this many times before giving up.
pub const MANIFEST_ATTEMPTS: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("object store: {0}")]
    Store(#[from] object_store::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid object path {0:?}: {1}")]
    Path(String, String),
    #[error("remote manifest at {0} is not valid JSON")]
    BadManifest(String),
    #[error("manifest update still conflicting after {0} attempts")]
    Conflict(usize),
    #[error("{0}")]
    Config(String),
}

pub type Result<T> = std::result::Result<T, StorageError>;

/// Build the object store behind an `s3://` / `gs://` URI from the standard environment
/// (`AWS_*` / `GOOGLE_*` variables), returning it with the key prefix.
pub fn store_for(uri: &RemoteUri) -> Result<(Arc<dyn ObjectStore>, ObjectPath)> {
    let store: Arc<dyn ObjectStore> = match uri.scheme {
        Scheme::S3 => Arc::new(
            object_store::aws::AmazonS3Builder::from_env()
                .with_bucket_name(&uri.bucket)
                .build()?,
        ),
        Scheme::Gs => Arc::new(
            object_store::gcp::GoogleCloudStorageBuilder::from_env()
                .with_bucket_name(&uri.bucket)
                .build()?,
        ),
    };
    Ok((store, object_path(&uri.prefix)?))
}

fn object_path(s: &str) -> Result<ObjectPath> {
    ObjectPath::parse(s.trim_matches('/')).map_err(|e| StorageError::Path(s.into(), e.to_string()))
}

fn join(prefix: &ObjectPath, rel: &str) -> Result<ObjectPath> {
    let joined = if prefix.as_ref().is_empty() {
        rel.to_string()
    } else {
        format!("{}/{rel}", prefix.as_ref())
    };
    object_path(&joined)
}

fn is_pdf(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".pdf")
}

// ── sources ──

/// One input document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEntry {
    /// File name (the upload name before sanitising).
    pub name: String,
    /// Local path or object key.
    pub location: String,
    pub size: Option<u64>,
}

/// A fetched PDF on local disk. For remote sources the file is temporary and is removed
/// when this value drops; for local sources it is the original file.
#[derive(Debug)]
pub struct FetchedPdf {
    path: PathBuf,
    _temp: Option<tempfile::TempPath>,
}

impl FetchedPdf {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Where input PDFs come from.
pub trait Source: Send + Sync {
    /// Every PDF under the source, sorted by location.
    fn list(&self) -> impl Future<Output = Result<Vec<SourceEntry>>> + Send;
    /// Make one entry available as a local file.
    fn fetch(&self, entry: &SourceEntry) -> impl Future<Output = Result<FetchedPdf>> + Send;
}

/// A local file or directory (searched recursively for `*.pdf`).
#[derive(Debug, Clone)]
pub struct LocalSource {
    root: PathBuf,
}

impl LocalSource {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        LocalSource { root: root.into() }
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            walk(&path, out)?;
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(is_pdf)
        {
            out.push(path);
        }
    }
    Ok(())
}

impl Source for LocalSource {
    async fn list(&self) -> Result<Vec<SourceEntry>> {
        let mut paths = Vec::new();
        if self.root.is_dir() {
            walk(&self.root, &mut paths)?;
        } else if self.root.is_file() {
            paths.push(self.root.clone());
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no such file or directory: {}", self.root.display()),
            )
            .into());
        }
        paths.sort();
        Ok(paths
            .into_iter()
            .map(|p| SourceEntry {
                name: p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                size: std::fs::metadata(&p).ok().map(|m| m.len()),
                location: p.to_string_lossy().into_owned(),
            })
            .collect())
    }

    async fn fetch(&self, entry: &SourceEntry) -> Result<FetchedPdf> {
        Ok(FetchedPdf {
            path: PathBuf::from(&entry.location),
            _temp: None,
        })
    }
}

/// PDFs under a prefix of an object store.
#[derive(Debug, Clone)]
pub struct ObjectSource {
    store: Arc<dyn ObjectStore>,
    prefix: ObjectPath,
}

impl ObjectSource {
    pub fn new(store: Arc<dyn ObjectStore>, prefix: &str) -> Result<Self> {
        Ok(ObjectSource {
            store,
            prefix: object_path(prefix)?,
        })
    }
}

impl Source for ObjectSource {
    async fn list(&self) -> Result<Vec<SourceEntry>> {
        let prefix = (!self.prefix.as_ref().is_empty()).then_some(&self.prefix);
        let mut stream = self.store.list(prefix);
        let mut out = Vec::new();
        while let Some(meta) = stream.next().await {
            let meta = meta?;
            let name = meta.location.filename().unwrap_or_default().to_string();
            if is_pdf(&name) {
                out.push(SourceEntry {
                    name,
                    location: meta.location.to_string(),
                    size: Some(meta.size),
                });
            }
        }
        out.sort_by(|a, b| a.location.cmp(&b.location));
        Ok(out)
    }

    async fn fetch(&self, entry: &SourceEntry) -> Result<FetchedPdf> {
        let location = object_path(&entry.location)?;
        let mut stream = self.store.get(&location).await?.into_stream();
        let mut file = tempfile::Builder::new()
            .prefix("pi-src-")
            .suffix(".pdf")
            .tempfile()?;
        while let Some(chunk) = stream.next().await {
            file.write_all(&chunk?)?;
        }
        file.flush()?;
        let temp = file.into_temp_path();
        Ok(FetchedPdf {
            path: temp.to_path_buf(),
            _temp: Some(temp),
        })
    }
}

/// A source chosen at run time from configuration.
#[derive(Debug, Clone)]
pub enum AnySource {
    Local(LocalSource),
    Object(ObjectSource),
}

impl AnySource {
    /// Build from `[ingest] source`.
    pub fn from_config(source: &IngestSource) -> Result<Self> {
        Ok(match source {
            IngestSource::Local(path) => AnySource::Local(LocalSource::new(path)),
            IngestSource::Remote(uri) => {
                let (store, prefix) = store_for(uri)?;
                AnySource::Object(ObjectSource { store, prefix })
            }
        })
    }
}

impl Source for AnySource {
    async fn list(&self) -> Result<Vec<SourceEntry>> {
        match self {
            AnySource::Local(s) => s.list().await,
            AnySource::Object(s) => s.list().await,
        }
    }

    async fn fetch(&self, entry: &SourceEntry) -> Result<FetchedPdf> {
        match self {
            AnySource::Local(s) => s.fetch(entry).await,
            AnySource::Object(s) => s.fetch(entry).await,
        }
    }
}

// ── mirror ──

/// Remote copy of a local `.pageindex/` store.
#[derive(Debug, Clone)]
pub struct Mirror {
    store: Arc<dyn ObjectStore>,
    prefix: ObjectPath,
}

impl Mirror {
    pub fn new(store: Arc<dyn ObjectStore>, prefix: &str) -> Result<Self> {
        Ok(Mirror {
            store,
            prefix: object_path(prefix)?,
        })
    }

    /// Build from `[storage] mirror`.
    pub fn from_uri(uri: &RemoteUri) -> Result<Self> {
        let (store, prefix) = store_for(uri)?;
        Ok(Mirror { store, prefix })
    }

    /// Upload one committed document from the local store at `local_root`: tree, pages,
    /// then doc.json (the commit point), then merge its meta into the remote manifest.
    pub async fn push_document(&self, local_root: &Path, doc_id: &str) -> Result<()> {
        if !pi_store::store::is_safe_id(doc_id) {
            return Err(StorageError::Path(
                doc_id.into(),
                "unsafe document id".into(),
            ));
        }
        let doc_dir = local_root.join(DOCS_DIR).join(doc_id);
        for name in [TREE_FILE, PAGES_FILE, DOC_FILE] {
            let bytes = tokio::fs::read(doc_dir.join(name)).await?;
            let key = join(&self.prefix, &format!("{DOCS_DIR}/{doc_id}/{name}"))?;
            self.store.put(&key, PutPayload::from(bytes)).await?;
        }
        let meta_bytes = tokio::fs::read(doc_dir.join(DOC_FILE)).await?;
        let meta: Value = serde_json::from_slice(&meta_bytes)
            .map_err(|_| StorageError::BadManifest(doc_dir.join(DOC_FILE).display().to_string()))?;
        let doc_id = doc_id.to_string();
        self.update_manifest(move |docs| {
            docs.insert(doc_id.clone(), meta.clone());
        })
        .await
    }

    /// Remove a document's files and its manifest entry.
    pub async fn remove_document(&self, doc_id: &str) -> Result<()> {
        if !pi_store::store::is_safe_id(doc_id) {
            return Err(StorageError::Path(
                doc_id.into(),
                "unsafe document id".into(),
            ));
        }
        let id = doc_id.to_string();
        self.update_manifest(move |docs| {
            docs.shift_remove(&id);
        })
        .await?;
        for name in [DOC_FILE, TREE_FILE, PAGES_FILE] {
            let key = join(&self.prefix, &format!("{DOCS_DIR}/{doc_id}/{name}"))?;
            match self.store.delete(&key).await {
                Ok(()) | Err(object_store::Error::NotFound { .. }) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// The remote manifest's `docs` map (empty when absent).
    pub async fn manifest(&self) -> Result<Map<String, Value>> {
        Ok(self.read_manifest().await?.0)
    }

    async fn read_manifest(&self) -> Result<(Map<String, Value>, Option<UpdateVersion>)> {
        let key = join(&self.prefix, MANIFEST_FILE)?;
        match self.store.get(&key).await {
            Ok(result) => {
                let version = UpdateVersion {
                    e_tag: result.meta.e_tag.clone(),
                    version: result.meta.version.clone(),
                };
                let bytes = result.bytes().await?;
                // Same tolerance as the local store: an unreadable manifest is a cache miss.
                let docs = match serde_json::from_slice::<Value>(&bytes) {
                    Ok(Value::Object(mut m)) => match m.remove("docs") {
                        Some(Value::Object(d)) => d,
                        _ => Map::new(),
                    },
                    _ => Map::new(),
                };
                Ok((docs, Some(version)))
            }
            Err(object_store::Error::NotFound { .. }) => Ok((Map::new(), None)),
            Err(e) => Err(e.into()),
        }
    }

    /// Read-modify-write the manifest with a conditional put, re-reading and re-applying
    /// `edit` on every conflict.
    async fn update_manifest(&self, edit: impl Fn(&mut Map<String, Value>)) -> Result<()> {
        let key = join(&self.prefix, MANIFEST_FILE)?;
        for attempt in 0..MANIFEST_ATTEMPTS {
            let (mut docs, version) = self.read_manifest().await?;
            edit(&mut docs);
            let mut wrapper = Map::new();
            wrapper.insert("docs".into(), Value::Object(docs));
            let payload = PutPayload::from(pyjson::dumps(&Value::Object(wrapper)).into_bytes());
            let mode = match version {
                Some(v) => PutMode::Update(v),
                None => PutMode::Create,
            };
            let opts = PutOptions {
                mode: mode.clone(),
                ..Default::default()
            };
            match self.store.put_opts(&key, payload.clone(), opts).await {
                Ok(_) => return Ok(()),
                Err(object_store::Error::Precondition { .. })
                | Err(object_store::Error::AlreadyExists { .. }) => {
                    let backoff = 10u64 << attempt.min(6);
                    tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
                }
                // Stores without conditional updates (the local filesystem): overwrite.
                Err(object_store::Error::NotImplemented { .. })
                    if matches!(mode, PutMode::Update(_)) =>
                {
                    self.store.put(&key, payload).await?;
                    return Ok(());
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(StorageError::Conflict(MANIFEST_ATTEMPTS))
    }
}
