//! Constants derived from the reference (VectifyAI/PageIndex @619cbd8).

/// Manifest file name under the store root. // ref: pageindex/local_store.py:77
pub const MANIFEST_FILE: &str = "manifest.json";
/// Lock file name under the store root (held with `flock(LOCK_EX)`). // ref: pageindex/local_store.py:107
pub const LOCK_FILE: &str = ".lock";
/// Per-document directory parent. // ref: pageindex/local_store.py:76
pub const DOCS_DIR: &str = "docs";
/// Per-document files. // ref: pageindex/local_store.py:120-122
pub const TREE_FILE: &str = "tree.json";
pub const PAGES_FILE: &str = "pages.json";
pub const DOC_FILE: &str = "doc.json";
/// Document ids are `"pi-" + uuid4().hex`. // ref: pageindex/local_api.py:154
pub const DOC_ID_PREFIX: &str = "pi-";

/// Upload-name byte budget. // ref: pageindex/naming.py:8
pub const MAX_NAME_BYTES: usize = 180;
/// Collision suffixes `_1` .. `_99`. // ref: pageindex/local_api.py:182
pub const MAX_NAME_SUFFIX: u32 = 99;

/// `list_documents` limit bounds. // ref: pageindex/local_api.py:364
pub const LIST_LIMIT_MAX: i64 = 10_000;

/// Key order of `get_document`. // ref: pageindex/local_api.py:347-349
pub const GET_DOCUMENT_KEYS: [&str; 8] = [
    "id",
    "name",
    "description",
    "status",
    "createdAt",
    "pageNum",
    "folderId",
    "metadata",
];
