//! The contract tools over a local store: port of the tool implementations and
//! `call_tool` in `pageindex/agent_tools.py`.
//!
//! Tools never fail at the transport level: every outcome, including argument errors and
//! unexpected store errors, is a JSON envelope (`{"success": true, ...}` or
//! `{"error", "errorCode"?, ..., "next_steps"}`) plus an `is_error` flag.

use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::consts::{
    ALL_DOCUMENTS_PAGE, ALL_TOOLS, CHAR_BUDGET, MAX_REMOVE, MAX_REQUESTED_PAGES,
    SIMILAR_NAMES_CUTOFF, SIMILAR_NAMES_LIMIT, STRUCTURE_FIRST_PAGE_THRESHOLD,
    TOOL_WAIT_INTERVAL_S, TOOL_WAIT_TIMEOUT_S, tool_contract,
};
use crate::pages::{Page, expand_pages, format_page_spec};
use crate::pyval::{normalize_created_at, py_int, truthy};
use crate::structure::{format_structure, split_structure};
use pi_store::pyjson::{self, py_str, py_type_name, serialized_len, str_repr};
use pi_store::{ApiError, LocalApi};

type Obj = Map<String, Value>;
/// `(payload, is_error)`.
type ToolResult = (Value, bool);
/// `Err` is an exception escaping the implementation (`call_tool` wraps it).
type Outcome = Result<ToolResult, String>;

// ── envelopes ── // ref: agent_tools.py:315-332

fn success(data: Obj, next_steps: Value) -> ToolResult {
    let mut payload = Obj::new();
    payload.insert("success".into(), Value::Bool(true));
    payload.extend(data);
    payload.insert("next_steps".into(), next_steps);
    (Value::Object(payload), false)
}

fn failure(error: &str, details: Option<Obj>, next_steps: Value, code: Option<&str>) -> ToolResult {
    let mut payload = Obj::new();
    payload.insert("error".into(), json!(error));
    if let Some(code) = code.filter(|c| !c.is_empty()) {
        payload.insert("errorCode".into(), json!(code));
    }
    if let Some(details) = details.filter(|d| !d.is_empty()) {
        payload.extend(details);
    }
    payload.insert("next_steps".into(), next_steps);
    (Value::Object(payload), true)
}

fn obj(value: Value) -> Obj {
    match value {
        Value::Object(m) => m,
        _ => Obj::new(),
    }
}

fn doc_name_details(doc_name: &Value) -> Option<Obj> {
    Some(obj(json!({"doc_name": doc_name})))
}

fn internal(e: ApiError) -> String {
    e.to_string()
}

// ── listing and name resolution ──

/// `_all_documents`: every listed document, newest first. // ref: agent_tools.py:337
fn all_documents(api: &LocalApi) -> Result<Vec<Obj>, String> {
    let mut documents = Vec::new();
    let mut offset = 0i64;
    loop {
        let page = api
            .list_documents(ALL_DOCUMENTS_PAGE, offset, None)
            .map_err(internal)?;
        let batch: Vec<Obj> = match page.get("documents") {
            Some(Value::Array(items)) => items.iter().cloned().map(obj).collect(),
            _ => Vec::new(),
        };
        offset += batch.len() as i64;
        let empty = batch.is_empty();
        documents.extend(batch);
        let total = page.get("total").and_then(Value::as_i64);
        if empty || total.is_some_and(|t| offset >= t) {
            return Ok(documents);
        }
    }
}

fn scope_documents(documents: Vec<Obj>, allowed: Option<&HashSet<String>>) -> Vec<Obj> {
    match allowed {
        None => documents,
        Some(ids) => documents
            .into_iter()
            .filter(|d| {
                d.get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| ids.contains(id))
            })
            .collect(),
    }
}

fn created_at_key(doc: &Obj) -> String {
    match doc.get("createdAt") {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        _ => String::new(),
    }
}

/// `_resolve_document`: same-name duplicates resolve to the newest; a miss suggests close
/// names. // ref: agent_tools.py:396
fn resolve_document(
    api: &LocalApi,
    doc_name: &Value,
    documents: Option<&[Obj]>,
    allowed: Option<&HashSet<String>>,
) -> Result<Result<Obj, ToolResult>, String> {
    let documents = match documents {
        Some(d) => scope_documents(d.to_vec(), allowed),
        None => scope_documents(all_documents(api)?, allowed),
    };
    let mut best: Option<&Obj> = None;
    for doc in documents.iter().filter(|d| d.get("name") == Some(doc_name)) {
        if best.is_none_or(|b| created_at_key(doc) > created_at_key(b)) {
            best = Some(doc);
        }
    }
    if let Some(best) = best {
        return Ok(Ok(best.clone()));
    }
    let names: Vec<String> = documents
        .iter()
        .filter_map(|d| d.get("name").filter(|n| truthy(n)).map(py_str))
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let similar = pi_pycompat::difflib::get_close_matches(
        &py_str(doc_name),
        &refs,
        SIMILAR_NAMES_LIMIT,
        SIMILAR_NAMES_CUTOFF,
    );
    let message = if similar.is_empty() {
        "Document not found or you do not have access to it".to_string()
    } else {
        let quoted: Vec<String> = similar.iter().map(|n| format!("\"{n}\"")).collect();
        format!("Document not found. Did you mean: {}?", quoted.join(", "))
    };
    Ok(Err(failure(
        &message,
        Some(obj(json!({"doc_name": doc_name, "similar_files": similar}))),
        json!({
            "summary": "The requested document does not exist or is not accessible",
            "options": [
                "Verify the document name is correct",
                "Use browse_documents() to see your recent documents",
                "Check if the document was deleted",
            ],
        }),
        Some("NOT_FOUND"),
    )))
}

