//! The storage half of `pageindex/local_api.py` (`LocalAPI`): building and committing a
//! document, and the read surface the agent tools use.

use std::collections::HashSet;
use std::fmt;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::consts::{
    DOC_ID_PREFIX, GET_DOCUMENT_KEYS, LIST_LIMIT_MAX, MAX_NAME_BYTES, MAX_NAME_SUFFIX,
};
use crate::naming::truncate_filename;
use crate::store::{DocStore, Meta};

/// Errors of the local API. `Api` carries the exact `PageIndexAPIError` message; `Value`
/// is a Python `ValueError`; `Io` an `OSError` escaping the store.
#[derive(Debug)]
pub enum ApiError {
    Api(String),
    Value(String),
    Io(io::Error),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Api(m) | ApiError::Value(m) => f.write_str(m),
            ApiError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<io::Error> for ApiError {
    fn from(e: io::Error) -> Self {
        ApiError::Io(e)
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// `"pi-" + uuid.uuid4().hex`. // ref: local_api.py:154
pub fn new_doc_id() -> String {
    format!("{DOC_ID_PREFIX}{}", uuid::Uuid::new_v4().simple())
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `datetime.isoformat()` of a naive UTC time with millisecond precision (the fraction is
/// omitted when it is zero, as Python does).
pub fn iso_from_unix_ms(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let milli = ms.rem_euclid(1000);
    let (y, mo, d) = civil_from_days(secs.div_euclid(86_400));
    let sod = secs.rem_euclid(86_400);
    let base = format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}",
        sod / 3600,
        sod / 60 % 60,
        sod % 60
    );
    if milli == 0 {
        base
    } else {
        format!("{base}.{:06}", milli * 1000)
    }
}

/// `_now_iso`: naive UTC, millisecond precision. // ref: local_api.py:29
pub fn now_iso() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    iso_from_unix_ms(ms)
}

/// The doc.json record, keys in the reference's order. // ref: local_api.py:161-171
pub fn doc_meta(
    doc_id: &str,
    name: &str,
    description: Option<&str>,
    created_at: &str,
    page_num: usize,
    metadata: Option<Map<String, Value>>,
    mode: &str,
) -> Meta {
    let mut m = Map::new();
    m.insert("id".into(), json!(doc_id));
    m.insert("name".into(), json!(name));
    m.insert("description".into(), json!(description));
    m.insert("status".into(), json!("completed"));
    m.insert("createdAt".into(), json!(created_at));
    m.insert("pageNum".into(), json!(page_num));
    m.insert("folderId".into(), Value::Null);
    m.insert(
        "metadata".into(),
        metadata.map(Value::Object).unwrap_or(Value::Null),
    );
    m.insert("mode".into(), json!(mode));
    m
}

/// pages.json: `[{"page_index": i + 1, "markdown": text}]`. // ref: local_api.py:155
pub fn pages_record<S: AsRef<str>>(page_texts: &[S]) -> Value {
    Value::Array(
        page_texts
            .iter()
            .enumerate()
            .map(|(i, t)| json!({"page_index": i + 1, "markdown": t.as_ref()}))
            .collect(),
    )
}

/// `utils.remove_fields(data, fields)` (without `max_len`). // ref: utils.py:621
pub fn remove_fields(data: &Value, fields: &[&str]) -> Value {
    match data {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(k, _)| !fields.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), remove_fields(v, fields)))
                .collect(),
        ),
        Value::Array(items) => {
            Value::Array(items.iter().map(|v| remove_fields(v, fields)).collect())
        }
        other => other.clone(),
    }
}

/// `_check_page_bounds`: every node span must lie inside 1..=page_count.
/// // ref: local_api.py:192
pub fn check_page_bounds(structure: &Value, page_count: usize) -> ApiResult<()> {
    let mut stack: Vec<&Value> = match structure {
        Value::Array(items) => items.iter().collect(),
        _ => Vec::new(),
    };
    while let Some(node) = stack.pop() {
        let (start, end) = (node.get("start_index"), node.get("end_index"));
        if let (Some(s), Some(e)) = (start, end)
            && !s.is_null()
            && !e.is_null()
        {
            let (sf, ef) = (
                s.as_f64().unwrap_or(f64::NAN),
                e.as_f64().unwrap_or(f64::NAN),
            );
            if !(1.0 <= sf && ef <= page_count as f64) {
                return Err(ApiError::Api(format!(
                    "Failed to submit document: the extracted structure references pages \
                     {}-{} outside the PDF's {page_count} readable pages.",
                    crate::pyjson::py_str(s),
                    crate::pyjson::py_str(e)
                )));
            }
        }
        if let Some(Value::Array(children)) = node.get("nodes") {
            stack.extend(children.iter());
        }
    }
    Ok(())
}

