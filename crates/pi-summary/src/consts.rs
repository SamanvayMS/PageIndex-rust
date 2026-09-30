//! Reference constants (VectifyAI/PageIndex @619cbd8).

/// Simultaneous summary model calls. ref: pageindex/utils.py:764
pub const SUMMARY_CONCURRENCY: usize = 64;
/// Leaves under this many tokens reuse their raw text as the summary. ref: pageindex/utils.py:765
pub const SUMMARY_RAW_TEXT_TOKENS: usize = 200;
/// Cap on leading pages fed into a parent summary. ref: pageindex/utils.py:766
pub const SUMMARY_INTRO_MAX_PAGES: usize = 3;
/// Word cap the summary prompts ask for. ref: pageindex/utils.py:767
pub const SUMMARY_MAX_WORDS: usize = 150;
