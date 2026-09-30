//! Watermark, boilerplate, and TOC-range detection.
//!
//! ref: pageindex/flash/classification/toc_boilerplate.py

use indexmap::IndexMap;
use pi_pycompat::pymath;

use super::body_text::{is_body_paragraph, normalized_block_text};
use super::keyword_tables::{
    INSTITUTION_THESIS_TRIE, PROFESSOR_TITLES_TRIE, TOC_TITLES_TRIE, dot_leader_row, search_trie,
};
use crate::model::block::{Block, heading_score};
use crate::model::char_stats::{info_weight, letter_count};
use crate::model::numbering::to_number;
use crate::model::rects::Bounded;
use crate::model::span_line::peek_text_of_line;
use crate::phases::{BlockRef, DocPage, Document};
use crate::tokens::tokenize_block;

/// Side-rail skewed blocks recurring on 3+ pages become type 12.
// ref: classification/toc_boilerplate.py::mark_watermarks
pub fn mark_watermarks(doc: &Document) {
    let mut buckets: IndexMap<String, Vec<BlockRef>> = IndexMap::new();
    for (pi0, page) in doc.pages.iter().enumerate() {
        for &bid in &page.output {
            let b = &page.blocks[bid];
            if b.weighted_skew < 1.0 || letter_count(&b.char_stats) < 5 {
                continue;
            }
            let hx = b.center_x();
            let pw = page.layout.bounds.bbox_width();
            if 0.1 * pw < hx && hx < 0.9 * pw {
                continue;
            }
            buckets
                .entry(normalized_block_text(b, page))
                .or_default()
                .push((pi0, bid));
        }
    }
    for group in buckets.values() {
        if group.len() < 3 {
            continue;
        }
        for &(p, b) in group {
            doc.pages[p].blocks[b].kind.set(12);
        }
    }
}

// ref: classification/toc_boilerplate.py::is_boilerplate_block
pub fn is_boilerplate_block(block: &Block, page: &DocPage) -> bool {
    let mut toks = tokenize_block(block, &page.layout);
    if info_weight(&block.char_stats) >= 200.0 || toks.length >= 100 {
        return false;
    }
    if search_trie(&INSTITUTION_THESIS_TRIE, &toks).is_some() {
        return true;
    }
    while toks.length > 0 {
        let Some(first_line) = toks.token_at(0).and_then(|t| t.line()) else {
            break;
        };
        let mut line_end = 0;
        while line_end < toks.length {
            match toks.token_at(line_end) {
                Some(t) if t.line() == Some(first_line) => line_end += 1,
                _ => break,
            }
        }
        if PROFESSOR_TITLES_TRIE
            .prefix_match(&toks.slice(0, line_end))
            .is_none()
        {
            return false;
        }
        toks = toks.from(line_end);
    }
    true
}

/// ref: classification/toc_boilerplate.py::NumberColumnCluster
#[derive(Debug, Clone, Copy)]
struct NumberColumnCluster {
    anchor_x: f64,
    width: f64,
    first: i64,
    last: i64,
    length: i64,
    increasing: bool,
}

// ref: classification/toc_boilerplate.py::extract_number_column
fn extract_number_column(block: &Block, page: &DocPage) -> Option<NumberColumnCluster> {
    let (mut column, mut last, mut seq) = (0i64, 0i64, 0i64);
    let mk = |column, last, seq, inc| NumberColumnCluster {
        anchor_x: block.center_x(),
        width: block.bbox_width(),
        first: column,
        last,
        length: seq,
        increasing: inc,
    };
    for &lid in &block.lines {
        let n = to_number(&peek_text_of_line(
            &page.layout.lines[lid],
            &page.layout.spans,
        ));
        if n.is_nan() {
            return None;
        }
        if !(n > 0.0 && n < 1e6 && n == n.ceil()) || n >= 1e4 || last as f64 > n {
            return Some(mk(column, last, seq, false));
        }
        if column <= 0 {
            column = n as i64;
        }
        last = n as i64;
        seq += 1;
    }
    Some(mk(column, last, seq, true))
}