fn status_of(entry: &Obj) -> Option<&str> {
    entry.get("status").and_then(Value::as_str)
}

fn is_terminal(entry: &Obj) -> bool {
    matches!(status_of(entry), Some("completed" | "failed"))
}

/// `_await_completion`: re-poll a processing document for up to 3 minutes when asked.
/// Local documents are stored terminal, so the wait normally never engages.
/// // ref: agent_tools.py:439
fn await_completion(api: &LocalApi, entry: Obj, wait: bool) -> Result<Obj, String> {
    let doc_id = entry.get("id").filter(|v| truthy(v)).map(py_str);
    let Some(doc_id) = doc_id.filter(|_| wait && !is_terminal(&entry)) else {
        return Ok(entry);
    };
    let deadline = Instant::now() + Duration::from_secs_f64(TOOL_WAIT_TIMEOUT_S);
    let mut current = entry;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_secs_f64(TOOL_WAIT_INTERVAL_S));
        let mut refreshed = match api.get_document(&doc_id) {
            Ok(v) => obj(v),
            Err(ApiError::Api(_)) => continue,
            Err(e) => return Err(internal(e)),
        };
        if refreshed.get("metadata").is_none_or(Value::is_null) {
            let kept = current.get("metadata").cloned().unwrap_or(Value::Null);
            refreshed.insert("metadata".into(), kept);
        }
        current.extend(refreshed);
        if is_terminal(&current) {
            return Ok(current);
        }
    }
    Ok(current)
}

/// `_not_ready_error`. // ref: agent_tools.py:463
fn not_ready_error(
    doc_name: &Value,
    status: &Value,
    operation: &str,
    timed_out: bool,
) -> ToolResult {
    let status_s = py_str(status);
    if status.as_str() == Some("failed") {
        return failure(
            &format!("Document processing failed. Current status: {status_s}"),
            doc_name_details(doc_name),
            json!({
                "summary": "Document processing has failed",
                "options": [
                    "Index the document again with PageIndexClient.submit_document()",
                    "Use browse_documents() to work with other documents",
                ],
            }),
            Some("INVALID_INPUT"),
        );
    }
    if timed_out {
        return failure(
            &format!("Document is still processing. Current status: {status_s}"),
            doc_name_details(doc_name),
            json!({
                "summary": "Document processing timeout",
                "options": [
                    "Try again later when processing is complete",
                    "Check status with get_document()",
                ],
            }),
            Some("INVALID_INPUT"),
        );
    }
    failure(
        &format!("Document is not ready for {operation}. Current status: {status_s}"),
        doc_name_details(doc_name),
        json!({
            "summary": "Document is still processing",
            "options": [
                "Wait for document processing to complete",
                "Check status with browse_documents() or get_document()",
            ],
        }),
        Some("INVALID_INPUT"),
    )
}

/// `_folder_unsupported`. // ref: agent_tools.py:506
fn folder_unsupported(param: &str) -> ToolResult {
    failure(
        &format!("Folders are not supported in local mode yet — omit {param}."),
        None,
        json!({
            "summary": "This local library does not have folders yet",
            "options": [
                "Retry the call without a folder_id",
                "Use browse_documents() to list the library root",
                "Folders are available on PageIndex cloud (PageIndexCloudClient with an API key)",
            ],
        }),
        Some("INVALID_INPUT"),
    )
}

/// `folder_id not in (None, "root")` (None is already dropped from the arguments).
fn folder_rejected(kw: &Obj) -> bool {
    kw.get("folder_id")
        .is_some_and(|f| f.as_str() != Some("root"))
}

