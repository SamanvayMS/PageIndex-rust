//! Caption detection, region growth and deduplication.
//!
//! ref: pageindex/flash/labels/caption_regions.py

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use pi_core::Rect;
use pi_pycompat::pymath;

use super::caption_text::{
    PERIOD_CHARS, REFERENCE_PHRASE_TRIE, extract_structural_number, format_caption_label,
    is_uppercase_dominant, token_case_signal,
};
use crate::classification::keyword_tables::{
    CHART_KEYWORDS_TRIE, FIGURE_KEYWORDS_TRIE, TABLE_KEYWORDS_TRIE,
};
use crate::model::block::{Block, BlockId, heading_score};
use crate::model::numbering::numbering_ro;
use crate::model::rects::{Bounded, extend_bottom_to, extend_top_to, rect_union};
use crate::phases::{BlockRef, DocPage, Document};
use crate::tokens::{TokenView, strip_leading_if_in, tokenize_block};

/// ref: labels/caption_regions.py::CaptionEntry
#[derive(Debug, Clone)]
pub struct CaptionEntry {
    /// e.g. "F3", "T2.1". ref slot: `primary_slot`
    pub label: String,
    /// 4 figure, 5 table, 11 chart.
    pub kind: u8,
    /// 1-based page number.
    pub page_index: usize,
    /// ref slot: `group_slot`
    pub block: BlockRef,
    /// Tokens after the keyword and number. ref slot: `secondary_slot`
    pub remainder: TokenView,
}

/// ref: labels/caption_regions.py::CaptionContext
#[derive(Debug, Default)]
pub struct CaptionContext {
    /// ref slot: `auxiliary_slot`
    pub entries: Vec<CaptionEntry>,
    /// Some entry carries a structural number. ref slot: `state_slot`
    pub has_numbered: bool,
    /// Page -> reading indices of caption heads. ref slot: `tertiary_slot`
    heads_by_page: HashMap<usize, HashSet<usize>>,
    /// Reading indices claimed on the current page. ref slot: `secondary_slot`
    claimed: HashSet<usize>,
}

/// ref: labels/caption_regions.py::CaptionedRegion
#[derive(Debug, Clone)]
pub struct CaptionedRegion {
    pub bbox: Rect,
    /// 0-based page position.
    pub page: usize,
    /// ref slot: `primary_slot`
    pub head: BlockId,
    /// ref slot: `output_slot`
    pub body: Vec<BlockId>,
    pub kind: u8,
    pub score: f64,
}

impl Bounded for CaptionedRegion {
    fn rect(&self) -> &Rect {
        &self.bbox
    }
}

// ref: labels/caption_regions.py::CaptionedRegion.__init__ (score formula)
fn make_region(
    doc: &Document,
    pi0: usize,
    head: BlockId,
    bbox: Rect,
    body: Vec<BlockId>,
    next: Option<BlockId>,
    flag: bool,
) -> CaptionedRegion {
    let page = &doc.pages[pi0];
    let pl = &page.layout;
    let kind = page.blocks[head].caption_label.get();
    let mut r = CaptionedRegion {
        bbox,
        page: pi0,
        head,
        body,
        kind,
        score: 0.0,
    };
    let parea = pl.bounds.area();
    let mut area_pct = if parea > 0.0 {
        100.0 * r.area() / parea
    } else {
        0.0
    };
    let score = if area_pct <= 0.0 {
        0.0
    } else {
        if let Some(n) = next.map(|id| &page.blocks[id])
            && n.top_edge() < r.top_edge()
            && n.right_edge() > r.left_edge()
            && flag
        {
            area_pct /= 5.0;
        }
        if kind == 4 {
            let mut inner = 0.0;
            for &b in &r.body {
                let b = &page.blocks[b];
                if b.weighted_skew > 1.0 {
                    continue;
                }
                inner += b.area();
            }
            if r.area() > 0.0 {
                area_pct * pymath::max(0.1, 1.0 - inner / r.area())
            } else {
                0.0
            }
        } else {
            let mut count = 1.0;
            for &b in &r.body {
                for &lid in &page.blocks[b].lines {
                    for &sid in &pl.lines[lid].spans {
                        count += pl.spans[sid].char_count() as f64;
                    }
                }
            }
            count * area_pct
        }
    };
    r.score = score;
    r
}