fn get_text_of_pdf_pages(pages: &[String], start: &Value, end: &Value) -> String {
    let (Some(s), Some(e)) = (start.as_i64(), end.as_i64()) else {
        return String::new();
    };
    let mut text = String::new();
    for page in (s - 1)..e {
        // Python indexing: negative indices count from the end.
        let idx = if page < 0 {
            pages.len() as i64 + page
        } else {
            page
        };
        if let Some(t) = usize::try_from(idx).ok().and_then(|i| pages.get(i)) {
            text.push_str(t);
        }
    }
    text
}

/// `utils.add_node_text`. // ref: utils.py:709
pub fn add_node_text(node: &mut Value, pages: &[String]) {
    match node {
        Value::Object(m) => {
            let start = m.get("start_index").cloned().unwrap_or(Value::Null);
            let end = m.get("end_index").cloned().unwrap_or(Value::Null);
            m.insert(
                "text".into(),
                Value::String(get_text_of_pdf_pages(pages, &start, &end)),
            );
            if let Some(children) = m.get_mut("nodes") {
                add_node_text(children, pages);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| add_node_text(v, pages)),
        _ => {}
    }
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(_) => true,
    }
}

/// `_format_tree_node`: the cloud `get_tree` wire shape. // ref: local_api.py:396
pub fn format_tree_node(node: &Value, node_summary: bool) -> Value {
    let empty = Vec::new();
    let children = match node.get("nodes") {
        Some(Value::Array(c)) => c,
        _ => &empty,
    };
    let mut out = Map::new();
    out.insert(
        "title".into(),
        node.get("title").cloned().unwrap_or_else(|| json!("")),
    );
    out.insert(
        "node_id".into(),
        node.get("node_id").cloned().unwrap_or(Value::Null),
    );
    out.insert(
        "page_index".into(),
        node.get("start_index").cloned().unwrap_or(Value::Null),
    );
    if truthy(node.get("key_items")) {
        out.insert("key_items".into(), node["key_items"].clone());
    }
    if node_summary && let Some(summary) = node.get("summary").filter(|s| !s.is_null()) {
        let key = if children.is_empty() {
            "summary"
        } else {
            "prefix_summary"
        };
        out.insert(key.into(), summary.clone());
    }
    if let Some(text) = node.get("text") {
        out.insert("text".into(), text.clone());
    }
    if !children.is_empty() {
        out.insert(
            "nodes".into(),
            Value::Array(
                children
                    .iter()
                    .map(|c| format_tree_node(c, node_summary))
                    .collect(),
            ),
        );
    }
    Value::Object(out)
}

/// `LocalAPI`'s storage surface over one `DocStore`.
#[derive(Debug, Clone)]
pub struct LocalApi {
    store: DocStore,
}

impl LocalApi {
    pub fn new(storage_path: impl AsRef<Path>) -> Self {
        LocalApi {
            store: DocStore::new(storage_path),
        }
    }

    pub fn store(&self) -> &DocStore {
        &self.store
    }

    /// `_unique_doc_name`: a taken name gets `_1`..`_99`. // ref: local_api.py:176
    pub fn unique_doc_name(&self, name: &str) -> ApiResult<String> {
        let metas = self.store.list_metas()?;
        let taken: HashSet<&str> = metas
            .iter()
            .filter_map(|m| m.get("name").and_then(Value::as_str))
            .collect();
        if !taken.contains(name) {
            return Ok(name.to_string());
        }
        for num in 1..=MAX_NAME_SUFFIX {
            let candidate = truncate_filename(name, MAX_NAME_BYTES, &format!("_{num}"))
                .map_err(ApiError::Value)?;
            if !taken.contains(candidate.as_str()) {
                return Ok(candidate);
            }
        }
        Err(ApiError::Api(
            "Failed to submit document: Too many files with similar names. Please use a \
             different file name."
                .into(),
        ))
    }