/// `_parse_page_spec`. // ref: agent_tools.py:583
fn parse_page_spec(pages: &Value, doc_name: &Value) -> Result<Vec<Page>, ToolResult> {
    expand_pages(pages).map_err(|e| match e.code {
        "too_many" => failure(
            &format!("Too many pages requested (over {MAX_REQUESTED_PAGES})"),
            doc_name_details(doc_name),
            json!({
                "summary": "The page specification spans too many pages",
                "options": [
                    "Request a narrower page range",
                    "The response holds only a few pages per call - page through with several smaller requests",
                ],
            }),
            Some("INVALID_INPUT"),
        ),
        "nonpositive" => failure(
            "Invalid page numbers. Page numbers must be positive integers",
            doc_name_details(doc_name),
            json!({
                "summary": "Invalid page numbers provided",
                "options": [
                    "Page numbers must be positive integers (>= 1)",
                    "Check the page specification format",
                ],
            }),
            Some("INVALID_INPUT"),
        ),
        _ => failure(
            "Invalid page specification format",
            doc_name_details(doc_name),
            json!({
                "summary": "Failed to parse the pages parameter",
                "options": [
                    "Use valid formats: \"5\", \"3,7,10\", \"5-10\", or \"1-3,7,9-12\"",
                    "Ensure page numbers are positive integers",
                ],
            }),
            Some("INVALID_INPUT"),
        ),
    })
}

/// `_flat_metadata`: string/number/boolean fields only, or None. // ref: agent_tools.py:380
fn flat_metadata(value: Option<&Value>) -> Option<Value> {
    let Some(Value::Object(m)) = value else {
        return None;
    };
    let flat: Obj = m
        .iter()
        .filter(|(_, v)| matches!(v, Value::String(_) | Value::Number(_) | Value::Bool(_)))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    (!flat.is_empty()).then_some(Value::Object(flat))
}

fn or_default(value: Option<&Value>, default: &str) -> Value {
    match value {
        Some(v) if truthy(v) => v.clone(),
        _ => json!(default),
    }
}

// ── tools ──

/// `_browse_documents`. // ref: agent_tools.py:723
fn browse_documents(api: &LocalApi, kw: &Obj, allowed: Option<&HashSet<String>>) -> Outcome {
    if kw
        .get("folder_id")
        .is_some_and(|f| f.as_str() != Some("root"))
    {
        return Ok(folder_unsupported("folder_id"));
    }
    let sort = kw.get("sort").cloned().unwrap_or_else(|| json!("time"));
    if !matches!(sort.as_str(), Some("time" | "relevance")) {
        return Ok(failure(
            "Invalid sort mode — only the default \"time\" sort is available in local mode.",
            None,
            json!({"summary": "Invalid sort mode",
                   "options": ["Use sort=\"time\" (newest first) or omit sort",
                               "Semantic ranking is available on PageIndex cloud (PageIndexCloudClient with an API key)"]}),
            Some("INVALID_INPUT"),
        ));
    }
    if sort.as_str() == Some("relevance") || kw.get("query").is_some_and(truthy) {
        return Ok(failure(
            "Relevance ranking is not supported in local mode yet — use the default time sort.",
            None,
            json!({"summary": "This local library does not have semantic ranking yet",
                   "options": ["Retry without sort/query and match the returned names and descriptions against the intent yourself",
                               "Page through the full library with `offset: next_offset`",
                               "Semantic ranking is available on PageIndex cloud (PageIndexCloudClient with an API key)"]}),
            Some("INVALID_INPUT"),
        ));
    }
    let offset = kw.get("offset").map_or(Some(0), py_int);
    let limit = kw.get("limit").map_or(Some(10), py_int);
    let (Some(offset), Some(limit)) = (offset, limit) else {
        return Ok(failure(
            "offset and limit must be numbers",
            None,
            json!({"summary": "Invalid pagination parameters",
                   "options": ["Pass integer offset and limit values"]}),
            Some("INVALID_INPUT"),
        ));
    };
    let offset = offset.max(0);
    let limit = limit.clamp(1, 50);

    let (window, total): (Vec<Obj>, Option<i128>) = match allowed {
        None => {
            let off = i64::try_from(offset).unwrap_or(i64::MAX);
            let listing = api
                .list_documents(limit as i64, off, None)
                .map_err(internal)?;
            let window = match listing.get("documents") {
                Some(Value::Array(items)) => items.iter().cloned().map(obj).collect(),
                _ => Vec::new(),
            };
            (
                window,
                listing.get("total").and_then(Value::as_i64).map(i128::from),
            )
        }
        Some(_) => {
            let scoped = scope_documents(all_documents(api)?, allowed);
            let total = scoped.len();
            let start = usize::try_from(offset).unwrap_or(usize::MAX).min(total);
            let end = start.saturating_add(limit as usize).min(total);
            (scoped[start..end].to_vec(), Some(total as i128))
        }
    };
    let window_end = offset + window.len() as i128;
    let has_more = !window.is_empty()
        && match total {
            Some(t) => window_end < t,
            None => window.len() as i128 == limit,
        };
    let next_offset = if has_more {
        json!(window_end as i64)
    } else {
        Value::Null
    };

    let (mut page_has_processing, mut page_has_failed) = (false, false);
    let mut items = Vec::new();
    for doc in &window {
        let status = or_default(doc.get("status"), "unknown");
        if status.as_str() == Some("failed") {
            page_has_failed = true;
        } else if status.as_str() != Some("completed") {
            page_has_processing = true;
        }
        let mut item = obj(json!({
            "name": or_default(doc.get("name"), "Unknown Document"),
            "description": or_default(doc.get("description"), "No description provided"),
            "status": status,
            "created_at": normalize_created_at(doc.get("createdAt")),
        }));
        if let Some(metadata) = flat_metadata(doc.get("metadata")) {
            item.insert("metadata".into(), metadata);
        }
        items.push(Value::Object(item));
    }

    let mut data = obj(json!({
        "documents": items,
        "sort": sort,
        "next_offset": next_offset,
        "has_more": has_more,
    }));
    if !kw.get("recursive").is_some_and(truthy) {
        data.insert("folders".into(), json!([]));
    }

    if items.is_empty() && offset == 0 {
        return Ok(success(
            data,
            json!({
                "summary": "Nothing to show",
                "options": ["Nothing here. Index documents with PageIndexClient.submit_document() to get started."],
                "auto_retry": "Index a document with PageIndexClient.submit_document() to get started",
            }),
        ));
    }

    let mut options: Vec<String> = Vec::new();
    if !items.is_empty() {
        options.push("Use get_document() with a document name to view details".into());
        options.push(format!(
            "Results returned ≠ correct results. Verify these documents match the user's actual \
             intent (topic, time period, document type) before proceeding.{} Do NOT use general \
             knowledge as a substitute.",
            if has_more {
                " If they do not match, page through the rest of the library."
            } else {
                ""
            }
        ));
    }
    if page_has_processing {
        options.push(
            "Some documents on this page are still processing. Use get_document() to check \
             individual status."
                .into(),
        );
    }
    if page_has_failed {
        options.push(
            "Some documents on this page failed processing. Use get_document() to see error \
             details."
                .into(),
        );
    }
    if has_more {
        options.push(
            "Use browse_documents() with `offset: next_offset` to load more documents".into(),
        );
    }
    let summary = if items.is_empty() {
        "Nothing to show".to_string()
    } else {
        format!(
            "Showing {} document(s){}",
            items.len(),
            if has_more { " (more available)" } else { "" }
        )
    };
    Ok(success(
        data,
        json!({"summary": summary, "options": options}),
    ))
}

