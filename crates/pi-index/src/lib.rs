//! End-to-end indexing of one PDF: the Rust counterpart of
//! `flash/api.py::page_index_flash` (with `flash/main.py::extract_toc`) and of
//! `local_api.py::_index_flash`.
//!
//! ```text
//! extract (01) ─► [triage + OCR] ─► layout per page (02-03, rayon) ─► detect_structure (04-08)
//!   ─► apply_embedded_toc (09) ─► page_index_flash post (fallbacks, merge/expand, summaries)
//!   ─► write_node_id ─► generate_doc_description ─► IndexedDoc
//! ```
//!
//! [`detect_structure`] runs stages 04-08 (blocks, classification, title, headings, outline).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use pi_core::PageSpans;
use pi_layout::{PageLayout, page_bbox_from_viewbox, process_page};
use pi_llm::{Llm, OpenAiClient, RoleConfig};
use pi_outline::{PdfSource, apply_embedded_toc};
use pi_summary::{FlashOptions, OptimizeMode};
use rayon::prelude::*;
use serde_json::{Map, Value, json};

pub use pi_config::Optimize;

/// Largest page-node fallback the managed pipelines accept. ref: flash/api.py:17
pub const FLAT_TREE_MAX_NODES: usize = 10;

/// Page size used when PDFium reports no view box (US Letter).
const DEFAULT_VIEWBOX: [f64; 4] = [0.0, 0.0, 612.0, 792.0];

/// Whether pages are triaged and routed to OCR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OcrMode {
    /// Text layer only (the reference's behaviour).
    #[default]
    Off,
    /// Triage every page; OCR the scanned / garbled / mixed ones when an OCR endpoint is
    /// configured (without one, triage labels are still reported).
    Auto,
}

/// Indexing options (`page_index_flash` arguments plus the Rust-only OCR switch).
#[derive(Debug, Clone)]
pub struct IndexOptions {
    /// Model-written node summaries (`summary=`).
    pub summary: bool,
    /// `optimize=`.
    pub optimize: Optimize,
    /// `use_embedded_toc=`.
    pub use_embedded_toc: bool,
    /// `summary_max_words=` (None: library default).
    pub summary_max_words: Option<usize>,
    /// `summary_concurrency=` (None: library defaults).
    pub summary_concurrency: Option<usize>,
    /// Write the one-sentence document description (needs the description model).
    pub description: bool,
    pub ocr: OcrMode,
    /// The `[ocr]` endpoint; required for OCR to actually run.
    pub ocr_config: Option<pi_config::OcrConfig>,
}

impl Default for IndexOptions {
    fn default() -> Self {
        IndexOptions {
            summary: true,
            optimize: Optimize::Full,
            use_embedded_toc: true,
            summary_max_words: None,
            summary_concurrency: None,
            description: true,
            ocr: OcrMode::Off,
            ocr_config: None,
        }
    }
}

impl IndexOptions {
    /// Options from the `[index]` and `[ocr]` sections.
    pub fn from_config(cfg: &pi_config::Config) -> Self {
        IndexOptions {
            optimize: cfg.index.optimize,
            use_embedded_toc: cfg.index.use_embedded_toc,
            summary_max_words: Some(cfg.index.summary_max_words as usize),
            ocr_config: Some(cfg.ocr.clone()),
            ..Default::default()
        }
    }
}

/// The model roles the indexing lane uses.
#[derive(Clone, Default)]
pub struct LlmRoles {
    pub summary: Option<Arc<dyn Llm>>,
    /// Expand model; defaults to `summary`.
    pub expand: Option<Arc<dyn Llm>>,
    /// Description model; defaults to `summary`.
    pub description: Option<Arc<dyn Llm>>,
    /// Summary model name (picks the tokenizer for the raw-text threshold).
    pub summary_model: Option<String>,
}