// ref: classification/toc_boilerplate.py::pick_nearer_cluster
fn pick_nearer_cluster(
    c: &NumberColumnCluster,
    pred: Option<usize>,
    succ: Option<usize>,
    clusters: &[NumberColumnCluster],
) -> Option<usize> {
    let d = pred.map_or(f64::INFINITY, |p| c.anchor_x - clusters[p].anchor_x);
    let e = succ.map_or(f64::INFINITY, |s| clusters[s].anchor_x - c.anchor_x);
    if d > c.width && e > c.width {
        return None;
    }
    if d < e { pred } else { succ }
}

/// Returns `(start_index, end_index)` of a TOC-like block range on the page.
// ref: classification/toc_boilerplate.py::detect_toc_range
fn detect_toc_range(
    doc: &Document,
    page: &DocPage,
    prev_toc_page: Option<usize>,
) -> Option<(i64, i64)> {
    let pl = &page.layout;
    let blocks = &page.output;
    let mut lines = 0;
    let mut dot_leader_blocks = 0;
    let mut weight = 0.0;
    let mut last_multiline: i64 = -1;
    let mut contents: i64 = -1;
    let mut pre_contents: i64 = -1;
    let mut last_toc: i64 = -1;
    let mut seen_body = false;
    let mut body_stop_y = pl.bounds.top_edge();
    let is_last_page = prev_toc_page.is_some_and(|p| p + 1 == page.index());
    let mut clusters: Vec<NumberColumnCluster> = Vec::new();

    for (cp, &bid) in blocks.iter().enumerate() {
        let cp = cp as i64;
        let block = &page.blocks[bid];
        let mut is_toc = false;
        for &lid in &block.lines {
            if dot_leader_row(&peek_text_of_line(&pl.lines[lid], &pl.spans)) {
                is_toc = true;
                if last_toc >= 0 {
                    last_toc = cp;
                } else {
                    lines += 1;
                    if lines >= 5 || (lines >= 3 && is_last_page) {
                        last_toc = cp;
                    }
                }
            }
        }
        if let Some(&ll) = block.lines.last()
            && dot_leader_row(&peek_text_of_line(&pl.lines[ll], &pl.spans))
        {
            is_toc = true;
            if last_toc >= 0 {
                last_toc = cp;
                continue;
            }
            dot_leader_blocks += 1;
            weight += info_weight(&block.char_stats);
            if is_last_page && dot_leader_blocks >= 2 && weight >= 0.8 * pl.stats.total_line_weight
            {
                last_toc = cp;
            }
        }
        if !is_toc && is_body_paragraph(&doc.stats, page, block) {
            seen_body = true;
            body_stop_y = pymath::min(body_stop_y, block.bottom_edge());
        }
        if block.line_count() > 1 && !is_toc {
            last_multiline = cp;
        }
        if contents < 0
            && block.line_count() <= 1
            && TOC_TITLES_TRIE.full_match(&tokenize_block(block, pl))
        {
            contents = cp;
            pre_contents = last_multiline;
            if !seen_body && block.top_edge() > 3.0 * pl.bounds.bbox_height() / 4.0 {
                return Some((pre_contents + 1, blocks.len() as i64 - 1));
            }
        }
        if block.right_edge() < pl.bounds.center_x() {
            continue;
        }
        if block.top_edge() > body_stop_y {
            continue;
        }
        if let Some(c) = extract_number_column(block, page) {
            let succ_i = clusters.partition_point(|x| x.anchor_x < c.anchor_x);
            let succ = (succ_i < clusters.len()).then_some(succ_i);
            let pred_i = clusters.partition_point(|x| x.anchor_x <= c.anchor_x);
            let pred = (pred_i > 0).then(|| pred_i - 1);
            let new = match pick_nearer_cluster(&c, pred, succ, &clusters) {
                Some(p) => {
                    let picked = clusters.remove(p);
                    NumberColumnCluster {
                        anchor_x: picked.anchor_x,
                        width: picked.width,
                        first: picked.first,
                        last: c.last,
                        length: picked.length + c.length,
                        increasing: picked.increasing && c.increasing && picked.last <= c.first,
                    }
                }
                None => c,
            };
            let at = clusters.partition_point(|x| x.anchor_x < new.anchor_x);
            if !(at < clusters.len() && clusters[at].anchor_x == new.anchor_x) {
                clusters.insert(at, new);
            }
            if new.increasing
                && (new.length >= 10 || (new.length >= 5 && is_last_page))
                && (new.last - new.first) as f64 > 0.01 * new.last as f64
            {
                last_toc = cp;
            }
        }
    }
    if last_toc < 0 {
        return None;
    }
    Some((
        if pre_contents >= 0 {
            pre_contents + 1
        } else {
            0
        },
        last_toc,
    ))
}