/// `_get_document`. // ref: agent_tools.py:837
fn get_document(api: &LocalApi, kw: &Obj, allowed: Option<&HashSet<String>>) -> Outcome {
    if folder_rejected(kw) {
        return Ok(folder_unsupported("folder_id"));
    }
    let doc_name = &kw["doc_name"];
    let entry = match resolve_document(api, doc_name, None, allowed)? {
        Ok(entry) => entry,
        Err(error) => return Ok(error),
    };
    let wait = kw.get("wait_for_completion").is_some_and(truthy);
    let entry = await_completion(api, entry, wait)?;

    let status = or_default(entry.get("status"), "unknown");
    let is_processing = !matches!(status.as_str(), Some("completed" | "failed"));
    let is_ready = status.as_str() == Some("completed");
    let page_num_v = or_default_num(entry.get("pageNum"));
    let page_num = page_num_v.as_i64().unwrap_or(0);
    let name = or_default(entry.get("name"), "Unknown Document");
    let name_s = py_str(&name);

    let mut suggestions: Vec<String> = Vec::new();
    if is_processing {
        suggestions
            .push("Document is still processing. Processing status can be checked later.".into());
    } else if is_ready {
        suggestions.push("Document is ready for analysis.".into());
        if page_num > 0 {
            let structure =
                format!("First explore structure: get_document_structure(doc_name: \"{name_s}\")");
            if page_num <= 5 {
                suggestions.extend([
                    format!("This is a short document with {page_num} pages."),
                    structure,
                    format!("Then extract all content: get_page_content(doc_name: \"{name_s}\", pages: \"1-{page_num}\")"),
                ]);
            } else if page_num <= STRUCTURE_FIRST_PAGE_THRESHOLD {
                suggestions.extend([
                    format!("This document has {page_num} pages."),
                    structure,
                    format!("Then extract key pages: get_page_content(doc_name: \"{name_s}\", pages: \"1,5,10\")"),
                ]);
            } else {
                suggestions.extend([
                    format!("This is a large document with {page_num} pages."),
                    structure,
                    format!("Then target specific sections: get_page_content(doc_name: \"{name_s}\", pages: \"1-3\")"),
                ]);
            }
        }
    } else {
        suggestions.push(
            "Document processing failed. Index the document again with \
             PageIndexClient.submit_document()."
                .into(),
        );
    }

    let mut data = obj(json!({
        "name": name,
        "description": or_default(entry.get("description"), "No description provided"),
        "status": status,
        "created_at": normalize_created_at(entry.get("createdAt")),
        "page_count": if truthy(&page_num_v) { page_num_v.clone() } else { Value::Null },
        "folder_id": entry.get("folderId").cloned().unwrap_or(Value::Null),
    }));
    if let Some(metadata) = flat_metadata(entry.get("metadata")) {
        data.insert("metadata".into(), metadata);
    }
    let mut next_steps = obj(json!({
        "summary": if is_ready {
            "Document is ready for analysis and querying."
        } else if is_processing {
            "Document is still being processed."
        } else {
            "Document processing has failed."
        },
        "options": suggestions,
    }));
    if is_processing {
        next_steps.insert(
            "auto_retry".into(),
            json!("Document processing status can be monitored periodically"),
        );
    }
    Ok(success(data, Value::Object(next_steps)))
}