impl LlmRoles {
    /// OpenAI-compatible clients for every role whose `base_url` and `model` are set.
    pub fn from_config(cfg: &pi_config::LlmConfig) -> Result<LlmRoles> {
        let client = |role: &pi_config::LlmRole| -> Result<Option<Arc<dyn Llm>>> {
            let (Some(base_url), Some(model)) = (&role.base_url, &role.model) else {
                return Ok(None);
            };
            let mut rc = RoleConfig::new(base_url.clone(), model.clone());
            rc.api_key = role.api_key();
            rc.concurrency = role.concurrency;
            rc.timeout = std::time::Duration::from_secs_f64(role.timeout_s);
            rc.max_retries = role.max_retries;
            Ok(Some(Arc::new(OpenAiClient::new(rc)?) as Arc<dyn Llm>))
        };
        Ok(LlmRoles {
            summary: client(&cfg.summary)?,
            expand: client(&cfg.expand)?,
            description: client(&cfg.description)?,
            summary_model: cfg.summary.model.clone(),
        })
    }
}

/// Seconds spent per stage.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Timings {
    pub extract_s: f64,
    /// Triage + OCR (0 when OCR is off).
    pub ocr_s: f64,
    pub layout_s: f64,
    /// Structure detection (04-08 seam) + embedded bookmarks (09).
    pub structure_s: f64,
    /// Fallbacks, merge/expand, summaries.
    pub post_s: f64,
    pub description_s: f64,
    pub total_s: f64,
}

/// Triage outcome of one page.
#[derive(Debug, Clone, PartialEq)]
pub struct PageRoute {
    pub page: u32,
    /// `text` / `scanned` / `garbled` / `mixed` / `graphic`.
    pub label: String,
    /// Whether OCR spans were used for this page.
    pub ocr: bool,
    pub error: Option<String>,
}

/// One indexed document.
#[derive(Debug, Clone)]
pub struct IndexedDoc {
    /// The `page_index_flash` result dict: `doc_name`, `doc_title`, `structure`,
    /// `has_abstract_or_references_section`, `toc_source`, and `optimize` when it ran.
    pub result: Map<String, Value>,
    /// The page texts the pipeline used (the content of `pages.json`).
    pub page_texts: Vec<String>,
    /// Per-page triage (empty when OCR is off).
    pub triage: Vec<PageRoute>,
    pub timings: Timings,
    pub description: Option<String>,
    /// `flash_rejection_reason`: why the managed pipelines would refuse this result.
    pub rejection: Option<String>,
}

impl IndexedDoc {
    pub fn structure(&self) -> &[Value] {
        self.result
            .get("structure")
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice)
    }

    pub fn toc_source(&self) -> &str {
        self.result
            .get("toc_source")
            .and_then(Value::as_str)
            .unwrap_or("")
    }

    /// The tree as stored: node ids renumbered (`write_node_id`), no internal keys.
    pub fn stored_structure(&self) -> Value {
        let mut v = Value::Array(self.structure().to_vec());
        pi_optimize::tree::write_node_id(&mut v, 0);
        v
    }

    /// Count of nodes at every depth.
    pub fn node_count(&self) -> usize {
        fn walk(nodes: &[Value]) -> usize {
            nodes
                .iter()
                .map(|n| {
                    1 + n
                        .get("nodes")
                        .and_then(Value::as_array)
                        .map_or(0, |c| walk(c))
                })
                .sum()
        }
        walk(self.structure())
    }

    /// Commit into a `.pageindex` store the way `local_api.py::submit_document` does (name
    /// uniqued under the store lock, tree without node text, pipeline page text), returning
    /// `(doc_id, stored name)`. Refused results are not stored unless `accept_flat` and the
    /// refusal is only the flat-tree size limit.
    pub fn commit(
        &self,
        api: &pi_store::LocalApi,
        doc_name: &str,
        accept_flat: bool,
    ) -> Result<Option<(String, String)>> {
        let flat_ok = accept_flat && self.toc_source() == "pages" && !self.structure().is_empty();
        if self.rejection.is_some() && !flat_ok {
            return Ok(None);
        }
        let structure = self.stored_structure();
        let committed = api
            .commit_document(pi_store::NewDocument {
                name: doc_name,
                description: self.description.as_deref(),
                structure: &structure,
                page_texts: &self.page_texts,
                metadata: self.metadata(),
                mode: "flash",
            })
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(Some((
            committed["doc_id"].as_str().unwrap_or_default().to_string(),
            committed["name"].as_str().unwrap_or_default().to_string(),
        )))
    }

    /// doc.json `metadata`: the page-label histogram when triage ran, else None.
    pub fn metadata(&self) -> Option<Map<String, Value>> {
        if self.triage.is_empty() {
            return None;
        }
        let mut labels: BTreeMap<&str, u64> = BTreeMap::new();
        for p in &self.triage {
            *labels.entry(p.label.as_str()).or_default() += 1;
        }
        let mut m = Map::new();
        m.insert("page_labels".into(), json!(labels));
        m.insert(
            "ocr_pages".into(),
            json!(self.triage.iter().filter(|p| p.ocr).count()),
        );
        Some(m)
    }
}

