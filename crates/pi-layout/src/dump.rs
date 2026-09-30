//! Parity serialization of stages 02-03, mirroring `parity/dump_reference.py` (`line_d`,
//! `page_stats_d`, the `02_lines` / `03_columns` payloads) and its file wrapper.

use pi_core::{PageSpans, Rect};
use serde_json::{Value, json};

use crate::heading_detection::{HeadingCandidate, OutlineNode};
use crate::model::block::{Block, block_text, heading_score};
use crate::model::span_line::peek_text_of_line;
use crate::phases::{
    BlockRef, Classified, DocPage, Document, PageLayout, page_bbox_from_viewbox, process_page,
};

/// `dump_reference.py::num`: non-finite floats become "inf" / "-inf" / "nan".
pub fn num(x: f64) -> Value {
    if x.is_nan() {
        json!("nan")
    } else if x.is_infinite() {
        json!(if x > 0.0 { "inf" } else { "-inf" })
    } else {
        json!(x)
    }
}

/// `dump_reference.py::rect`: `[left, right, top, bottom]`.
pub fn rect(r: &Rect) -> Value {
    json!([num(r.left), num(r.right), num(r.top), num(r.bottom)])
}

/// One page of `02_lines` (`{"page", "page_bbox", "stats", "lines"}`).
pub fn page_lines(p: &PageLayout) -> Value {
    let lines: Vec<Value> = p
        .lines
        .iter()
        .map(|l| {
            json!({
                "bbox": rect(&l.bbox),
                "text": peek_text_of_line(l, &p.spans),
                "spans": l.spans,
                "column": l.column,
                "bold_frac": num(l.bold_frac),
                "italic_frac": num(l.italic_frac),
                "skew_frac": num(l.skew_frac),
                "avg_font_size": num(l.avg_font_size),
                "max_span_height": num(l.max_span_height),
                "numbering_kind": l.numbering_kind,
                "numbering_text": l.numbering_text,
                "ink_density": num(l.ink_density),
            })
        })
        .collect();
    json!({
        "page": p.page,
        "page_bbox": rect(&p.bounds),
        "stats": serde_json::to_value(&p.stats).expect("page stats"),
        "lines": lines,
    })
}

/// One page of `03_columns` (`{"page", "columns"}`).
pub fn page_columns(p: &PageLayout) -> Value {
    json!({"page": p.page, "columns": p.columns.iter().map(rect).collect::<Vec<_>>()})
}

/// Runs stages 02-03 over a document's `01_spans` pages, as `flash/main.py::extract_toc` does.
pub fn process_document(pages: &[PageSpans]) -> Vec<PageLayout> {
    pages
        .iter()
        .map(|p| {
            let vb = p.viewbox.expect("reference requires a view box");
            let mut layout = process_page(&p.spans, p.page, page_bbox_from_viewbox(vb, p.rotation));
            layout.viewport_box = Some(vb);
            layout.rot = p.rotation;
            layout
        })
        .collect()
}

/// The stage file wrapper written by `dump_reference.py::dump`.
pub fn wrap(doc: &str, stage: &str, data: Value) -> Value {
    json!({
        "schema": 1,
        "reference": "619cbd8",
        "producer": "pi-layout",
        "unicode": pi_pycompat::unicode::UNIDATA_VERSION,
        "doc": doc,
        "stage": stage,
        "data": data,
    })
}

