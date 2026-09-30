//! Source and Mirror against object_store's in-memory and local-filesystem backends
//! (no network).

use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use futures::stream::BoxStream;
use object_store::memory::InMemory;
use object_store::path::Path as ObjectPath;
use object_store::{
    CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore,
    ObjectStoreExt, PutMode, PutMultipartOptions, PutOptions, PutPayload, PutResult,
};
use serde_json::json;

use pi_storage::{LocalSource, Mirror, ObjectSource, Source};
use pi_store::{LocalApi, NewDocument};

fn commit(api: &LocalApi, name: &str) -> String {
    let texts = vec![format!("text of {name}")];
    let tree = json!([{"title": name, "node_id": "0000", "start_index": 1, "end_index": 1,
                       "text": "dropped"}]);
    api.commit_document(NewDocument {
        name,
        description: Some("d"),
        structure: &tree,
        page_texts: &texts,
        metadata: None,
        mode: "flash",
    })
    .unwrap()["doc_id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn remote_bytes(store: &dyn ObjectStore, key: &str) -> Vec<u8> {
    store
        .get(&ObjectPath::from(key))
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap()
        .to_vec()
}

#[tokio::test]
async fn mirror_uploads_same_layout_and_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(".pageindex");
    let api = LocalApi::new(&root);
    let store = Arc::new(InMemory::new());
    let mirror = Mirror::new(store.clone(), "team/idx/").unwrap();
    let mut ids = Vec::new();
    for name in ["a.pdf", "b.pdf"] {
        let id = commit(&api, name);
        mirror.push_document(&root, &id).await.unwrap();
        ids.push(id);
    }
    for id in &ids {
        for file in ["tree.json", "pages.json", "doc.json"] {
            let local = std::fs::read(root.join("docs").join(id).join(file)).unwrap();
            let remote = remote_bytes(store.as_ref(), &format!("team/idx/docs/{id}/{file}")).await;
            assert_eq!(local, remote, "{id}/{file}");
        }
    }
    // Same commit order, same merge: the remote manifest is byte-identical to the local one.
    let local_manifest = std::fs::read(root.join("manifest.json")).unwrap();
    assert_eq!(
        local_manifest,
        remote_bytes(store.as_ref(), "team/idx/manifest.json").await
    );

    mirror.remove_document(&ids[0]).await.unwrap();
    let docs = mirror.manifest().await.unwrap();
    assert_eq!(docs.keys().collect::<Vec<_>>(), vec![&ids[1]]);
    assert!(
        store
            .head(&ObjectPath::from(format!(
                "team/idx/docs/{}/doc.json",
                ids[0]
            )))
            .await
            .is_err()
    );
    assert!(mirror.push_document(&root, "../escape").await.is_err());
}

/// Delegates to an in-memory store, but slips a competing manifest write in front of the
/// first conditional manifest put — the race a second indexer would cause.
#[derive(Debug)]
struct Racing {
    inner: InMemory,
    raced: AtomicBool,
}

impl fmt::Display for Racing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Racing({})", self.inner)
    }
}

#[async_trait]
impl ObjectStore for Racing {
    async fn put_opts(
        &self,
        location: &ObjectPath,
        payload: PutPayload,
        opts: PutOptions,
    ) -> object_store::Result<PutResult> {
        if location.as_ref().ends_with("manifest.json")
            && !matches!(opts.mode, PutMode::Overwrite)
            && !self.raced.swap(true, Ordering::SeqCst)
        {
            let theirs = json!({"docs": {"pi-theirs": {"id": "pi-theirs", "name": "theirs.pdf"}}});
            self.inner
                .put(location, PutPayload::from(theirs.to_string().into_bytes()))
                .await?;
        }
        self.inner.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &ObjectPath,
        opts: PutMultipartOptions,
    ) -> object_store::Result<Box<dyn MultipartUpload>> {
        self.inner.put_multipart_opts(location, opts).await
    }

    async fn get_opts(
        &self,
        location: &ObjectPath,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        self.inner.get_opts(location, options).await
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, object_store::Result<ObjectPath>>,
    ) -> BoxStream<'static, object_store::Result<ObjectPath>> {
        self.inner.delete_stream(locations)
    }

    fn list(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
        self.inner.list(prefix)
    }

    async fn list_with_delimiter(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> object_store::Result<ListResult> {
        self.inner.list_with_delimiter(prefix).await
    }

    async fn copy_opts(
        &self,
        from: &ObjectPath,
        to: &ObjectPath,
        options: CopyOptions,
    ) -> object_store::Result<()> {
        self.inner.copy_opts(from, to, options).await
    }
}

