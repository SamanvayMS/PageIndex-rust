//! Constants and contract text derived from the reference (VectifyAI/PageIndex @619cbd8).

use serde_json::{Value, json};

/// // ref: pageindex/agent_tools.py:39
pub const TOOL_RESPONSE_CHAR_LIMIT: usize = 100_000;
/// // ref: pageindex/agent_tools.py:40
pub const STRUCTURE_FIRST_PAGE_THRESHOLD: i64 = 20;
/// `int(TOOL_RESPONSE_CHAR_LIMIT * 0.95)`. // ref: pageindex/agent_tools.py:42
pub const CHAR_BUDGET: usize = 95_000;
/// // ref: pageindex/agent_tools.py:43
pub const MAX_REQUESTED_PAGES: u128 = 10_000;
/// // ref: pageindex/agent_tools.py:44
pub const SIMILAR_NAMES_LIMIT: usize = 3;
/// `difflib.get_close_matches(..., cutoff=0.5)`. // ref: pageindex/agent_tools.py:410-411
pub const SIMILAR_NAMES_CUTOFF: f64 = 0.5;
/// Seconds. // ref: pageindex/agent_tools.py:45
pub const TOOL_WAIT_TIMEOUT_S: f64 = 180.0;
/// Seconds. // ref: pageindex/agent_tools.py:46
pub const TOOL_WAIT_INTERVAL_S: f64 = 5.0;
/// `_all_documents` page size. // ref: pageindex/agent_tools.py:347
pub const ALL_DOCUMENTS_PAGE: i64 = 100;
/// `remove_document` batch cap. // ref: pageindex/agent_tools.py:1149
pub const MAX_REMOVE: usize = 10;

/// // ref: pageindex/agent_tools.py:305-307
pub const READ_TOOLS: [&str; 4] = [
    "browse_documents",
    "get_document",
    "get_document_structure",
    "get_page_content",
];
pub const MANAGEMENT_TOOLS: [&str; 1] = ["remove_document"];
/// `list(_IMPLEMENTATIONS)`. // ref: pageindex/agent_tools.py:1180-1186
pub const ALL_TOOLS: [&str; 5] = [
    "browse_documents",
    "get_document",
    "get_document_structure",
    "get_page_content",
    "remove_document",
];

/// // ref: pageindex/agent_tools.py:649-650
pub const STRUCTURE_KEY_ORDER: [&str; 8] = [
    "title",
    "node_id",
    "start_index",
    "end_index",
    "page_index",
    "prefix_summary",
    "summary",
    "nodes",
];

/// // ref: pageindex/agent_tools.py:48-53
pub const DOC_NAME_DESCRIPTION: &str = "Copy the `name` field verbatim from a browse_documents() or \
search_documents() response (case-sensitive, include extension). Example: \"Q3 Report.pdf\". If \
the response shows two documents with the same name, pass `folder_id` alongside to disambiguate.";

/// // ref: pageindex/agent_tools.py:54-60
pub const FOLDER_ID_DISAMBIGUATOR_DESCRIPTION: &str = "Disambiguator for same-name documents. Copy \
the `folder_id` from the intended browse/search result; use \"root\" for root-level documents, or \
\"shared-with-me\"/\"following\" for the read-only folders at the library root; omit if \
`doc_name` is unique. Copy any folder_id verbatim from a browse_documents()/get_folder_structure() \
response, never construct one.";

/// // ref: pageindex/agent_tools.py:61-64
pub const WAIT_FOR_COMPLETION_DESCRIPTION: &str = "If true and document is processing, \
automatically wait up to 3 minutes until completed. Reduces repeated tool calls.";

