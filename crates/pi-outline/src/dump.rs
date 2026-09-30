//! Parity serialization of stages 06-08, mirroring `parity/dump_reference.py`
//! (`candidate_d`, `outline_d`, the `06_candidates` / `07_outline` / `08_tree_raw` payloads).

use pi_layout::dump::num;
use pi_layout::model::block::heading_score;
use serde_json::{Value, json};

use crate::model::{Doc, HeadingCandidate, Node};
use crate::pipeline::{Gate, OutlineOutput};

/// `[page_index, orig_index]` of a block.
fn block_ref(d: Doc, page: usize, id: usize) -> Value {
    json!([d.page_index(page), d.block(page, id).orig_index.get()])
}

/// A numbering entry: ints when integral (the reference's Python ints).
fn number(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9.0e15 {
        json!(n as i64)
    } else {
        num(n)
    }
}

/// `dump_reference.py::candidate_d`.
pub fn candidate(d: Doc, c: &HeadingCandidate) -> Value {
    json!({
        "type": c.kind,
        "page": d.page_index(c.page),
        "block": block_ref(d, c.page, c.block),
        "anchor": c.anchor.map_or(Value::Null, |a| block_ref(d, c.page, a)),
        "numbering": c.numbering.iter().map(|&n| number(n)).collect::<Vec<_>>(),
        "prefix": c.prefix.as_ref().map_or(Value::Null, |t| json!(t.to_string_py())),
        "title": c.title.as_ref().map_or(Value::Null, |t| json!(t.to_string_py())),
        "has_numbering": c.has_numbering,
        "is_prominent": c.is_prominent,
        "script": c.script,
        "y_frac": num(c.y_frac),
        "heading_score": num(heading_score(d.cblock(c))),
    })
}

/// `dump_reference.py::outline_d`.
pub fn outline(d: Doc, nodes: &[Node]) -> Value {
    Value::Array(
        nodes
            .iter()
            .map(|n| json!({"heading": candidate(d, &n.heading), "children": outline(d, &n.children.borrow())}))
            .collect(),
    )
}

/// The `06_candidates` payload.
pub fn candidates_stage(d: Doc, out: &OutlineOutput) -> Value {
    Value::Array(out.candidates.iter().map(|c| candidate(d, c)).collect())
}

/// The `07_outline` payload. `assembled` must be serialized before the gates mutate block
/// types, which the payload does not read, so serializing afterwards is equivalent.
pub fn outline_stage(d: Doc, out: &OutlineOutput) -> Value {
    let gate = match &out.gate {
        Gate::Valid { chapter_valid } => {
            json!({"outline_valid": true, "chapter_valid": chapter_valid})
        }
        Gate::Invalid { max_gap: None } => json!({"outline_valid": false}),
        Gate::Invalid { max_gap: Some(g) } => {
            json!({"outline_valid": false, "max_gap": g.map_or(json!(0), num)})
        }
    };
    json!({
        "assembled": outline(d, &out.assembled),
        "gate": gate,
        "final": outline(d, &out.final_nodes),
        "has_abstract_or_references": out.has_abstract_or_references,
    })
}

/// The `08_tree_raw` payload.
pub fn tree_stage(out: &OutlineOutput) -> Value {
    Value::Array(out.tree.clone())
}

/// The stage file wrapper written by `dump_reference.py::dump`.
pub fn wrap(doc: &str, stage: &str, data: Value) -> Value {
    json!({
        "schema": 1,
        "reference": "619cbd8",
        "producer": "pi-outline",
        "unicode": pi_pycompat::unicode::UNIDATA_VERSION,
        "doc": doc,
        "stage": stage,
        "data": data,
    })
}