    /// The commit tail of `submit_document`: under the store lock, unique the name, build
    /// the meta, and save tree (minus node `text`) and pages. Returns `{doc_id, name}`.
    /// // ref: local_api.py:152-174
    pub fn commit_document(&self, doc: NewDocument<'_>) -> ApiResult<Value> {
        check_page_bounds(doc.structure, doc.page_texts.len())?;
        let doc_id = new_doc_id();
        let pages = pages_record(doc.page_texts);
        let _lock = self.store.lock()?;
        let name = self.unique_doc_name(doc.name)?;
        let meta = doc_meta(
            &doc_id,
            &name,
            doc.description,
            &now_iso(),
            doc.page_texts.len(),
            doc.metadata,
            doc.mode,
        );
        self.store.save_document(
            &doc_id,
            &meta,
            &remove_fields(doc.structure, &["text"]),
            &pages,
        )?;
        Ok(json!({"doc_id": doc_id, "name": name}))
    }

    fn require_doc(&self, doc_id: &str, prefix: &str) -> ApiResult<Meta> {
        self.store
            .get_meta(doc_id)?
            .ok_or_else(|| ApiError::Api(format!("{prefix}: Document not found.")))
    }

    fn require_data(data: Option<Value>, prefix: &str) -> ApiResult<Value> {
        data.ok_or_else(|| ApiError::Api(format!("{prefix}: stored document data is unreadable.")))
    }

    fn require_pages(&self, doc_id: &str, prefix: &str) -> ApiResult<Value> {
        let pages = Self::require_data(self.store.get_pages(doc_id)?, prefix)?;
        if !truthy(Some(&pages)) {
            return Err(ApiError::Api(format!(
                "{prefix}: stored document has no page content."
            )));
        }
        Ok(pages)
    }

    fn completed_envelope(doc_id: &str, result: Value, meta: &Meta) -> Value {
        json!({
            "doc_id": doc_id,
            "status": "completed",
            "retrieval_ready": true,
            "result": result,
            "metadata": meta.get("metadata").cloned().unwrap_or(Value::Null),
            "features": {},
        })
    }

    /// `raw_tree`: the stored tree verbatim. // ref: local_api.py:271
    pub fn raw_tree(&self, doc_id: &str) -> ApiResult<Option<Value>> {
        Ok(self.store.get_tree(doc_id)?)
    }

    /// `get_tree`. // ref: local_api.py:276
    pub fn get_tree(
        &self,
        doc_id: &str,
        node_summary: bool,
        include_text: bool,
    ) -> ApiResult<Value> {
        const P: &str = "Failed to get tree result";
        let meta = self.require_doc(doc_id, P)?;
        let mut structure = Self::require_data(self.store.get_tree(doc_id)?, P)?;
        if include_text {
            let pages = self.require_pages(doc_id, P)?;
            let texts = page_markdowns(&pages);
            add_node_text(&mut structure, &texts);
        }
        let result = match &structure {
            Value::Array(nodes) => nodes
                .iter()
                .map(|n| format_tree_node(n, node_summary))
                .collect(),
            _ => Vec::new(),
        };
        Ok(Self::completed_envelope(
            doc_id,
            Value::Array(result),
            &meta,
        ))
    }

    /// `get_ocr(format)`: `"page"` (stored pages), `"node"` (flattened tree with text) or
    /// `"raw"` (markdown joined by blank lines). // ref: local_api.py:287
    pub fn get_ocr(&self, doc_id: &str, format: &str) -> ApiResult<Value> {
        const P: &str = "Failed to get OCR result";
        if !matches!(format, "page" | "node" | "raw") {
            return Err(ApiError::Value(
                "Format parameter must be 'page', 'node', or 'raw'".into(),
            ));
        }
        let meta = self.require_doc(doc_id, P)?;
        let result = if format == "node" {
            let mut structure = Self::require_data(self.store.get_tree(doc_id)?, P)?;
            let pages = self.require_pages(doc_id, P)?;
            add_node_text(&mut structure, &page_markdowns(&pages));
            let mut out = Vec::new();
            flatten_nodes(&structure, 1, &mut out);
            Value::Array(out)
        } else {
            let pages = self.require_pages(doc_id, P)?;
            if format == "page" {
                pages
            } else {
                Value::String(page_markdowns(&pages).join("\n\n"))
            }
        };
        Ok(Self::completed_envelope(doc_id, result, &meta))
    }

    /// `PageIndexClient.get_document_id`: the newest document with that name (a folder
    /// prefix is ignored). // ref: client.py:1886
    pub fn get_document_id(&self, name: &str) -> ApiResult<String> {
        let name = name.rsplit('/').next().unwrap_or(name);
        let listing = self.list_documents(1, 0, Some(name))?;
        match listing["documents"].get(0).and_then(|d| d.get("id")) {
            Some(Value::String(id)) => Ok(id.clone()),
            _ => Err(ApiError::Api(format!(
                "No document named {} found.",
                crate::pyjson::str_repr(name)
            ))),
        }
    }