/// `entry.get("pageNum") or 0`.
fn or_default_num(value: Option<&Value>) -> Value {
    match value {
        Some(v) if truthy(v) => v.clone(),
        _ => json!(0),
    }
}

/// `_get_document_structure`. // ref: agent_tools.py:905
fn get_document_structure(api: &LocalApi, kw: &Obj, allowed: Option<&HashSet<String>>) -> Outcome {
    if folder_rejected(kw) {
        return Ok(folder_unsupported("folder_id"));
    }
    let doc_name = &kw["doc_name"];
    let entry = match resolve_document(api, doc_name, None, allowed)? {
        Ok(entry) => entry,
        Err(error) => return Ok(error),
    };
    let wait = kw.get("wait_for_completion").is_some_and(truthy);
    let waited = wait && !is_terminal(&entry);
    let entry = await_completion(api, entry, wait)?;
    if status_of(&entry) != Some("completed") {
        let status = entry.get("status").cloned().unwrap_or(Value::Null);
        return Ok(not_ready_error(
            doc_name,
            &status,
            "structure retrieval",
            waited && status_of(&entry) != Some("failed"),
        ));
    }

    let doc_id = entry.get("id").map(py_str).unwrap_or_default();
    let mut tree = api.raw_tree(&doc_id).map_err(internal)?;
    if tree.is_none() {
        // Fallback: the cloud-shaped tree without text (`client.get_tree(...,
        // include_text=False)`), whose own errors surface here.
        match api.get_tree(&doc_id, true, false) {
            Ok(envelope) => {
                tree = envelope.get("result").filter(|r| !r.is_null()).map(|r| {
                    if truthy(r) {
                        pi_store::api::remove_fields(r, &["text"])
                    } else {
                        r.clone()
                    }
                });
            }
            Err(ApiError::Api(message)) => {
                return Ok(failure(
                    &format!("Failed to retrieve document structure: {message}"),
                    doc_name_details(doc_name),
                    json!({
                        "summary": "Failed to retrieve document structure due to an error",
                        "options": [
                            "The document may not exist or is not accessible",
                            "Check if the document name is correct",
                            "Try again in a few moments",
                        ],
                    }),
                    Some("INTERNAL_ERROR"),
                ));
            }
            Err(e) => return Err(internal(e)),
        }
    }
    let Some(tree) = tree else {
        return Ok(failure(
            "Structure not available for this document",
            doc_name_details(doc_name),
            json!({
                "summary": "Structure not available for this document",
                "options": [
                    "The document may not have been processed correctly or structure extraction may have failed",
                    "Try processing the document again if possible",
                ],
            }),
            Some("INTERNAL_ERROR"),
        ));
    };

    let formatted = format_structure(&tree);
    let mut chunks = split_structure(&formatted, CHAR_BUDGET);
    let total_parts = chunks.len().max(1) as i128;
    let requested = kw.get("part").map_or(Some(1), py_int).unwrap_or(1);
    let current = requested.max(1).min(total_parts);

    if total_parts == 1 {
        return Ok(success(
            obj(json!({"doc_name": doc_name, "structure": chunks.swap_remove(0)})),
            json!({
                "summary": "Document structure retrieved successfully.",
                "options": ["Use get_page_content() to extract specific content from pages"],
            }),
        ));
    }
    let next_steps = if current < total_parts {
        json!({
            "summary": format!("Showing part {current} of {total_parts}."),
            "options": [
                format!("Request next part with part: {}", current + 1),
                format!("Jump to last part with part: {total_parts}"),
                "Proceed to get_page_content() for specific sections",
            ],
        })
    } else {
        json!({
            "summary": "All parts retrieved for current pagination.",
            "options": ["Use get_page_content() to extract specific content from pages"],
        })
    };
    let chunk = chunks.swap_remove((current - 1) as usize);
    Ok(success(
        obj(json!({
            "doc_name": doc_name,
            "total_parts": total_parts as i64,
            "structure": chunk,
            "pagination": {
                "part": current as i64,
                "total_parts": total_parts as i64,
                "has_more": current < total_parts,
            },
        })),
        next_steps,
    ))
}

/// A `page_index` key of `by_index`: its integer value and whether the first key object
/// stored for it was a bool (Python's `True == 1` share one dict slot).
#[derive(Clone, Copy)]
struct PageKey {
    value: i128,
    is_bool: bool,
}