/// `TOOL_CONTRACT`: names, descriptions, schemas and annotations identical to the cloud MCP
/// server's tools/list. // ref: pageindex/agent_tools.py:68-303
pub fn tool_contract() -> Value {
    let doc_name = json!({"type": "string", "minLength": 1, "description": DOC_NAME_DESCRIPTION});
    let folder_id = json!({
        "anyOf": [{"type": "string"}, {"type": "null"}],
        "description": FOLDER_ID_DISAMBIGUATOR_DESCRIPTION,
    });
    let wait = json!({
        "type": "boolean", "default": false, "description": WAIT_FOR_COMPLETION_DESCRIPTION,
    });
    json!({
        "browse_documents": {
            "annotations": {"readOnlyHint": true, "openWorldHint": false},
            "description": "Primary document retrieval tool. After orienting with \
    get_folder_structure() (when available), use this for all document-related questions. The bare \
    call returns root-level sub-folders and documents; pass folder_id to drill into a sub-folder level \
    by level. Use sort=\"relevance\" + query for semantic ranking. Do NOT jump to search_documents() \
    first — it is an escalation path, only after browse_documents(sort=\"relevance\") has failed.",
            "schema": {
                "type": "object",
                "properties": {
                    "folder_id": {
                        "type": "string",
                        "default": "root",
                        "description": "Folder scope (default \"root\"). Pass a specific folder ID \
    to scope into that folder, or \"root\" to reference the library root. The read-only \
    \"shared-with-me\" and \"following\" folders live at the library root — pass one of those ids to \
    browse them. Copy any folder_id verbatim from a browse/tree response, never construct one. \
    Combine with `recursive` to control breadth.",
                    },
                    "recursive": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether to include documents from descendant folders. \
    When false (default), returns the direct contents of folder_id along with its sub-folders — \
    prefer this for level-by-level exploration so you retain folder hierarchy context. When true, \
    flattens all descendant documents into one list and omits sub-folders — use only when a \
    non-recursive browse of the target folder returned no relevant results and you need to widen the \
    scope, or the user explicitly requests a flat listing.",
                    },
                    "sort": {
                        "type": "string",
                        "enum": ["time", "relevance"],
                        "default": "time",
                        "description": "Sort order. \"time\" (default) sorts by upload date \
    (newest first); \"relevance\" orders documents by semantic relevance to `query`. Relevance also \
    works inside the read-only shared folders — pass their folder_id — but at the library root it \
    ranks only your own documents.",
                    },
                    "query": {
                        "type": "string",
                        "description": "Search query for relevance ranking. Required when \
    sort=\"relevance\".",
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 9007199254740991_u64,
                        "default": 0,
                        "description": "Zero-based pagination offset. Pass the value of \
    `next_offset` from the previous response to fetch the next page.",
                    },
                    "limit": {
                        "type": "number",
                        "minimum": 1,
                        "maximum": 50,
                        "default": 10,
                        "description": "Number of documents to return per page (1-50, default 10)",
                    },
                },
                "required": [],
            },
        },
        "get_document": {
            "annotations": {"readOnlyHint": true, "openWorldHint": false},
            "description": "Check a document's processing status and metadata. `status` is one \
    of \"pending\", \"queued\", \"processing\", \"completed\", or \"failed\" — call this before \
    `get_document_structure()` or `get_page_content()` to confirm the document is ready.",
            "schema": {
                "type": "object",
                "properties": {
                    "doc_name": doc_name,
                    "folder_id": folder_id,
                    "wait_for_completion": wait,
                },
                "required": ["doc_name"],
            },
        },
        "get_document_structure": {
            "annotations": {"readOnlyHint": true, "openWorldHint": false},
            "description": format!("Extract a document's hierarchical outline (headers, \
    sections, page references). REQUIRED for documents over {STRUCTURE_FIRST_PAGE_THRESHOLD} pages — \
    call this first to locate relevant sections, then pass their page numbers to \
    `get_page_content()`. Use the `part` parameter to iterate large outlines until \
    `pagination.has_more` is false."),
            "schema": {
                "type": "object",
                "properties": {
                    "doc_name": doc_name,
                    "folder_id": folder_id,
                    "part": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 9007199254740991_u64,
                        "default": 1,
                        "description": "Part number for pagination (1-based, default 1). For \
    large outlines, increment until the response's `pagination.has_more` becomes false.",
                    },
                    "wait_for_completion": wait,
                },
                "required": ["doc_name"],
            },
        },
        "get_page_content": {
            "annotations": {"readOnlyHint": true, "openWorldHint": false},
            "description": format!("Extract page content from a processed document. Use tight, \
    targeted page ranges — never the whole document at once. For documents over \
    {STRUCTURE_FIRST_PAGE_THRESHOLD} pages, call `get_document_structure()` first to pick relevant \
    sections. Embedded image paths in the response feed into `get_document_image()`."),
            "schema": {
                "type": "object",
                "properties": {
                    "doc_name": doc_name,
                    "folder_id": folder_id,
                    "pages": {
                        "type": "string",
                        "minLength": 1,
                        "pattern": PAGES_PATTERN,
                        "description": "Page specification: \"5\", \"3,7,10\", \"5-10\", or \
    \"1-3,7,9-12\"",
                    },
                    "wait_for_completion": wait,
                },
                "required": ["doc_name", "pages"],
            },
        },
        "remove_document": {
            "annotations": {
                "readOnlyHint": false, "destructiveHint": true,
                "idempotentHint": true, "openWorldHint": false,
            },
            "description": "Permanently delete documents and all associated data. Only invoke \
    when the user explicitly names the documents AND confirms deletion. Returns `results` — one entry \
    per requested document: `{ doc_name, status: \"deleted\" | \"not_found\" | \"failed\", error? }`. \
    Inspect each entry for per-document failures. This action is irreversible.",
            "schema": {
                "type": "object",
                "properties": {
                    "doc_names": {
                        "type": "array",
                        "items": {"type": "string", "minLength": 1},
                        "minItems": 1,
                        "maxItems": 10,
                        "description": "Array of document names to delete. Each name must be \
    copied verbatim from the `name` field of a browse_documents() or search_documents() response \
    (case-sensitive, include extension). Example: [\"Q3 Report.pdf\", \"draft.pdf\"]. Max 10 per \
    call.",
                    },
                    "folder_id": folder_id,
                },
                "required": ["doc_names"],
            },
        },
    })
}

