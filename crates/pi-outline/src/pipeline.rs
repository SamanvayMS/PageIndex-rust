//! Steps 11 of `flash/main.py::extract_toc`: outline assembly, the validity gates and the dict
//! tree, on a document classified through stage 05.

use serde_json::Value;

use crate::consts::{MAX_GAP_MIN_PAGES, MAX_GAP_PAGE_FRACTION};
use crate::model::{Cand, Doc, Node};
use crate::outline_assembly::{
    assemble_outline, compute_max_heading_gap, has_table_or_prominent, is_chapter_outline_valid,
    is_outline_valid, mark_outline_block_types, outline_to_dict_tree,
};

/// Outcome of the outline gate (`07_outline.gate`).
#[derive(Debug, Clone, PartialEq)]
pub enum Gate {
    Valid {
        chapter_valid: bool,
    },
    Invalid {
        /// `compute_max_heading_gap(...)["max_gap"]` for documents of 3+ pages; `Some(None)`
        /// is the reference's int `0`.
        max_gap: Option<Option<f64>>,
    },
}

/// Everything stages 06-08 produce.
pub struct OutlineOutput {
    /// `build_doc_heading_candidates` result (06).
    pub candidates: Vec<Cand>,
    /// `assemble_outline` result before the gates.
    pub assembled: Vec<Node>,
    pub gate: Gate,
    /// Outline after the gates.
    pub final_nodes: Vec<Node>,
    pub has_abstract_or_references: bool,
    /// `outline_to_dict_tree` (08); empty when the outline is.
    pub tree: Vec<Value>,
}

/// Section openers found by the stage-05 pass of `pi_layout` as outline nodes of this crate
/// (candidates are immutable and gain identity here; the openers have no children yet).
pub fn openers_from_layout(nodes: &[pi_layout::heading_detection::OutlineNode]) -> Vec<Node> {
    nodes
        .iter()
        .map(|n| {
            let h = &n.heading;
            crate::model::new_node(std::rc::Rc::new(crate::model::HeadingCandidate {
                kind: h.kind,
                page: h.page,
                block: h.block,
                anchor: h.anchor,
                numbering: h.numbering.clone(),
                prefix: h.prefix.clone(),
                title: h.title.clone(),
                has_numbering: h.has_numbering,
                is_prominent: h.is_prominent,
                script: h.script,
                y_frac: h.y_frac,
            }))
        })
        .collect()
}

/// Stages 05-08 on a document built by `pi_layout::phases::build_document`: classification
/// (`pi_layout::phases::classify_document`), then [`extract_outline`].
// ref: main.py::extract_toc (steps 5-11)
pub fn classify_and_outline(
    doc: &pi_layout::phases::Document,
) -> (pi_layout::phases::Classified, OutlineOutput) {
    let classified = pi_layout::phases::classify_document(doc);
    let openers = openers_from_layout(&classified.section_openers);
    let out = extract_outline(Doc::new(doc), openers);
    (classified, out)
}

/// Outline assembly and validation (`extract_toc` step 11).
// ref: main.py::extract_toc (step 11)
pub fn extract_outline(d: Doc, mut section_openers: Vec<Node>) -> OutlineOutput {
    let mut candidates = Vec::new();
    let mut sink = |c: &[Cand]| candidates = c.to_vec();
    let mut nodes = assemble_outline(d, &mut section_openers, Some(&mut sink));
    let assembled = nodes.clone();
    let gate;
    let has_abstract_or_references;
    if is_outline_valid(d, &nodes) {
        let chapter_valid = is_chapter_outline_valid(d, &nodes);
        if !chapter_valid {
            nodes = Vec::new();
        }
        gate = Gate::Valid { chapter_valid };
        has_abstract_or_references = false;
    } else {
        mark_outline_block_types(d, &nodes);
        let page_count = d.pages().len();
        let mut max_gap = None;
        if page_count >= MAX_GAP_MIN_PAGES {
            let g = compute_max_heading_gap(d, &nodes, 1.0).0;
            max_gap = Some(g);
            if g.unwrap_or(0.0) > MAX_GAP_PAGE_FRACTION * page_count as f64 {
                nodes = Vec::new();
            }
        }
        gate = Gate::Invalid { max_gap };
        has_abstract_or_references = has_table_or_prominent(&nodes);
    }
    let tree = if nodes.is_empty() {
        Vec::new()
    } else {
        outline_to_dict_tree(d, &nodes, d.pages().len())
    };
    OutlineOutput {
        candidates,
        assembled,
        gate,
        final_nodes: nodes,
        has_abstract_or_references,
        tree,
    }
}