    /// `get_document`. // ref: local_api.py:343
    pub fn get_document(&self, doc_id: &str) -> ApiResult<Value> {
        let meta = self.store.get_meta(doc_id)?.ok_or_else(|| {
            ApiError::Api("Failed to get document metadata: Document not found".into())
        })?;
        Ok(Value::Object(
            GET_DOCUMENT_KEYS
                .iter()
                .map(|k| (k.to_string(), meta.get(*k).cloned().unwrap_or(Value::Null)))
                .collect(),
        ))
    }

    /// `delete_document`. // ref: local_api.py:351
    pub fn delete_document(&self, doc_id: &str) -> ApiResult<Value> {
        if !self.store.delete_document(doc_id)? {
            return Err(ApiError::Api(
                "Failed to delete document: Document not found.".into(),
            ));
        }
        Ok(json!({"message": "Document deleted successfully."}))
    }

    /// `list_documents` (no folders locally): newest first, ties by id.
    /// // ref: local_api.py:356
    pub fn list_documents(&self, limit: i64, offset: i64, name: Option<&str>) -> ApiResult<Value> {
        if !(1..=LIST_LIMIT_MAX).contains(&limit) {
            return Err(ApiError::Value("limit must be between 1 and 10000".into()));
        }
        if offset < 0 {
            return Err(ApiError::Value("offset must be non-negative".into()));
        }
        let key = |m: &Meta, k: &str| -> String {
            match m.get(k) {
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            }
        };
        let mut metas = self.store.list_metas()?;
        metas.sort_by_key(|m| key(m, "id"));
        // Stable descending sort, as `list.sort(reverse=True)` (ties keep their order).
        metas.sort_by_key(|m| std::cmp::Reverse(key(m, "createdAt")));
        if let Some(name) = name {
            metas.retain(|m| m.get("name").and_then(Value::as_str) == Some(name));
        }
        let total = metas.len();
        let start = usize::try_from(offset).unwrap_or(usize::MAX).min(total);
        let end = start.saturating_add(limit as usize).min(total);
        let get = |m: &Meta, k: &str| m.get(k).cloned().unwrap_or(Value::Null);
        let documents: Vec<Value> = metas[start..end]
            .iter()
            .map(|m| {
                json!({
                    "id": get(m, "id"),
                    "name": get(m, "name"),
                    "description": get(m, "description"),
                    "status": get(m, "status"),
                    "createdAt": get(m, "createdAt"),
                    "pageNum": m.get("pageNum").cloned().unwrap_or(json!(0)),
                    "folderId": null,
                    "path": null,
                    "metadata": get(m, "metadata"),
                    "features": {},
                })
            })
            .collect();
        Ok(json!({
            "documents": documents,
            "total": total,
            "limit": limit,
            "offset": offset,
        }))
    }
}

/// Inputs to [`LocalApi::commit_document`].
#[derive(Debug)]
pub struct NewDocument<'a> {
    /// Sanitized upload name (see [`crate::naming::sanitize_filename`]).
    pub name: &'a str,
    pub description: Option<&'a str>,
    /// Tree with node ids; node `text` is stripped before saving.
    pub structure: &'a Value,
    pub page_texts: &'a [String],
    pub metadata: Option<Map<String, Value>>,
    /// "flash" or "standard".
    pub mode: &'a str,
}

/// `get_ocr(format="node")`'s walk: `{title, level, page_index, text}` per node, preorder.
fn flatten_nodes(nodes: &Value, level: u64, out: &mut Vec<Value>) {
    let Value::Array(items) = nodes else { return };
    for node in items {
        out.push(json!({
            "title": node.get("title").cloned().unwrap_or_else(|| json!("")),
            "level": level,
            "page_index": node.get("start_index").cloned().unwrap_or(Value::Null),
            "text": node.get("text").cloned().unwrap_or_else(|| json!("")),
        }));
        if let Some(children) = node.get("nodes").filter(|c| truthy(Some(c))) {
            flatten_nodes(children, level + 1, out);
        }
    }
}

/// `[p.get("markdown", "") for p in pages]`.
fn page_markdowns(pages: &Value) -> Vec<String> {
    match pages {
        Value::Array(items) => items
            .iter()
            .map(|p| match p.get("markdown") {
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_matches_python_isoformat() {
        assert_eq!(iso_from_unix_ms(0), "1970-01-01T00:00:00");
        assert_eq!(
            iso_from_unix_ms(1_785_578_400_123),
            "2026-08-01T10:00:00.123000"
        );
    }
}
