//! Rebuilding the stage-05 document state from the Python goldens, for parity runs until the
//! stage-05 port (classification, title, captions) is wired in.
//!
//! Stages 02-04 run in Rust from `01_spans.json`. The classification state
//! `find_section_openers` sees comes from `05a_pre_openers.json` (written by
//! `parity/dump_reference.py`; see [`load_pre_openers`]), the section openers are recomputed,
//! then the caption regions of `05_classified.json` are applied as `extract_toc` does, and the
//! result is checked against `05_classified.json`.

use std::path::Path;

use pi_core::PageSpans;
use pi_layout::model::block::dominant_style_of;
use pi_layout::phases::{Document, build_document};
use serde_json::Value;

use crate::dump;
use crate::heading_detection::find_section_openers;
use crate::model::{Doc, Node};

pub fn load(p: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))
}

/// Stages 02-04 from `01_spans.json`.
pub fn document_from_spans(dir: &Path) -> Result<Document, String> {
    let spans = load(&dir.join("01_spans.json"))?;
    let pages: Vec<PageSpans> =
        serde_json::from_value(spans["data"].clone()).map_err(|e| format!("01_spans: {e}"))?;
    Ok(build_document(pi_layout::dump::process_document(&pages)))
}

fn u8_of(v: &Value) -> u8 {
    v.as_u64().unwrap_or(0) as u8
}

/// Overwrite per-block classification fields and page flags from a golden `pages` list
/// (`05a_pre_openers` or `05_classified` shape), then recompute each page's body style hashes
/// the way `extract_toc` step 6 fills them.
pub fn apply_classification(doc: &Document, pages: &Value) -> Result<(), String> {
    let pages = pages.as_array().ok_or("pages: not an array")?;
    if pages.len() != doc.pages.len() {
        return Err(format!("page count {} vs {}", pages.len(), doc.pages.len()));
    }
    for (pg, gp) in doc.pages.iter().zip(pages) {
        let blocks = gp["blocks"].as_array().ok_or("blocks: not an array")?;
        if blocks.len() != pg.output.len() {
            return Err(format!(
                "page {}: block count {} vs {}",
                pg.index(),
                blocks.len(),
                pg.output.len()
            ));
        }
        pg.has_caption
            .set(gp["has_caption"].as_bool().unwrap_or(false));
        pg.title_or_refs
            .set(gp["title_or_refs"].as_bool().unwrap_or(false));
        pg.has_body.set(gp["has_body"].as_bool().unwrap_or(false));
        let mut hashes = pg.body_style_hashes.borrow_mut();
        hashes.clear();
        for (&id, gb) in pg.output.iter().zip(blocks) {
            let b = &pg.blocks[id];
            b.kind.set(u8_of(&gb["type"]));
            b.is_body_paragraph
                .set(gb["is_body_paragraph"].as_bool().unwrap_or(false));
            b.used_as_heading
                .set(gb["used_as_heading"].as_bool().unwrap_or(false));
            b.caption_claimed
                .set(gb["caption_claimed"].as_bool().unwrap_or(false));
            b.caption_label.set(u8_of(&gb["caption_label"]));
            b.region_label.set(u8_of(&gb["region_label"]));
            if b.is_body_paragraph.get() {
                hashes.insert(dominant_style_of(b).to_string());
            }
        }
    }
    Ok(())
}

/// Best-effort pre-opener state when `05a_pre_openers.json` is absent: `05_classified` with
/// the effects of `find_section_openers` and `build_caption_regions` undone (opener blocks back
/// to type 0 and unused; no region labels or claimed captions, which only caption regions set).
/// Blocks the openers pass retyped 12 as duplicates cannot be told apart and stay 12.
fn guess_pre_openers(doc: &Document, classified: &Value) -> Result<(), String> {
    apply_classification(doc, &classified["pages"])?;
    for pg in &doc.pages {
        for &id in &pg.output {
            let b = &pg.blocks[id];
            b.region_label.set(0);
            b.caption_claimed.set(false);
            b.used_as_heading.set(false);
        }
    }
    for n in classified["section_openers"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let r = &n["heading"]["block"];
        let (p, o) = (
            r[0].as_u64().unwrap_or(0) as usize,
            r[1].as_u64().unwrap_or(0) as usize,
        );
        if let Some(pg) = doc.pages.get(p.wrapping_sub(1))
            && let Some(&id) = pg.output.get(o)
        {
            pg.blocks[id].kind.set(0);
        }
    }
    Ok(())
}