/// The contract's `pages` pattern (ECMA; matched with ASCII semantics).
/// // ref: pageindex/agent_tools.py:253
pub const PAGES_PATTERN: &str = r"^(\d+(-\d+)?)(,\s*\d+(-\d+)?)*$";

/// `_LOCAL_HIDDEN_PARAMS`. // ref: pageindex/agent_tools.py:1272-1278
pub const LOCAL_HIDDEN_PARAMS: [(&str, &[&str]); 5] = [
    (
        "browse_documents",
        &["folder_id", "recursive", "sort", "query"],
    ),
    ("get_document", &["folder_id"]),
    ("get_document_structure", &["folder_id"]),
    ("get_page_content", &["folder_id"]),
    ("remove_document", &["folder_id"]),
];

/// // ref: pageindex/agent_tools.py:1280-1284
pub const LOCAL_DOC_NAME_DESCRIPTION: &str = "Copy the `name` field verbatim from a \
browse_documents() response (case-sensitive, include extension). Example: \"Q3 Report.pdf\". \
Document names are unique in a local library.";

/// `_LOCAL_DESCRIPTIONS["browse_documents"]`. // ref: pageindex/agent_tools.py:1287-1295
pub const LOCAL_BROWSE_DESCRIPTION: &str = "Primary document retrieval tool — first choice for \
any document-related question. Lists your documents newest first with names and descriptions; \
match them against the user's intent and page through with `offset: next_offset` (limit up to 50) \
while `has_more` is true. Folder browsing and semantic ranking (sort=\"relevance\") are not \
supported in local mode yet — they work on PageIndex cloud.";

/// The sentence-dropping edit applied to `get_page_content`'s description.
/// // ref: pageindex/agent_tools.py:1298-1300
pub const LOCAL_IMAGE_SENTENCE_RE: &str = r"[\s\x1C-\x1F]*[^.]*`get_document_image\(\)`[^.]*\.";

/// `_LOCAL_PARAM_DESCRIPTIONS["remove_document", "doc_names"]`.
/// // ref: pageindex/agent_tools.py:1307-1312
pub const LOCAL_DOC_NAMES_DESCRIPTION: &str = "Array of document names to delete. Each name must \
be copied verbatim from the `name` field of a browse_documents() response (case-sensitive, include \
extension). Example: [\"Q3 Report.pdf\", \"draft.pdf\"]. Max 10 per call.";

// ── agent instructions ── // ref: pageindex/agent_tools.py:1581-1627

const INSTRUCTIONS_HEADER: &str = "PageIndex by Vectify AI is a document platform for uploading \
and managing long PDFs (research papers, financial reports, legal docs, textbooks, etc.).";

const READING_WORKFLOW: &str = "READING WORKFLOW:
- For documents over 20 pages: call get_document_structure() first to locate relevant sections, then get_page_content() with targeted page ranges.
- For small documents (20 pages or fewer): call get_page_content() directly.";

const TOOL_USAGE_RULES: &str = "TOOL USAGE RULES:
- Invoke a tool only when all required parameters are present or clearly inferable. Never invent placeholder values.
- If a tool returns an error, present the provided next_steps/options to the user instead of retrying blindly.";