fn block_of(doc: &Document, r: BlockRef) -> &Block {
    &doc.pages[r.0].blocks[r.1]
}

// ref: labels/caption_text.py::caption_outranks
fn caption_outranks(doc: &Document, a: &CaptionEntry, b: &CaptionEntry) -> bool {
    let (pa, pb) = (&doc.pages[a.block.0], &doc.pages[b.block.0]);
    let (ba, bb) = (block_of(doc, a.block), block_of(doc, b.block));
    let ua = is_uppercase_dominant(&tokenize_block(ba, &pa.layout));
    let ub = is_uppercase_dominant(&tokenize_block(bb, &pb.layout));
    if ua != ub {
        return ua;
    }
    let ga = token_case_signal(a.remainder.first());
    let gb = token_case_signal(b.remainder.first());
    if ga != gb {
        return ga > gb;
    }
    if a.page_index != b.page_index {
        return a.page_index < b.page_index;
    }
    ba.reading_order_index.get() < bb.reading_order_index.get()
}

/// Figure / table / chart labels; records entries and sets the head blocks' caption label.
// ref: labels/caption_regions.py::detect_captions
pub fn detect_captions(doc: &Document, ctx: &mut CaptionContext) {
    for (pi0, page) in doc.pages.iter().enumerate() {
        for &bid in &page.reading {
            let b = &page.blocks[bid];
            if b.kind.get() != 0 {
                continue;
            }
            let toks = tokenize_block(b, &page.layout);
            let (kind, prefix) = if let Some(p) = FIGURE_KEYWORDS_TRIE.prefix_match(&toks) {
                (4, p)
            } else if let Some(p) = TABLE_KEYWORDS_TRIE.prefix_match(&toks) {
                (5, p)
            } else if let Some(p) = CHART_KEYWORDS_TRIE.prefix_match(&toks) {
                (11, p)
            } else {
                continue;
            };
            let mut rem = strip_leading_if_in(&toks.from(prefix.length), &PERIOD_CHARS);
            let number = extract_structural_number(&rem, true);
            let label = format_caption_label(kind, number.as_ref());
            if let Some(n) = &number {
                ctx.has_numbered = true;
                rem = rem.from(n.length);
            }
            if REFERENCE_PHRASE_TRIE.prefix_match(&rem).is_some() {
                continue;
            }
            page.has_caption.set(true);
            ctx.entries.push(CaptionEntry {
                label,
                kind,
                page_index: page.index(),
                block: (pi0, bid),
                remainder: rem,
            });
            b.caption_label.set(kind);
        }
    }
}

// ref: labels/caption_regions.py::dedupe_caption_entries
fn dedupe_caption_entries(doc: &Document, ctx: &CaptionContext) -> Vec<CaptionEntry> {
    if !ctx.has_numbered {
        return ctx.entries.clone();
    }
    let mut by_label: IndexMap<String, CaptionEntry> = IndexMap::new();
    for c in &ctx.entries {
        if c.label.chars().count() <= 1 {
            continue;
        }
        let replace = match by_label.get(&c.label) {
            None => true,
            Some(e) => caption_outranks(doc, c, e),
        };
        if replace {
            by_label.insert(c.label.clone(), c.clone());
        }
    }
    let mut out: Vec<CaptionEntry> = by_label.into_values().collect();
    out.sort_by_key(|c| {
        (
            c.page_index,
            block_of(doc, c.block).reading_order_index.get(),
        )
    });
    out
}

