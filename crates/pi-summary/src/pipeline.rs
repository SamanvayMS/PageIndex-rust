//! The model passes of `page_index_flash` after extraction: optimize (merge / expand) and the
//! summaries, overlapped as `flash/api.py::_optimize_and_summarize` does.
//! ref: pageindex/flash/api.py:76-123, 190-231

use std::sync::{Arc, Mutex};

use pi_llm::Llm;
use pi_optimize::flash::page_lines;
use pi_optimize::tree::{Tree, TreeError};
use pi_optimize::{OptimizeError, OptimizeOptions, Prepared, optimize, prepare};
use serde_json::{Map, Value};

use crate::scheduler::{SummaryError, SummaryOptions, SummaryScheduler, summarize_tree};

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error(transparent)]
    Tree(#[from] TreeError),
    #[error(transparent)]
    Optimize(#[from] OptimizeError),
    #[error(transparent)]
    Summary(#[from] SummaryError),
    #[error("{0}")]
    Config(String),
}

/// `optimize=` of `page_index_flash`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptimizeMode {
    Off,
    /// Deterministic merge only.
    Merge,
    /// Merge + LLM expand.
    Full,
}

/// The model-facing arguments of `page_index_flash`.
#[derive(Clone)]
pub struct FlashOptions {
    pub summary: bool,
    pub optimize: OptimizeMode,
    /// `summary_concurrency`: cap per lane (summaries; expand up to its own ceiling of 32).
    pub concurrency: Option<usize>,
    /// `summary_max_words`.
    pub max_words: Option<usize>,
    /// Summary model name (picks the tokenizer for the raw-text threshold).
    pub summary_model: Option<String>,
    pub summary_llm: Option<Arc<dyn Llm>>,
    /// Model for expand; defaults to `summary_llm`.
    pub optimize_llm: Option<Arc<dyn Llm>>,
}

/// `_optimize_async(structure, page_texts, do_expand, model, on_final, concurrency)`: the
/// report dict. ref: api.py:82
pub async fn optimize_async(
    tree: &Mutex<Tree>,
    page_texts: &[String],
    do_expand: bool,
    llm: Option<&dyn Llm>,
    on_final: Option<&pi_optimize::OnFinal<'_>>,
    concurrency: Option<usize>,
) -> Result<Map<String, Value>, OptimizeError> {
    let lines: Vec<Vec<String>> = page_texts.iter().map(|p| page_lines(p)).collect();
    let opts = OptimizeOptions {
        do_expand,
        page_count: Some(page_texts.len() as i64),
        concurrency,
        ..Default::default()
    };
    let outcome = optimize(tree, Some(page_texts), Some(&lines), llm, &opts, on_final).await?;
    Ok(outcome.report())
}

/// `_optimize_and_summarize`: expand and summarize on one loop; a node is summarized as soon
/// as expand can no longer change it, a parent once its children are done. ref: api.py:111
pub async fn optimize_and_summarize(
    tree: Arc<Mutex<Tree>>,
    page_texts: Arc<Vec<String>>,
    optimize_llm: &dyn Llm,
    summary_llm: Arc<dyn Llm>,
    summary: &SummaryOptions,
) -> Result<Map<String, Value>, PipelineError> {
    let scheduler = SummaryScheduler::new(tree.clone(), page_texts.clone(), summary_llm, summary);
    let on_final = |nodes: &[usize]| scheduler.mark_final(nodes);
    let report = optimize_async(
        &tree,
        &page_texts,
        true,
        Some(optimize_llm),
        Some(&on_final),
        summary.concurrency,
    )
    .await?;
    scheduler.finish().await?;
    Ok(report)
}

/// `page_index_flash` from the `extract_toc` result on: fallbacks, refusal, optimize and
/// summaries. Returns the result dict (without `page_texts`). ref: api.py:191-231
pub async fn page_index_flash_post(
    result: Map<String, Value>,
    opts: &FlashOptions,
) -> Result<Map<String, Value>, PipelineError> {
    let Prepared {
        mut result,
        pages,
        refused,
    } = prepare(result);
    if refused {
        return Ok(result);
    }
    let structure = result
        .get("structure")
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    let has_structure = structure.as_array().is_some_and(|a| !a.is_empty());
    if !has_structure {
        return Ok(result);
    }
    let summary_llm = opts.summary_llm.clone();
    let optimize_llm = opts.optimize_llm.clone().or_else(|| summary_llm.clone());
    if opts.summary && summary_llm.is_none() {
        return Err(PipelineError::Config(
            "summary=True needs a summary model".into(),
        ));
    }
    let sopts = SummaryOptions {
        model: opts.summary_model.clone(),
        concurrency: opts.concurrency,
        max_words: opts.max_words,
        ..Default::default()
    };
    let tree = Arc::new(Mutex::new(Tree::from_value(&structure)?));
    let pages = Arc::new(pages);
    // bookmark-only extractions carry no page_texts and scanned ones only empty strings
    let do_expand = opts.optimize == OptimizeMode::Full && pages.iter().any(|p| !p.is_empty());
    if do_expand && optimize_llm.is_none() {
        return Err(PipelineError::Config(
            "optimize='full' needs a model".into(),
        ));
    }
    if opts.optimize != OptimizeMode::Off && opts.summary && do_expand {
        let report = optimize_and_summarize(
            tree.clone(),
            pages.clone(),
            optimize_llm.as_deref().unwrap(),
            summary_llm.clone().unwrap(),
            &sopts,
        )
        .await?;
        result.insert("structure".into(), tree.lock().unwrap().to_value());
        result.insert("optimize".into(), Value::Object(report));
        return Ok(result);
    }
    if opts.optimize != OptimizeMode::Off {
        let llm = if do_expand {
            optimize_llm.as_deref()
        } else {
            None
        };
        let report = optimize_async(&tree, &pages, do_expand, llm, None, opts.concurrency).await?;
        result.insert("optimize".into(), Value::Object(report));
    }
    if opts.summary {
        summarize_tree(tree.clone(), pages, summary_llm.unwrap(), &sopts).await?;
    } else {
        tree.lock().unwrap().strip_internal_keys(); // summarize_tree does this on its way out
    }
    result.insert("structure".into(), tree.lock().unwrap().to_value());
    Ok(result)
}
