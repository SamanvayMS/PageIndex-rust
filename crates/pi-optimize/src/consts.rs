//! Reference constants (VectifyAI/PageIndex @619cbd8).

/// Only look ahead (expand) on nodes larger than this many pages. ref: pageindex/tree_optimize.py:68
pub const TRIGGER_PAGES: i64 = 5;
/// R(v), the routing cost of visiting a node, in pages. ref: pageindex/tree_optimize.py:69
pub const ROUTING_COST: i64 = 1;
/// Ceiling on simultaneous expand calls. ref: pageindex/tree_optimize.py:70
pub const EXPAND_CONCURRENCY: usize = 32;
/// Per-page text handed to the model, in code points. ref: pageindex/tree_optimize.py:71
pub const PAGE_CHARS: usize = 6000;
/// A union title longer than this (code points) falls back to a page label.
/// ref: pageindex/tree_optimize.py:72
pub const TITLE_MAX_CHARS: usize = 200;
/// Default `max_rounds` of `optimize`. ref: pageindex/tree_optimize.py:792
pub const MAX_ROUNDS: u32 = 3;
/// Default `empty_retries` of `optimize`. ref: pageindex/tree_optimize.py:793
pub const EMPTY_RETRIES: u32 = 1;
/// Default `min_gain_ratio` of `optimize`. ref: pageindex/tree_optimize.py:791
pub const MIN_GAIN_RATIO: f64 = 0.0;
/// Width of relabelled node ids. ref: pageindex/tree_optimize.py:208
pub const NODE_ID_WIDTH: usize = 4;
/// Largest page-node fallback the managed pipelines accept as an index.
/// ref: pageindex/flash/api.py:16
pub const FLAT_TREE_MAX_NODES: usize = 10;

/// ref: pageindex/tree_optimize.py:74-92 (`EXPAND_PROMPT`, `{title}`/`{start}`/`{end}`/`{pages}`
/// placeholders, `{{`/`}}` already unescaped). MIT, Copyright (c) 2025 Vectify AI.
pub const EXPAND_PROMPT_HEAD: &str =
    "You are splitting an over-long section of a PDF into its subsections.\n\nSection title: ";
pub const EXPAND_PROMPT_TAIL: &str =
    "\n\nList the subsection headings that BEGIN within these pages, in document order,
each with the page number it begins on. Rules:

- Use only headings printed in the document. Never invent or paraphrase one.
- A running header, a table column label, a table row label, or a cross-reference
  is not a subsection heading.
- If this section is continuous prose, or a single table spanning the pages,
  return an empty list. That is a valid and expected answer.
- Do not include the section's own title.

Reply with JSON only:
{\"subsections\": [{\"title\": \"<verbatim heading>\", \"page\": <int>}]}";

/// `EXPAND_PROMPT.format(title=..., start=..., end=..., pages=...)`.
pub fn expand_prompt(title: &str, start: i64, end: i64, pages: &str) -> String {
    format!("{EXPAND_PROMPT_HEAD}{title}\nPages: {start}-{end}\n\n{pages}{EXPAND_PROMPT_TAIL}")
}