const DISCOVERY: &str = "DOCUMENT DISCOVERY:
- browse_documents() — DEFAULT discovery tool, first choice for any document-related question. The bare call returns your documents newest first with names and descriptions; match them against the user's intent.";

const DECISION: &str = "DECISION:
- \"What do I have / list / recent\" → browse_documents()
- ANY question that needs a document to answer (including \"find THE paper about Y\") → browse_documents(), then pick the documents whose name/description matches the question";

const AFTER_DISCOVERY: &str = "- Skip discovery ONLY for questions with NO possible document connection (e.g., \"capital of France\").
- After discovery: 1 match or 1 clearly best match → proceed to read and answer without asking. Multiple equally relevant → ask user to pick.
- Results returned ≠ correct results. If the returned documents do not clearly match the user's intent (e.g., wrong topic, wrong time period, wrong document type), treat it the same as \"not found\" and continue the PERSISTENCE protocol below.";

const PERSISTENCE: &str = "PERSISTENCE (before concluding the target document is not in the library):
This protocol applies both when results are empty AND when results are returned but none match the user's intent. Do NOT give up after a single discovery attempt. Follow these steps in order:
1. browse_documents() and compare every returned name/description against the user's intent
2. Rephrase the query with synonyms or alternative terms and browse again
3. Page through the ENTIRE library with `limit: 50` and `offset: next_offset` until has_more is false — MANDATORY, must be completed before concluding \"not found\"
Only after ALL steps have been tried may you conclude the document is not in the library. Do NOT fall back to general knowledge — if the user's question references their own documents, exhaust every discovery path first.";

/// The sections of `AGENT_INSTRUCTIONS`, joined with a blank line.
pub const AGENT_INSTRUCTIONS_PARTS: [&str; 7] = [
    INSTRUCTIONS_HEADER,
    READING_WORKFLOW,
    TOOL_USAGE_RULES,
    DISCOVERY,
    DECISION,
    AFTER_DISCOVERY,
    PERSISTENCE,
];

/// `CHAT_HEADER`: the managed chat's lead-in, placed before the agent instructions.
/// // ref: pageindex/local_chat.py:20-23
pub const CHAT_HEADER: &str = "You are PageIndex by Vectify AI, a document-focused assistant. \
Be concise, never use emojis, and do not expose tool names.";

/// `LOCAL_CITATION_PROMPTS["markdown"]`. // ref: pageindex/agent_tools.py:1635-1644
pub const CITATION_PROMPT_MARKDOWN: &str = "GROUNDING
- Answer only from the user's PageIndex documents. Call get_page_content() and state only what was actually read there.
- Never fill a gap from general knowledge. When the documents do not answer the question, say so.

CITATIONS
- Cite only statements supported by tool outputs, as a bracketed reference: [{docName}, p. {pageNumber}] or [{docName}, p. {pageNumber}, block {blockId}]. Place immediately after the claim.
- When page content includes block_id values, citations MUST be block-level: copy the exact block_id of the supporting block. Page-only cites are allowed ONLY when the tool output carries no block_id (legacy documents, structure outlines). NEVER invent or alter block_id values.
- For a claim drawn from multiple blocks on one page, add one reference per supporting block (at most 3); beyond that, cite the single strongest block.
- Each reference must reference a SINGLE page integer. For multi-page citations, use separate references.";

/// `LOCAL_CITATION_PROMPTS["cite"]`. // ref: pageindex/agent_tools.py:1645-1654
pub const CITATION_PROMPT_CITE: &str = "GROUNDING
- Answer only from the user's PageIndex documents. Call get_page_content() and state only what was actually read there.
- Never fill a gap from general knowledge. When the documents do not answer the question, say so.

CITATIONS
- Cite only statements supported by tool outputs: <cite doc=\"{docName}\" page=\"{pageNumber}\"/> or <cite doc=\"{docName}\" page=\"{pageNumber}\" block=\"{blockId}\"/>. Place immediately after the claim.
- When page content includes block_id values, citations MUST be block-level: copy the exact block_id of the supporting block. Page-only cites are allowed ONLY when the tool output carries no block_id (legacy documents, structure outlines). NEVER invent or alter block_id values.
- For a claim drawn from multiple blocks on one page, add one tag per supporting block (at most 3); beyond that, cite the single strongest block.
- Each tag must reference a SINGLE page integer. For multi-page citations, use separate tags.";