impl PageKey {
    fn to_json(self) -> Value {
        if self.is_bool {
            json!(self.value != 0)
        } else {
            json!(self.value as i64)
        }
    }

    fn display(self) -> String {
        match (self.is_bool, self.value) {
            (true, 0) => "False".into(),
            (true, _) => "True".into(),
            (false, v) => v.to_string(),
        }
    }
}

fn page_gt(page: Page, max: i128) -> bool {
    i128::try_from(page).map_or(true, |p| p > max)
}

/// `_get_page_content`. // ref: agent_tools.py:1009
fn get_page_content(api: &LocalApi, kw: &Obj, allowed: Option<&HashSet<String>>) -> Outcome {
    if folder_rejected(kw) {
        return Ok(folder_unsupported("folder_id"));
    }
    let doc_name = &kw["doc_name"];
    let entry = match resolve_document(api, doc_name, None, allowed)? {
        Ok(entry) => entry,
        Err(error) => return Ok(error),
    };
    let wait = kw.get("wait_for_completion").is_some_and(truthy);
    let waited = wait && !is_terminal(&entry);
    let entry = await_completion(api, entry, wait)?;
    if status_of(&entry) != Some("completed") {
        let status = entry.get("status").cloned().unwrap_or(Value::Null);
        return Ok(not_ready_error(
            doc_name,
            &status,
            "page content retrieval",
            waited && status_of(&entry) != Some("failed"),
        ));
    }

    let requested = match parse_page_spec(&kw["pages"], doc_name) {
        Ok(pages) => pages,
        Err(error) => return Ok(error),
    };

    let doc_id = entry.get("id").map(py_str).unwrap_or_default();
    let ocr = match api.get_ocr(&doc_id, "page") {
        Ok(v) => v,
        Err(ApiError::Api(message)) => {
            return Ok(failure(
                &format!("Failed to retrieve page content: {message}"),
                doc_name_details(doc_name),
                json!({
                    "summary": "Unable to retrieve page content due to a service issue.",
                    "options": [
                        "Verify the document name is correct using browse_documents()",
                        "Check if the document processing is complete with get_document()",
                        "Ensure the requested page numbers are valid",
                    ],
                    "auto_retry": "This may be a temporary issue - you can try the request again",
                }),
                Some("INTERNAL_ERROR"),
            ));
        }
        Err(e) => return Err(internal(e)),
    };
    let page_data = ocr.get("result").cloned().unwrap_or(Value::Null);
    let items: Vec<&Value> = match &page_data {
        v if !truthy(v) => Vec::new(),
        Value::Array(items) => items.iter().collect(),
        // Iterating a dict yields its keys, a str its characters: never dicts.
        Value::Object(_) | Value::String(_) => Vec::new(),
        other => {
            return Err(format!("'{}' object is not iterable", py_type_name(other)));
        }
    };
    let mut by_index: BTreeMap<i128, (PageKey, &Obj)> = BTreeMap::new();
    for item in items {
        let Value::Object(map) = item else { continue };
        let key = match map.get("page_index") {
            Some(Value::Bool(b)) => PageKey {
                value: *b as i128,
                is_bool: true,
            },
            Some(Value::Number(n)) if !n.is_f64() => PageKey {
                value: n
                    .as_i64()
                    .map(i128::from)
                    .or_else(|| n.as_u64().map(i128::from))
                    .unwrap_or(0),
                is_bool: false,
            },
            _ => continue,
        };
        by_index
            .entry(key.value)
            .and_modify(|slot| slot.1 = map)
            .or_insert((key, map));
    }
    let max_page = by_index
        .last_key_value()
        .map(|(_, (k, _))| *k)
        .unwrap_or(PageKey {
            value: 0,
            is_bool: false,
        });

    let out_of_range: Vec<Page> = requested
        .iter()
        .copied()
        .filter(|&p| page_gt(p, max_page.value))
        .collect();
    let valid_pages: Vec<Page> = requested
        .iter()
        .copied()
        .filter(|&p| !page_gt(p, max_page.value))
        .collect();
    if !out_of_range.is_empty() && valid_pages.is_empty() {
        let spec = format_page_spec(&out_of_range);
        return Ok(failure(
            &format!(
                "All requested pages are out of range. Document has {} pages, but you \
                 requested pages: {spec}",
                max_page.display()
            ),
            Some(obj(json!({
                "doc_name": doc_name,
                "max_pages": max_page.to_json(),
                "requested_pages": spec,
            }))),
            json!({
                "summary": "All requested pages are out of range for this document",
                "options": [
                    format!("Request pages between 1 and {}", max_page.display()),
                    "Use get_document() to check document page count",
                ],
            }),
            Some("INVALID_INPUT"),
        ));
    }

    let mut content = Vec::new();
    let mut included: Vec<Page> = Vec::new();
    let mut remaining: Vec<Page> = Vec::new();
    let mut budget = CHAR_BUDGET as i64;
    for &page in &valid_pages {
        let item = by_index.get(&(page as i128)).map(|(_, m)| *m);
        let text = match item.and_then(|m| m.get("markdown")) {
            Some(Value::String(s)) => s.clone(),
            _ => format!("Page {page} content not available"),
        };
        let entry = json!({"page": page as i64, "text": text});
        let size = serialized_len(&entry) as i64 + 2; // +2: json ", " item separator
        if included.is_empty() || budget - size >= 0 {
            content.push(entry);
            included.push(page);
            budget -= size;
        } else {
            remaining.push(page);
        }
    }

    let mut options: Vec<String> = vec![
        "Use get_document_structure() to understand document organization".into(),
        "Request additional pages as needed".into(),
    ];
    if !remaining.is_empty() {
        options.insert(
            0,
            format!(
                "For remaining pages, request: {}",
                format_page_spec(&remaining)
            ),
        );
    }
    if !out_of_range.is_empty() {
        let m = max_page.display();
        options.insert(
            0,
            format!("Document has {m} pages total - request pages 1-{m}"),
        );
    }
    let summary = if !remaining.is_empty() || !out_of_range.is_empty() {
        let mut parts = vec![format!(
            "Retrieved {} of {} requested pages.",
            included.len(),
            requested.len()
        )];
        if !remaining.is_empty() {
            parts.push(format!(
                "Pages {} were omitted due to response size limits.",
                format_page_spec(&remaining)
            ));
        }
        if !out_of_range.is_empty() {
            parts.push(format!(
                "Pages {} were out of range.",
                format_page_spec(&out_of_range)
            ));
        }
        parts.join(" ")
    } else {
        format!(
            "Successfully retrieved content for {} page{}.",
            content.len(),
            if content.len() == 1 { "" } else { "s" }
        )
    };
    Ok(success(
        obj(json!({
            "doc_name": doc_name,
            "total_pages": max_page.to_json(),
            "requested_pages": format_page_spec(&requested),
            "returned_pages": format_page_spec(&included),
            "content": content,
        })),
        json!({"summary": summary, "options": options}),
    ))
}

