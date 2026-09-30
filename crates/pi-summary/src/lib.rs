//! Bottom-up summary scheduler: a port of `SummaryScheduler` / `summarize_tree` /
//! `generate_doc_description` from `pageindex/utils.py` and of the model passes of
//! `flash/api.py::page_index_flash` (VectifyAI/PageIndex @619cbd8, MIT).

pub mod consts;
pub mod describe;
pub mod parse;
pub mod pipeline;
pub mod prompts;
pub mod pyrepr;
pub mod scheduler;

pub use describe::{clean_structure_for_description, generate_doc_description};
pub use pipeline::{
    FlashOptions, OptimizeMode, PipelineError, optimize_and_summarize, page_index_flash_post,
};
pub use scheduler::{SummaryError, SummaryOptions, SummaryScheduler, summarize_tree};