/// `flash_rejection_reason(result)`. ref: flash/api.py:148
pub fn flash_rejection_reason(result: &Map<String, Value>) -> Option<String> {
    let n = result
        .get("structure")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let source = result.get("toc_source").and_then(Value::as_str);
    if source == Some("unreadable") {
        return Some(
            "PageIndex Flash found no text layer in this PDF (scanned or image-only); run OCR \
             before indexing it."
                .into(),
        );
    }
    if source == Some("pages") && n > FLAT_TREE_MAX_NODES {
        return Some(format!(
            "PageIndex Flash found no layout structure in this document ({n} pages); try \
             mode='standard', which builds the structure with the model."
        ));
    }
    if n == 0 {
        return Some(
            "PageIndex Flash could not extract a structure from this PDF; try mode='standard', \
             which builds the structure with the model."
                .into(),
        );
    }
    None
}

/// Stages 04-08 (`flash/main.py::extract_toc` steps 3-11): blocks and reading order,
/// classification, title, captions, section openers, heading candidates, outline assembly with
/// its validity gates, and `outline_to_dict_tree`. Returns the `extract_toc` result dict
/// (before embedded bookmarks) with `page_texts` = each page's block texts in reading order
/// joined by newlines, exactly as the reference builds them.
pub fn detect_structure(doc_name: &str, layouts: Vec<PageLayout>) -> Map<String, Value> {
    let doc = pi_layout::phases::build_document(layouts);
    let (classified, outline) = pi_outline::pipeline::classify_and_outline(&doc);
    let page_texts: Vec<String> = doc
        .pages
        .iter()
        .map(|p| {
            p.reading
                .iter()
                .map(|&id| {
                    pi_layout::model::block::block_text(&p.blocks[id], &p.layout).to_string()
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect();
    let mut result = Map::new();
    result.insert("doc_name".into(), json!(doc_name));
    result.insert("doc_title".into(), json!(classified.doc_title));
    result.insert("structure".into(), Value::Array(outline.tree));
    result.insert(
        "has_abstract_or_references_section".into(),
        json!(outline.has_abstract_or_references),
    );
    result.insert("page_texts".into(), json!(page_texts));
    result.insert("toc_source".into(), json!("detected"));
    result
}

fn optimize_mode(o: Optimize) -> OptimizeMode {
    match o {
        Optimize::Full => OptimizeMode::Full,
        Optimize::Merge => OptimizeMode::Merge,
        Optimize::Off => OptimizeMode::Off,
    }
}

fn label_name(l: pi_triage::PageLabel) -> String {
    serde_json::to_value(l)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Per-page OCR output kept for `pages.json`: the table markdown of each page.
type PageTables = Vec<Vec<String>>;

/// Triage every page; OCR the ones that need it when an endpoint is configured. The engine is
/// chosen by `[ocr] profile`: `spans-json` (generic vision model) or `paddleocr-vl`.
async fn triage_and_ocr(
    bytes: Vec<u8>,
    pages: Vec<PageSpans>,
    cfg: Option<&pi_config::OcrConfig>,
) -> Result<(Vec<PageSpans>, Vec<PageRoute>, PageTables)> {
    let b = bytes.clone();
    let triage = tokio::task::spawn_blocking(move || {
        pi_triage::triage_pdf_bytes(b, &pi_triage::Thresholds::default())
    })
    .await??;
    let endpoint = cfg.and_then(|c| Some((c, c.base_url.clone()?, c.model.clone()?)));
    let wants_ocr = triage.iter().any(|t| t.label.needs_ocr());
    let Some((c, base_url, model)) = endpoint.filter(|_| wants_ocr) else {
        let routes = triage
            .iter()
            .map(|t| PageRoute {
                page: t.signals.page,
                label: label_name(t.label),
                ocr: false,
                error: None,
            })
            .collect();
        let n = pages.len();
        return Ok((pages, routes, vec![Vec::new(); n]));
    };
    let opts = pi_ocr::OcrOptions {
        dpi: c.dpi as f64,
        concurrency: c.concurrency,
        ..Default::default()
    };
    let timeout = c.timeout_s.ceil() as u64;
    let routed = if c.profile == "paddleocr-vl" {
        let mut pc = pi_ocr::PaddleVlConfig::new(base_url, model, c.api_key());
        pc.timeout_s = timeout;
        pc.max_tokens = c.max_tokens;
        pc.max_pixels = c.max_pixels;
        pc.tables = c.tables;
        let engine = pi_ocr::PaddleVlEngine::new(pc)?;
        pi_ocr::ocr_and_route(bytes, pages, &triage, &engine, &opts).await?
    } else {
        let mut vc = pi_ocr::OpenAiVisionConfig::new(base_url, model, c.api_key());
        vc.timeout_s = timeout;
        vc.json_mode = c.json_mode;
        vc.max_tokens = c.max_tokens;
        let engine = pi_ocr::OpenAiVisionEngine::new(vc)?;
        pi_ocr::ocr_and_route(bytes, pages, &triage, &engine, &opts).await?
    };
    let routes = routed
        .iter()
        .zip(&triage)
        .map(|(r, t)| PageRoute {
            page: t.signals.page,
            label: label_name(r.label),
            ocr: !matches!(r.source, pi_ocr::PageSource::TextLayer),
            error: r.error.clone(),
        })
        .collect();
    let tables = routed
        .iter()
        .map(|r| r.tables.iter().map(|t| t.markdown.clone()).collect())
        .collect();
    Ok((
        routed.into_iter().map(|r| r.spans).collect(),
        routes,
        tables,
    ))
}

/// Index one PDF. `llm` is required for `summary`, `optimize = Full` and `description`;
/// without it those passes are skipped (summary/full with no model is an error, as in the
/// reference; the description is simply left out).
pub async fn index_document(
    pdf_bytes: Vec<u8>,
    doc_name: &str,
    opts: &IndexOptions,
    llm: Option<LlmRoles>,
) -> Result<IndexedDoc> {
    let t_total = Instant::now();
    let mut timings = Timings::default();
    if !pdf_bytes.starts_with(b"%PDF-") {
        bail!("File does not look like a PDF: {doc_name}");
    }

    // 01 extract
    let t = Instant::now();
    let b = pdf_bytes.clone();
    let spans = tokio::task::spawn_blocking(move || pi_extract::extract_pdf_bytes(b))
        .await?
        .with_context(|| format!("Could not open PDF: {doc_name}"))?;
    if spans.is_empty() {
        bail!("PDF contains no pages");
    }
    timings.extract_s = t.elapsed().as_secs_f64();

    // triage + OCR
    let t = Instant::now();
    let (spans, triage, ocr_tables) = match opts.ocr {
        OcrMode::Off => (spans, Vec::new(), Vec::new()),
        OcrMode::Auto => triage_and_ocr(pdf_bytes.clone(), spans, opts.ocr_config.as_ref()).await?,
    };
    timings.ocr_s = t.elapsed().as_secs_f64();

    // 02-03 layout, per page in parallel. ref: flash/main.py:139-150
    let t = Instant::now();
    let layouts = tokio::task::spawn_blocking(move || {
        spans
            .par_iter()
            .map(|p| {
                let vb = p.viewbox.unwrap_or(DEFAULT_VIEWBOX);
                let mut layout =
                    process_page(&p.spans, p.page, page_bbox_from_viewbox(vb, p.rotation));
                // extract_toc sets these for the heading coordinate projection (main.py:145-146)
                layout.viewport_box = p.viewbox;
                layout.rot = p.rotation;
                layout
            })
            .collect::<Vec<PageLayout>>()
    })
    .await?;
    timings.layout_s = t.elapsed().as_secs_f64();

    // 04-08 seam, then 09 embedded bookmarks.
    let t = Instant::now();
    let mut result = detect_structure(doc_name, layouts);
    let mut page_texts: Vec<String> = result
        .get("page_texts")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|v| v.as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default();
    // OCR'd tables go into the page text (pages.json and the LLM passes) as markdown after the
    // page's own text; the tree comes from the spans and is unaffected.
    if ocr_tables.iter().any(|t| !t.is_empty()) {
        for (text, tables) in page_texts.iter_mut().zip(&ocr_tables) {
            for md in tables {
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(md);
            }
        }
        result.insert("page_texts".into(), json!(page_texts));
    }
    if opts.use_embedded_toc {
        let structure: Vec<Value> = result
            .get("structure")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let n = page_texts.len() as i64;
        let texts = page_texts.clone();
        let b = pdf_bytes.clone();
        let (structure, source) = tokio::task::spawn_blocking(move || {
            apply_embedded_toc(&structure, PdfSource::Bytes(b), n, Some(&texts))
        })
        .await?;
        result.insert("structure".into(), Value::Array(structure));
        result.insert("toc_source".into(), json!(source));
    }
    timings.structure_s = t.elapsed().as_secs_f64();

    // page_index_flash post-processing. ref: flash/api.py:190-231
    let t = Instant::now();
    let roles = llm.unwrap_or_default();
    let fopts = FlashOptions {
        summary: opts.summary,
        optimize: optimize_mode(opts.optimize),
        concurrency: opts.summary_concurrency,
        max_words: opts.summary_max_words,
        summary_model: roles.summary_model.clone(),
        summary_llm: roles.summary.clone(),
        optimize_llm: roles.expand.clone(),
    };
    let result = pi_summary::page_index_flash_post(result, &fopts).await?;
    timings.post_s = t.elapsed().as_secs_f64();

    let rejection = flash_rejection_reason(&result);
    let mut doc = IndexedDoc {
        result,
        page_texts,
        triage,
        timings,
        description: None,
        rejection,
    };

    // generate_doc_description(create_clean_structure_for_description(structure)).
    // ref: local_api.py:253-257
    let t = Instant::now();
    let describer = roles.description.clone().or(roles.summary.clone());
    if opts.description
        && doc.rejection.is_none()
        && let Some(llm) = describer
    {
        let clean = pi_summary::clean_structure_for_description(&doc.stored_structure());
        doc.description = Some(pi_summary::generate_doc_description(&clean, llm.as_ref()).await?);
    }
    doc.timings.description_s = t.elapsed().as_secs_f64();
    doc.timings.total_s = t_total.elapsed().as_secs_f64();
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejection_policy() {
        let r = |structure: Value, src: &str| {
            let mut m = Map::new();
            m.insert("structure".into(), structure);
            m.insert("toc_source".into(), json!(src));
            flash_rejection_reason(&m)
        };
        assert!(
            r(json!([]), "unreadable")
                .unwrap()
                .contains("no text layer")
        );
        assert!(
            r(Value::Array(vec![json!({}); 11]), "pages")
                .unwrap()
                .contains("11 pages")
        );
        assert!(r(Value::Array(vec![json!({}); 10]), "pages").is_none());
        assert!(
            r(json!([]), "detected")
                .unwrap()
                .contains("could not extract")
        );
        assert!(r(json!([{}]), "bookmarks").is_none());
    }
}