/// Stage-05 state for stages 06-08 plus the recomputed section openers. Returns the openers and
/// a list of divergences from `05_classified.json` (openers and final block state).
pub fn classified_document(dir: &Path) -> Result<(Document, Vec<Node>, Vec<String>), String> {
    let doc = document_from_spans(dir)?;
    let classified = load(&dir.join("05_classified.json"))?;
    let classified = &classified["data"];
    let pre_path = dir.join("05a_pre_openers.json");
    let mut notes = Vec::new();
    let title_page;
    if pre_path.exists() {
        let pre = load(&pre_path)?;
        apply_classification(&doc, &pre["data"]["pages"])?;
        title_page = pre["data"]["title_page"].as_u64().unwrap_or(0) as usize;
    } else {
        guess_pre_openers(&doc, classified)?;
        title_page = classified["title_page"].as_u64().unwrap_or(0) as usize;
        notes.push("no 05a_pre_openers.json: pre-opener state guessed from 05_classified".into());
    }
    let d = Doc::new(&doc);
    let openers = find_section_openers(d, title_page);
    let got = dump::outline(d, &openers);
    if let Some(diff) = first_diff(
        "section_openers",
        &classified["section_openers"],
        &got,
        1e-9,
    ) {
        notes.push(format!("05 {diff}"));
    }
    // build_caption_regions: the head block takes its caption label as region label, and the
    // body blocks are claimed.
    for r in classified["caption_regions"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let at = |v: &Value| -> Option<&pi_layout::model::block::Block> {
            let pg = doc.pages.get((v[0].as_u64()? as usize).wrapping_sub(1))?;
            Some(&pg.blocks[*pg.output.get(v[1].as_u64()? as usize)?])
        };
        if let Some(h) = at(&r["head"]) {
            h.region_label.set(h.caption_label.get());
        }
        for b in r["body"].as_array().into_iter().flatten() {
            if let Some(b) = at(b) {
                b.caption_claimed.set(true);
            }
        }
    }
    // Check the rebuilt state against 05_classified, then adopt the golden state so later
    // stages start from the reference's.
    'check: for (pg, gp) in doc
        .pages
        .iter()
        .zip(classified["pages"].as_array().into_iter().flatten())
    {
        for key in ["has_caption", "title_or_refs", "has_body"] {
            let ours = match key {
                "has_caption" => pg.has_caption.get(),
                "title_or_refs" => pg.title_or_refs.get(),
                _ => pg.has_body.get(),
            };
            if gp[key].as_bool() != Some(ours) {
                notes.push(format!("05 page {}: {key} differs", pg.index()));
                break 'check;
            }
        }
        for (&id, gb) in pg
            .output
            .iter()
            .zip(gp["blocks"].as_array().into_iter().flatten())
        {
            let b = &pg.blocks[id];
            let ours = [
                ("type", Value::from(b.kind.get())),
                ("is_body_paragraph", Value::from(b.is_body_paragraph.get())),
                ("used_as_heading", Value::from(b.used_as_heading.get())),
                ("caption_claimed", Value::from(b.caption_claimed.get())),
                ("caption_label", Value::from(b.caption_label.get())),
                ("region_label", Value::from(b.region_label.get())),
            ];
            for (k, v) in ours {
                if gb[k] != v {
                    notes.push(format!(
                        "05 page {} block {}: {k} want {} got {v}",
                        pg.index(),
                        b.orig_index.get(),
                        gb[k]
                    ));
                    break 'check;
                }
            }
        }
    }
    apply_classification(&doc, &classified["pages"])?;
    Ok((doc, openers, notes))
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    a == b || (a - b).abs() <= tol * a.abs().max(b.abs())
}

/// First divergence between `want` (golden) and `got`: ints, strings and bools exactly,
/// floats within `tol` relative; "inf"/"nan" strings compare as strings.
pub fn first_diff(path: &str, want: &Value, got: &Value, tol: f64) -> Option<String> {
    match (want, got) {
        (Value::Number(x), Value::Number(y)) => {
            let same = if x.is_f64() || y.is_f64() {
                close(
                    x.as_f64().unwrap_or(f64::NAN),
                    y.as_f64().unwrap_or(f64::NAN),
                    tol,
                )
            } else {
                x.as_i64() == y.as_i64() && x.as_u64() == y.as_u64()
            };
            (!same).then(|| format!("{path}: want {x} got {y}"))
        }
        (Value::Array(xs), Value::Array(ys)) => {
            for (i, (x, y)) in xs.iter().zip(ys).enumerate() {
                if let Some(d) = first_diff(&format!("{path}[{i}]"), x, y, tol) {
                    return Some(d);
                }
            }
            (xs.len() != ys.len())
                .then(|| format!("{path}: length want {} got {}", xs.len(), ys.len()))
        }
        (Value::Object(xs), Value::Object(ys)) => {
            for (k, x) in xs {
                match ys.get(k) {
                    Some(y) => {
                        if let Some(d) = first_diff(&format!("{path}.{k}"), x, y, tol) {
                            return Some(d);
                        }
                    }
                    None => return Some(format!("{path}.{k}: missing")),
                }
            }
            ys.keys()
                .find(|k| !xs.contains_key(*k))
                .map(|k| format!("{path}.{k}: unexpected"))
        }
        _ => (want != got).then(|| format!("{path}: want {want} got {got}")),
    }
}