/// `dump_reference.py::block_d` (`full` adds the stage 05 classification fields).
pub fn block(b: &Block, page: &DocPage, full: bool) -> Value {
    let mut d = json!({
        "bbox": rect(&b.bbox),
        "text": block_text(b, &page.layout),
        "lines": b.lines,
        "orig_index": b.orig_index.get(),
        "reading_order_index": b.reading_order_index.get(),
        "isolated_centered": b.isolated_centered.get(),
        "center_aligned": b.center_aligned,
        "bold_frac": num(b.bold_frac),
        "italic_frac": num(b.italic_frac),
        "weighted_skew": num(b.weighted_skew),
        "weighted_font_size": num(b.weighted_font_size),
        "density_chars": num(b.density_chars),
        "density_area": num(b.density_area),
        "max_line_height": num(b.max_line_height),
    });
    if full {
        let m = d.as_object_mut().expect("object");
        m.insert("type".into(), json!(b.kind.get()));
        m.insert("is_body_paragraph".into(), json!(b.is_body_paragraph.get()));
        m.insert("used_as_heading".into(), json!(b.used_as_heading.get()));
        m.insert("caption_claimed".into(), json!(b.caption_claimed.get()));
        m.insert("caption_label".into(), json!(b.caption_label.get()));
        m.insert("region_label".into(), json!(b.region_label.get()));
        m.insert("heading_score".into(), num(heading_score(b)));
    }
    d
}

/// The `04_blocks` payload.
pub fn blocks_stage(doc: &Document) -> Value {
    let pages: Vec<Value> = doc
        .pages
        .iter()
        .map(|p| {
            json!({
                "page": p.layout.page,
                "reading_order": p.reading.iter().map(|&id| p.blocks[id].orig_index.get()).collect::<Vec<_>>(),
                "blocks": p.output.iter().map(|&id| block(&p.blocks[id], p, false)).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({"doc_stats": serde_json::to_value(&doc.stats).expect("doc stats"), "pages": pages})
}

/// `[page (1-based), orig_index]`, the reference's block registry entry.
pub fn block_ref(doc: &Document, r: BlockRef) -> Value {
    json!([
        doc.pages[r.0].index(),
        doc.pages[r.0].blocks[r.1].orig_index.get()
    ])
}

/// Numbering values are ints in the reference whenever integral (`int(...)`), else floats.
fn number_value(v: f64) -> Value {
    if v.is_finite() && v == v.trunc() && v.abs() < 1e15 {
        json!(v as i64)
    } else {
        num(v)
    }
}

/// `dump_reference.py::candidate_d`.
pub fn candidate(doc: &Document, c: &HeadingCandidate) -> Value {
    json!({
        "type": c.kind,
        "page": doc.pages[c.page].index(),
        "block": block_ref(doc, (c.page, c.block)),
        "anchor": c.anchor.map(|a| block_ref(doc, (c.page, a))),
        "numbering": c.numbering.iter().map(|&n| number_value(n)).collect::<Vec<_>>(),
        "prefix": c.prefix.as_ref().map(|t| t.to_string_py()),
        "title": c.title.as_ref().map(|t| t.to_string_py()),
        "has_numbering": c.has_numbering,
        "is_prominent": c.is_prominent,
        "script": c.script,
        "y_frac": num(c.y_frac),
        "heading_score": num(heading_score(c.block(doc))),
    })
}

/// `dump_reference.py::outline_d`.
pub fn outline(doc: &Document, nodes: &[OutlineNode]) -> Value {
    Value::Array(
        nodes
            .iter()
            .map(|n| json!({"heading": candidate(doc, &n.heading), "children": outline(doc, &n.children)}))
            .collect(),
    )
}

/// The `05_classified` payload.
pub fn classified_stage(doc: &Document, c: &Classified) -> Value {
    json!({
        "doc_title": c.doc_title,
        "title_page": c.title_page,
        "title_blocks": c.title_blocks.iter().map(|&r| block_ref(doc, r)).collect::<Vec<_>>(),
        "caption_regions": c.caption_regions.iter().map(|r| json!({
            "head": block_ref(doc, (r.page, r.head)),
            "body": r.body.iter().map(|&b| block_ref(doc, (r.page, b))).collect::<Vec<_>>(),
            "type": r.kind,
            "score": num(r.score),
        })).collect::<Vec<_>>(),
        "section_openers": outline(doc, &c.section_openers),
        "pages": doc.pages.iter().map(|p| json!({
            "page": p.index(),
            "has_caption": p.has_caption.get(),
            "title_or_refs": p.title_or_refs.get(),
            "has_body": p.has_body.get(),
            "blocks": p.output.iter().map(|&id| block(&p.blocks[id], p, true)).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}