fn column_of(page: &DocPage, b: &Block) -> i32 {
    b.lines.first().map_or(-1, |&l| page.layout.lines[l].column)
}

// ref: labels/caption_regions.py::extend_caption_region
fn extend_caption_region(
    doc: &Document,
    ctx: &CaptionContext,
    entry: &CaptionEntry,
    prior: &[CaptionedRegion],
    page_set: Option<&HashSet<usize>>,
    dir: i64,
) -> Option<CaptionedRegion> {
    let pi0 = entry.page_index - 1;
    let page = &doc.pages[pi0];
    let pl = &page.layout;
    let origin_id = entry.block.1;
    let origin = &page.blocks[origin_id];
    let anchor = if dir > 0 {
        origin.bottom_edge()
    } else {
        origin.top_edge()
    };
    let mut bbox = Rect::new(origin.left_edge(), origin.right_edge(), anchor, anchor);
    let mut blocks: Vec<BlockId> = Vec::new();
    let sorted = &page.reading;
    let n = sorted.len() as i64;
    let mut index = origin.reading_order_index.get() as i64 + dir;
    let mut previous_id = origin_id;
    let origin_col = column_of(page, origin);
    let cols = &pl.columns;
    let col_at = |i: i64| (i >= 0 && (i as usize) < cols.len()).then(|| cols[i as usize]);

    while 0 <= index && index < n {
        let cid = sorted[index as usize];
        let caption = &page.blocks[cid];
        let cc = column_of(page, caption);
        if cc < 0 {
            break;
        }
        if dir < 0 && cc < origin_col && caption.bottom_edge() < anchor {
            let mut ci = cc as i64 - 1;
            let mut cr = col_at(ci);
            while let Some(c) = cr {
                if !(c.bottom < bbox.top || c.right < bbox.left || c.left > bbox.right) {
                    break;
                }
                ci -= 1;
                cr = col_at(ci);
            }
            bbox = extend_top_to(&bbox, cr.map_or(pl.bounds.top, |c| c.bottom));
            break;
        }
        if dir > 0 && cc > origin_col && caption.top_edge() > anchor {
            let mut ci = cc as i64 + 1;
            let mut cr = col_at(ci);
            while let Some(c) = cr {
                if !(c.top > bbox.bottom || c.right < bbox.left || c.left > bbox.right) {
                    break;
                }
                ci += 1;
                cr = col_at(ci);
            }
            bbox = extend_bottom_to(&bbox, cr.map_or(pl.bounds.bottom, |c| c.top));
            break;
        }
        bbox = if dir < 0 {
            extend_top_to(&bbox, caption.bottom_edge())
        } else {
            extend_bottom_to(&bbox, caption.top_edge())
        };
        if caption.kind.get() != 0 || ctx.claimed.contains(&caption.reading_order_index.get()) {
            break;
        }
        if page_set.is_some_and(|s| s.contains(&(index as usize))) {
            break;
        }
        let size = pymath::min(doc.stats.body_font_size, origin.weighted_font_size);
        if caption.is_body_paragraph.get()
            && caption.weighted_font_size > pymath::min(0.9 * size, size - 1.5)
        {
            break;
        }
        let next = sorted.get(index as usize + 1).map(|&id| &page.blocks[id]);
        let previous = &page.blocks[previous_id];
        let gap = if dir > 0 {
            previous.bottom_edge() - caption.top_edge()
        } else {
            0.0
        };
        let line_gap = pl.stats.median_overlap_gap - pl.stats.median_font_size;
        if dir > 0
            && let Some(nb) = next
            && caption.line_count() <= 4
            && caption.char_stats.first_cat != 3
            && gap > line_gap
            && (previous_id == origin_id
                || gap > pymath::min(3.0 * line_gap, caption.bottom_edge() - nb.top_edge()))
        {
            let next2 = sorted.get(index as usize + 2).map(|&id| &page.blocks[id]);
            if heading_score(caption) >= heading_score(previous) + 0.5
                && (nb.is_body_paragraph.get() || next2.is_some_and(|x| x.is_body_paragraph.get()))
            {
                break;
            }
            let (_, line_text) = numbering_ro(&pl.lines[caption.first_line()], &pl.spans);
            if !line_text.is_empty()
                && caption.char_stats.first_cat == 2
                && heading_score(caption) >= size
                && gap > 2.0 * caption.weighted_font_size
                && caption.char_count() as i64 - line_text.chars().count() as i64 > 2
            {
                break;
            }
        }
        blocks.push(cid);
        bbox = rect_union(&bbox, &caption.bbox);
        index += dir;
        previous_id = cid;
    }
    if dir < 0 && index < 0 {
        bbox = extend_top_to(&bbox, pl.bounds.top);
    } else if dir > 0 && index >= n {
        bbox = extend_bottom_to(&bbox, pl.bounds.bottom);
    }
    if dir < 0 {
        let mut fwd = origin.reading_order_index.get() + 1;
        while fwd < sorted.len() {
            let b = &page.blocks[sorted[fwd]];
            let (cx, cy) = (b.center_x(), b.center_y());
            if cx < bbox.left || cx > bbox.right || cy < bbox.bottom || cy > bbox.top {
                break;
            }
            blocks.push(sorted[fwd]);
            bbox = rect_union(&bbox, &b.bbox);
            fwd += 1;
        }
    }
    let area = bbox.area();
    if area <= 0.0 {
        return None;
    }
    for p in prior {
        let ov = pymath::max(
            0.0,
            pymath::min(bbox.right, p.bbox.right) - pymath::max(bbox.left, p.bbox.left),
        ) * pymath::max(
            0.0,
            pymath::min(bbox.top, p.bbox.top) - pymath::max(bbox.bottom, p.bbox.bottom),
        );
        if ov >= 0.25 * pymath::min(area, p.area()) {
            return None;
        }
    }
    let next = (0 <= index && index < n).then(|| sorted[index as usize]);
    let on_page_set = page_set.is_some_and(|s| index >= 0 && s.contains(&(index as usize)));
    Some(make_region(
        doc,
        pi0,
        origin_id,
        bbox,
        blocks,
        next,
        on_page_set,
    ))
}