/// `_remove_document`. // ref: agent_tools.py:1128
fn remove_document(api: &LocalApi, kw: &Obj, allowed: Option<&HashSet<String>>) -> Outcome {
    if folder_rejected(kw) {
        return Ok(folder_unsupported("folder_id"));
    }
    let names = match &kw["doc_names"] {
        Value::Array(items) if !items.is_empty() => items,
        _ => {
            return Ok(failure(
                "At least one document name is required",
                None,
                json!({"summary": "No document names provided",
                       "options": ["Pass doc_names as a non-empty array"]}),
                Some("INVALID_INPUT"),
            ));
        }
    };
    if !names.iter().all(|n| {
        n.as_str()
            .is_some_and(|s| !pi_pycompat::pystr::strip(s).is_empty())
    }) {
        return Ok(failure(
            "doc_names must be an array of non-empty document name strings",
            None,
            json!({"summary": "Invalid document names",
                   "options": ["Copy each name verbatim from a browse_documents() response"]}),
            Some("INVALID_INPUT"),
        ));
    }
    let mut seen = HashSet::new();
    let names: Vec<&Value> = names
        .iter()
        .filter(|n| seen.insert(n.as_str().unwrap_or_default().to_string()))
        .collect();
    if names.len() > MAX_REMOVE {
        return Ok(failure(
            "Maximum 10 documents can be deleted at once",
            None,
            json!({"summary": "Too many documents in one call",
                   "options": ["Delete at most 10 documents per call"]}),
            Some("INVALID_INPUT"),
        ));
    }
    let documents = all_documents(api)?;
    let mut results = Vec::new();
    for doc_name in &names {
        let entry = match resolve_document(api, doc_name, Some(&documents), allowed)? {
            Ok(entry) => entry,
            Err(_) => {
                results.push(json!({"doc_name": doc_name, "status": "not_found"}));
                continue;
            }
        };
        let doc_id = entry.get("id").map(py_str).unwrap_or_default();
        match api.delete_document(&doc_id) {
            Ok(_) => results.push(json!({"doc_name": doc_name, "status": "deleted"})),
            Err(e) => results.push(json!({"doc_name": doc_name, "status": "failed",
                                          "error": e.to_string()})),
        }
    }
    let deleted = results.iter().filter(|r| r["status"] == "deleted").count();
    Ok(success(
        obj(json!({"results": results})),
        json!({
            "summary": format!("Deleted {deleted} of {} document(s).", names.len()),
            "options": ["Use browse_documents() to review the remaining library"],
        }),
    ))
}

// ── dispatch ──

type Implementation = fn(&LocalApi, &Obj, Option<&HashSet<String>>) -> Outcome;