/// TOC blocks become type 9; boilerplate-dominated front-matter pages type 12.
// ref: classification/toc_boilerplate.py::mark_toc_and_boilerplate
pub fn mark_toc_and_boilerplate(doc: &Document) {
    let npages = doc.pages.len() as f64;
    let mpw = doc.stats.median_page_weight;
    let mut prev_toc: Option<usize> = None;
    for page in &doc.pages {
        let pl = &page.layout;
        let pi = page.index();
        if (pi - 1) as f64 >= npages / 2.0 && pl.stats.total_line_weight >= 0.9 * mpw {
            continue;
        }
        if pi > 1
            && pi < 50
            && pl.stats.total_line_weight < pymath::max(200.0, pymath::min(0.75 * mpw, 1000.0))
        {
            let (mut total, mut bp, mut bpl) = (0.0, 0.0, 0);
            for &bid in &page.output {
                let b = &page.blocks[bid];
                if b.kind.get() != 0 || b.weighted_skew >= 1.0 {
                    continue;
                }
                let w = info_weight(&b.char_stats) * heading_score(b);
                total += w;
                if is_boilerplate_block(b, page) {
                    bp += w;
                    bpl += 1;
                }
            }
            if bp >= 0.8 * total && bpl >= 3 {
                for &bid in &page.output {
                    page.blocks[bid].kind.set(12);
                }
                continue;
            }
        }
        let Some((start, end)) = detect_toc_range(doc, page, prev_toc) else {
            continue;
        };
        prev_toc = Some(pi);
        let mut centered = 0;
        let mut wflag = 0.0;
        let mut width = 0.0;
        let mut walk_break_y = pl.bounds.top_edge();
        for idx in start.max(0) as usize..page.output.len() {
            let b = &page.blocks[page.output[idx]];
            let score = heading_score(b);
            if idx as i64 <= end {
                if b.isolated_centered.get() {
                    centered += 1;
                }
                let hw = info_weight(&b.char_stats);
                wflag += b.weighted_font_size * hw;
                width += hw;
                walk_break_y = pymath::min(walk_break_y, b.bottom_edge());
                b.kind.set(9);
                continue;
            }
            if b.weighted_skew < 1.0
                && score > doc.stats.body_font_size + 4.0
                && score > pl.stats.median_font_size + 4.0
            {
                break;
            }
            if centered <= 1 && b.isolated_centered.get() {
                break;
            }
            if info_weight(&b.char_stats) > 300.0
                && b.char_stats.category_counts[6] > 2
                && b.density_area > 0.5
            {
                break;
            }
            if width > 0.0 {
                let avg = wflag / width;
                if walk_break_y - b.top_edge() > avg && b.weighted_skew < 1.0 && score > avg + 1.5 {
                    break;
                }
            }
            walk_break_y = pymath::min(walk_break_y, b.bottom_edge());
            b.kind.set(9);
        }
    }
}