#[tokio::test]
async fn manifest_conflict_is_merged_not_overwritten() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(".pageindex");
    let api = LocalApi::new(&root);
    let store = Arc::new(Racing {
        inner: InMemory::new(),
        raced: AtomicBool::new(false),
    });
    let mirror = Mirror::new(store.clone(), "").unwrap();
    let id = commit(&api, "ours.pdf");
    mirror.push_document(&root, &id).await.unwrap();
    assert!(store.raced.load(Ordering::SeqCst));
    let docs = mirror.manifest().await.unwrap();
    assert_eq!(
        docs.keys().collect::<Vec<_>>(),
        vec!["pi-theirs", id.as_str()]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_mirrors_keep_every_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(".pageindex");
    let api = LocalApi::new(&root);
    let ids: Vec<String> = (0..24)
        .map(|i| commit(&api, &format!("d{i}.pdf")))
        .collect();
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let mut tasks = Vec::new();
    for chunk in ids.chunks(6) {
        let mirror = Mirror::new(store.clone(), "p").unwrap();
        let root = root.clone();
        let chunk = chunk.to_vec();
        tasks.push(tokio::spawn(async move {
            for id in chunk {
                mirror.push_document(&root, &id).await.unwrap();
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    let docs = Mirror::new(store, "p").unwrap().manifest().await.unwrap();
    let mut got: Vec<&String> = docs.keys().collect();
    got.sort();
    let mut want: Vec<&String> = ids.iter().collect();
    want.sort();
    assert_eq!(got, want);
}

#[tokio::test]
async fn mirror_on_local_filesystem() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(".pageindex");
    let remote_dir = tmp.path().join("remote");
    std::fs::create_dir_all(&remote_dir).unwrap();
    let store =
        Arc::new(object_store::local::LocalFileSystem::new_with_prefix(&remote_dir).unwrap());
    let mirror = Mirror::new(store, "mirror").unwrap();
    let api = LocalApi::new(&root);
    let a = commit(&api, "a.pdf");
    mirror.push_document(&root, &a).await.unwrap(); // PutMode::Create
    let b = commit(&api, "b.pdf");
    mirror.push_document(&root, &b).await.unwrap(); // Update unsupported -> overwrite
    // The mirrored directory is itself a readable store.
    let mirrored = LocalApi::new(remote_dir.join("mirror"));
    let listing = mirrored.list_documents(10, 0, None).unwrap();
    assert_eq!(listing["total"], 2);
    assert_eq!(
        std::fs::read(root.join("manifest.json")).unwrap(),
        std::fs::read(remote_dir.join("mirror/manifest.json")).unwrap()
    );
}

#[tokio::test]
async fn object_source_streams_to_a_temp_file() {
    let store = Arc::new(InMemory::new());
    for (key, body) in [
        ("in/b.pdf", "B"),
        ("in/sub/a.PDF", "A"),
        ("in/notes.txt", "x"),
        ("other/c.pdf", "C"),
    ] {
        store
            .put(
                &ObjectPath::from(key),
                PutPayload::from(body.as_bytes().to_vec()),
            )
            .await
            .unwrap();
    }
    let source = ObjectSource::new(store, "in/").unwrap();
    let entries = source.list().await.unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["b.pdf", "a.PDF"]);
    let fetched = source.fetch(&entries[1]).await.unwrap();
    let path = fetched.path().to_path_buf();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "A");
    drop(fetched);
    assert!(!path.exists(), "remote sources leave no local copy");
}

#[tokio::test]
async fn local_source_lists_pdfs_recursively() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::create_dir_all(dir.join("x/y")).unwrap();
    for f in ["z.pdf", "x/y/a.pdf", "x/readme.md"] {
        std::fs::write(dir.join(f), "%PDF").unwrap();
    }
    let source = LocalSource::new(dir);
    let entries = source.list().await.unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["a.pdf", "z.pdf"]);
    let fetched = source.fetch(&entries[0]).await.unwrap();
    assert!(fetched.path().is_file());
    let path = fetched.path().to_path_buf();
    drop(fetched);
    assert!(path.exists(), "local sources are never deleted");
    let single = LocalSource::new(dir.join("z.pdf")).list().await.unwrap();
    assert_eq!(single.len(), 1);
    assert!(
        LocalSource::new(Path::new("/nonexistent/x"))
            .list()
            .await
            .is_err()
    );
}