/// Each implementation's parameters in signature order: (name, required).
fn signature(name: &str) -> Option<(Implementation, &'static [(&'static str, bool)])> {
    Some(match name {
        "browse_documents" => (
            browse_documents as Implementation,
            &[
                ("folder_id", false),
                ("recursive", false),
                ("sort", false),
                ("query", false),
                ("offset", false),
                ("limit", false),
            ][..],
        ),
        "get_document" => (
            get_document,
            &[
                ("doc_name", true),
                ("folder_id", false),
                ("wait_for_completion", false),
            ][..],
        ),
        "get_document_structure" => (
            get_document_structure,
            &[
                ("doc_name", true),
                ("folder_id", false),
                ("part", false),
                ("wait_for_completion", false),
            ][..],
        ),
        "get_page_content" => (
            get_page_content,
            &[
                ("doc_name", true),
                ("pages", true),
                ("folder_id", false),
                ("wait_for_completion", false),
            ][..],
        ),
        "remove_document" => (
            remove_document,
            &[("doc_names", true), ("folder_id", false)][..],
        ),
        _ => return None,
    })
}

/// `_coerce_bool_args`: string booleans ("false", "no", "0", "") become bools.
/// // ref: agent_tools.py:1193
fn coerce_bool_args(name: &str, kwargs: &mut Obj) {
    let contract = tool_contract();
    let Some(Value::Object(props)) = contract
        .get(name)
        .and_then(|t| t.get("schema"))
        .and_then(|s| s.get("properties"))
    else {
        return;
    };
    for (key, spec) in props {
        if spec.get("type").and_then(Value::as_str) != Some("boolean") {
            continue;
        }
        if let Some(Value::String(s)) = kwargs.get(key) {
            let lowered = pi_pycompat::pystr::strip(s).to_lowercase();
            let b = !matches!(lowered.as_str(), "false" | "no" | "0" | "");
            kwargs.insert(key.clone(), Value::Bool(b));
        }
    }
}

fn invalid_arguments(name: &str, message: &str) -> ToolResult {
    failure(
        &format!("Invalid arguments for {name}: {message}"),
        None,
        json!({"summary": "Invalid tool arguments",
               "options": [format!("Check the {name}() parameter names and types")]}),
        Some("INVALID_INPUT"),
    )
}

/// `call_tool`: run one contract tool, returning `(envelope_json, is_error)`. Never fails:
/// argument errors and unexpected errors become error envelopes. `doc_ids` restricts every
/// document lookup to that allowlist. // ref: agent_tools.py:1204
pub fn call_tool(
    api: &LocalApi,
    name: &str,
    arguments: Option<&Value>,
    doc_ids: Option<&[String]>,
) -> (String, bool) {
    let (payload, is_error) = dispatch(api, name, arguments, doc_ids);
    (pyjson::dumps(&payload), is_error)
}

fn dispatch(
    api: &LocalApi,
    name: &str,
    arguments: Option<&Value>,
    doc_ids: Option<&[String]>,
) -> ToolResult {
    let Some((implementation, params)) = signature(name) else {
        return failure(
            &format!("Unknown tool: {name}"),
            Some(obj(
                json!({"tool_name": name, "available_tools": ALL_TOOLS}),
            )),
            json!({"summary": "Tool not found",
                   "options": [format!("Available tools: {}", ALL_TOOLS.join(", "))]}),
            Some("INVALID_INPUT"),
        );
    };
    let arguments = match arguments {
        None | Some(Value::Null) => Obj::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(other) => {
            return failure(
                &format!(
                    "Invalid arguments for {name}: expected a JSON object, got {}",
                    py_type_name(other)
                ),
                None,
                json!({"summary": "Invalid tool arguments",
                       "options": [format!("Pass {name}() arguments as a JSON object of its parameters")]}),
                Some("INVALID_INPUT"),
            );
        }
    };
    // Underscore keys are the private scope channel, never model arguments; None ≡ omitted.
    let mut kwargs: Obj = arguments
        .into_iter()
        .filter(|(k, v)| !k.starts_with('_') && !v.is_null())
        .collect();
    coerce_bool_args(name, &mut kwargs);
    // inspect.signature(...).bind: missing required parameters first (signature order),
    // then the first unexpected keyword (argument order).
    for (param, required) in params {
        if *required && !kwargs.contains_key(*param) {
            return invalid_arguments(
                name,
                &format!("missing a required argument: {}", str_repr(param)),
            );
        }
    }
    if let Some(extra) = kwargs.keys().find(|k| !params.iter().any(|(p, _)| p == k)) {
        return invalid_arguments(
            name,
            &format!("got an unexpected keyword argument {}", str_repr(extra)),
        );
    }
    let allowed: Option<HashSet<String>> = doc_ids.map(|ids| ids.iter().cloned().collect());
    match implementation(api, &kwargs, allowed.as_ref()) {
        Ok(result) => result,
        Err(message) => failure(
            &format!("{name} failed: {message}"),
            None,
            json!({"summary": "Unexpected error while running the tool",
                   "options": ["Try the request again"],
                   "auto_retry": "This is likely a temporary issue - you can try the request again"}),
            Some("INTERNAL_ERROR"),
        ),
    }
}