/// Extends each deduplicated entry backward and forward and keeps the higher-scoring region.
// ref: labels/caption_regions.py::build_caption_regions
pub fn build_caption_regions(doc: &Document, ctx: &mut CaptionContext) -> Vec<CaptionedRegion> {
    ctx.heads_by_page.clear();
    ctx.claimed.clear();
    let entries = dedupe_caption_entries(doc, ctx);
    for c in &entries {
        ctx.heads_by_page
            .entry(c.page_index)
            .or_default()
            .insert(block_of(doc, c.block).reading_order_index.get());
    }
    let mut out = Vec::new();
    let mut page = 0;
    let mut prior: Vec<CaptionedRegion> = Vec::new();
    for e in &entries {
        if e.page_index != page {
            prior.clear();
            ctx.claimed.clear();
            page = e.page_index;
        }
        if prior.len() >= 8 {
            continue;
        }
        let page_set = ctx.heads_by_page.get(&e.page_index).cloned();
        let back = extend_caption_region(doc, ctx, e, &prior, page_set.as_ref(), -1);
        let fwd = extend_caption_region(doc, ctx, e, &prior, page_set.as_ref(), 1);
        let winner = match (back, fwd) {
            (Some(b), f) if f.as_ref().is_none_or(|f| b.score > f.score) => Some(b),
            (_, f) => f,
        };
        if let Some(w) = winner {
            let pg = &doc.pages[w.page];
            for &b in &w.body {
                ctx.claimed.insert(pg.blocks[b].reading_order_index.get());
            }
            prior.push(w.clone());
            out.push(w);
        }
    }
    out
}
