//! Header and footer detection via cross-page recurrence.
//!
//! ref: pageindex/flash/classification/header_footer.py

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use pi_pycompat::pymath;

use super::body_text::{
    is_body_paragraph, longest_word_and_number, normalized_block_text, record_recurring_text,
    span_page_number, span_style_text_key,
};
use super::keyword_tables::{
    CHART_KEYWORDS_TRIE, COPYRIGHT_TRIE, FIGURE_KEYWORDS_TRIE, TABLE_KEYWORDS_TRIE,
    VOLUME_WORDS_TRIE, search_trie,
};
use crate::model::block::{Block, BlockId, deaccented_text, heading_score};
use crate::model::char_stats::{info_weight, letter_count};
use crate::model::rects::Bounded;
use crate::phases::{BlockRef, DocPage, Document};
use crate::stats::weighted_percentile;
use crate::tokens::tokenize_block;

/// ref: classification/header_footer.py::PageMarkState
struct PageMarkState {
    first_index: i64,
    max_heading_score: f64,
    chars: u64,
}

// ref: classification/header_footer.py::record_marked_block
fn record_marked_block(st: &mut PageMarkState, idx: usize, b: &Block) {
    st.first_index = idx as i64;
    st.max_heading_score = pymath::max(st.max_heading_score, heading_score(b));
    st.chars += b.char_count() as u64;
}

/// ref: classification/header_footer.py::HeaderFooterContext
struct HfCtx<'a> {
    doc: &'a Document,
    /// 1 header, 2 footer. ref slot: `primary_slot`
    kind: u8,
    /// Span style+text key -> page count. ref slot: `option_slot`
    style_counts: HashMap<String, u64>,
    /// Per-page page numbers. ref slot: `tertiary_slot`
    page_numbers: Vec<HashSet<i64>>,
    /// Normalized text -> (page_index, block). ref slot: `auxiliary_slot`
    text_locs: HashMap<String, Vec<(usize, BlockRef)>>,
    /// Word/number key -> page -> (text key, block). ref slot: `measure_slot`
    word_map: HashMap<String, HashMap<usize, (String, BlockRef)>>,
    /// Per-page candidate blocks. ref slot: `state_slot`
    candidates: Vec<Vec<BlockId>>,
}

impl HfCtx<'_> {
    fn block(&self, r: BlockRef) -> &Block {
        &self.doc.pages[r.0].blocks[r.1]
    }

    fn npages(&self) -> usize {
        self.doc.pages.len()
    }
}

// ref: classification/header_footer.py::is_header_positioned
fn is_header_positioned(kind: u8, b: &Block, r: Option<&Block>) -> bool {
    let cond = match r {
        None => true,
        Some(r) if kind == 1 => b.top_edge() > r.bottom_edge(),
        Some(r) => b.bottom_edge() < r.top_edge(),
    };
    cond && b.line_count() == 1
        && info_weight(&b.char_stats) >= 8.0
        && letter_count(&b.char_stats) >= 5
        && b.char_stats.category_counts[1] >= 1
}

// ref: classification/header_footer.py::has_adjacent_page_numbers
fn has_adjacent_page_numbers(ctx: &HfCtx, page: usize, n: i64, flag: bool) -> bool {
    let pi = page as i64 - 1;
    let count = ctx.npages() as i64;
    let has = |p: i64, v: i64| p >= 0 && p < count && ctx.page_numbers[p as usize].contains(&v);
    let adj = |d: i64| has(pi - d, n - d) || has(pi + d, n + d);
    let (a1, a2) = (adj(1), adj(2));
    if !a1 && !a2 {
        return false;
    }
    if a1 && a2 {
        return true;
    }
    let a4 = adj(4);
    if n as f64 > page as f64 / 2.0 - 30.0 {
        return a1 || (!flag && a2) || (a2 && a4);
    }
    a2 && a4
}

// ref: classification/header_footer.py::mark_header_footer
fn mark_header_footer(ctx: &HfCtx, b: &Block, page: &DocPage) {
    record_recurring_text(ctx.doc, deaccented_text(b, &page.layout));
    b.kind.set(ctx.kind);
}

// ref: classification/header_footer.py::find_cross_page_match
fn find_cross_page_match(
    ctx: &HfCtx,
    pi0: usize,
    b: &Block,
    text_key: &str,
    r: Option<&Block>,
) -> Option<BlockRef> {
    let page = &ctx.doc.pages[pi0];
    let page_index = page.index();
    if let Some(entries) = ctx.text_locs.get(text_key) {
        for &(epi, eref) in entries {
            if (epi as i64) < page_index as i64 - 3 {
                continue;
            }
            if epi == page_index {
                continue;
            }
            if epi > page_index + 3 {
                break;
            }
            let eb = ctx.block(eref);
            let dl = eb.left_edge() - b.left_edge();
            let dt = eb.top_edge() - b.top_edge();
            let dr = eb.right_edge() - b.right_edge();
            let db = eb.bottom_edge() - b.bottom_edge();
            let dsq = dl * dl + dt * dt + dr * dr + db * db;
            let size = page.layout.stats.median_font_size;
            if !(dsq >= 100.0
                || (dsq >= 1.0
                    && ((page_index == 1 && heading_score(b) >= size + 0.5)
                        || (epi == 1 && heading_score(eb) >= size + 0.5))))
            {
                return Some(eref);
            }
        }
    }
    if is_header_positioned(ctx.kind, b, r) {
        for key in longest_word_and_number(b, page) {
            let Some(mv) = ctx.word_map.get(&key) else {
                continue;
            };
            if (mv.len() as f64) < pymath::max(4.0, ctx.npages() as f64 / 4.0) {
                continue;
            }
            let target = heading_score(b);
            for npi in page_index as i64 - 2..page_index as i64 + 3 {
                if npi == page_index as i64 || npi < 0 {
                    continue;
                }
                let Some((ntk, nref)) = mv.get(&(npi as usize)) else {
                    continue;
                };
                let nb = ctx.block(*nref);
                let bfs = page.layout.stats.median_font_size;
                if (target - heading_score(nb)).abs() > 1.0
                    || (page_index == 1 && target >= bfs + 0.5)
                    || (npi == 1 && heading_score(nb) >= bfs + 0.5)
                {
                    continue;
                }
                let tl = text_key.chars().count().min(ntk.chars().count()) as f64 / 5.0;
                if bounded_edit_distance(text_key, ntk, tl) >= tl {
                    continue;
                }
                return Some(*nref);
            }
        }
    }
    None
}

/// Banded Levenshtein distance capped at `limit` (ceil'd; `<= 0` means the longer length).
// ref: classification/header_footer.py::bounded_edit_distance
pub fn bounded_edit_distance(a: &str, b: &str, limit: f64) -> f64 {
    let mut a: Vec<char> = a.chars().collect();
    let mut b: Vec<char> = b.chars().collect();
    let c: i64 = if limit <= 0.0 {
        a.len().max(b.len()) as i64
    } else {
        limit.ceil() as i64
    };
    if a.is_empty() {
        return (b.len() as i64).min(c) as f64;
    }
    if b.is_empty() {
        return (a.len() as i64).min(c) as f64;
    }
    if a.len() < b.len() {
        std::mem::swap(&mut a, &mut b);
    }
    let la = a.len();
    if (la - b.len()) as i64 >= c {
        return c as f64;
    }
    let mut lo = 0usize;
    let mut hi = 0usize;
    let mut prev = vec![0i64; la + 1];
    let mut cur = vec![0i64; la + 1];
    // Seed row 0, but only out to column c.
    for (s, p) in prev.iter_mut().enumerate() {
        *p = s as i64;
        if s as i64 > c {
            break;
        }
        hi = s;
    }
    for s in 1..=b.len() {
        let ch = b[s - 1];
        let mut kv = la;
        let mut mv = 0usize;
        for lv in lo..=(hi + 1).min(la) {
            if lv == lo {
                cur[lv] = 1 + prev[lv];
            } else if a[lv - 1] == ch {
                cur[lv] = prev[lv - 1];
            } else {
                cur[lv] = 1 + cur[lv - 1].min(prev[lv - 1]);
                if lv <= hi {
                    cur[lv] = cur[lv].min(1 + prev[lv]);
                }
            }
            if cur[lv] < c {
                kv = kv.min(lv);
                mv = lv;
            }
        }
        if kv > mv {
            return c as f64;
        }
        std::mem::swap(&mut prev, &mut cur);
        lo = kv;
        hi = mv;
    }
    (prev[hi] + la as i64 - hi as i64).min(c) as f64
}

/// Walk callback of pass 1. Returns true to halt.
fn collect_candidate(
    ctx: &mut HfCtx,
    pi0: usize,
    bid: BlockId,
    seen: &mut HashSet<String>,
    first_ref: &mut Option<BlockId>,
) -> bool {
    let doc = ctx.doc;
    let page = &doc.pages[pi0];
    let pl = &page.layout;
    let b = &page.blocks[bid];
    if b.weighted_skew >= 1.0 || b.area() <= 0.0 {
        return false;
    }
    let den = pl.bounds.bbox_height();
    let num = if ctx.kind == 1 {
        b.top_edge()
    } else {
        b.bottom_edge()
    };
    let relative = if den != 0.0 {
        num / den
    } else if num != 0.0 {
        f64::INFINITY.copysign(num)
    } else {
        f64::NAN
    };
    let pass = if (ctx.kind == 1 && relative < 0.8) || (ctx.kind == 2 && relative > 0.2) {
        false
    } else {
        let toks = tokenize_block(b, pl);
        if search_trie(&COPYRIGHT_TRIE, &toks).is_some() {
            true
        } else {
            !(b.line_count() >= 3
                || info_weight(&b.char_stats) * (1.0 + b.bold_frac) >= 200.0
                || FIGURE_KEYWORDS_TRIE.prefix_match(&toks).is_some()
                || CHART_KEYWORDS_TRIE.prefix_match(&toks).is_some()
                || TABLE_KEYWORDS_TRIE.prefix_match(&toks).is_some())
        }
    };
    if !pass {
        return true;
    }
    if letter_count(&b.char_stats) >= 5 && first_ref.is_none() {
        *first_ref = Some(bid);
    }
    let r = first_ref.map(|id| &page.blocks[id]);
    if b.kind.get() == 0 && b.char_count() > 0 {
        ctx.candidates[pi0].push(bid);
        if letter_count(&b.char_stats) >= 5 {
            let text_key = normalized_block_text(b, page);
            ctx.text_locs
                .entry(text_key.clone())
                .or_default()
                .push((page.index(), (pi0, bid)));
            if is_header_positioned(ctx.kind, b, r) {
                for key in longest_word_and_number(b, page) {
                    ctx.word_map
                        .entry(key)
                        .or_default()
                        .entry(page.index())
                        .or_insert_with(|| (text_key.clone(), (pi0, bid)));
                }
            }
        }
        for &lid in &b.lines {
            for &sid in &pl.lines[lid].spans {
                let span = &pl.spans[sid];
                if span.char_count() == 0 {
                    continue;
                }
                let ok = span_style_text_key(span);
                if b.char_count() >= 4 && !seen.contains(&ok) {
                    *ctx.style_counts.entry(ok.clone()).or_insert(0) += 1;
                    seen.insert(ok);
                }
                if let Some(n) = span_page_number(span) {
                    ctx.page_numbers[pi0].insert(n);
                }
            }
        }
    }
    false
}

/// Three-pass header (kind 1) / footer (kind 2) detector.
// ref: classification/header_footer.py::detect_header_footer
pub fn detect_header_footer(doc: &Document, kind: u8) {
    let mut ctx = HfCtx {
        doc,
        kind,
        style_counts: HashMap::new(),
        page_numbers: Vec::new(),
        text_locs: HashMap::new(),
        word_map: HashMap::new(),
        candidates: Vec::new(),
    };
    let npages = doc.pages.len();

    // Pass 1: per-page candidate collection.
    for pi0 in 0..npages {
        let mut seen = HashSet::new();
        ctx.page_numbers.push(HashSet::new());
        ctx.candidates.push(Vec::new());
        let mut first_ref = None;
        let order: Vec<BlockId> = if kind == 1 {
            doc.pages[pi0].output.clone()
        } else {
            doc.pages[pi0].output.iter().rev().copied().collect()
        };
        for bid in order {
            if collect_candidate(&mut ctx, pi0, bid, &mut seen, &mut first_ref) {
                break;
            }
        }
    }

    // Pass 2: per-page rejection sweep.
    let mut text_counts: IndexMap<String, u64> = IndexMap::new();
    let mut samples: Vec<(f64, f64)> = Vec::new();
    for pi0 in 0..npages {
        let page = &doc.pages[pi0];
        let pl = &page.layout;
        let cands = ctx.candidates[pi0].clone();
        let mut st = PageMarkState {
            first_index: -1,
            max_heading_score: 0.0,
            chars: 0,
        };
        let mut seen_page_number = false;
        let mut first_sub: Option<BlockId> = None;
        for (ci, &bid) in cands.iter().enumerate() {
            let cb = &page.blocks[bid];
            if cb.char_count() == 0 {
                continue;
            }
            if cb.kind.get() == kind {
                record_marked_block(&mut st, ci, cb);
                continue;
            }
            if cb.kind.get() != 0 {
                continue;
            }
            let toks = tokenize_block(cb, pl);
            if toks.length < 10 && COPYRIGHT_TRIE.prefix_match(&toks).is_some() {
                mark_header_footer(&ctx, cb, page);
                record_marked_block(&mut st, ci, cb);
                continue;
            }
            if letter_count(&cb.char_stats) >= 5 {
                if first_sub.is_none() {
                    first_sub = Some(bid);
                }
                let pk = normalized_block_text(cb, page);
                let r = first_sub.map(|id| &page.blocks[id]);
                if let Some(mref) = find_cross_page_match(&ctx, pi0, cb, &pk, r) {
                    mark_header_footer(&ctx, cb, page);
                    record_marked_block(&mut st, ci, cb);
                    let mb = ctx.block(mref);
                    let other_unmarked = mb.kind.get() != kind;
                    if other_unmarked {
                        mark_header_footer(&ctx, mb, &doc.pages[mref.0]);
                    }
                    if pk.chars().count() >= 5 {
                        let prev = text_counts.get(&pk).copied().unwrap_or(0);
                        text_counts.insert(pk, if prev != 0 || other_unmarked { 2 } else { 1 });
                    }
                    continue;
                }
            }
            let style_threshold = pymath::max(2.0, pymath::min(npages as f64 / 3.0, 5.0));
            let mut chars = 0u64;
            for &lid in &cb.lines {
                for &sid in &pl.lines[lid].spans {
                    let span = &pl.spans[sid];
                    if span.char_count() == 0 {
                        continue;
                    }
                    let sh = span_style_text_key(span);
                    if ctx.style_counts.get(&sh).copied().unwrap_or(0) as f64 >= style_threshold {
                        chars += span.char_count() as u64;
                        continue;
                    }
                    if let Some(n) = span_page_number(span)
                        && has_adjacent_page_numbers(&ctx, page.index(), n, seen_page_number)
                    {
                        seen_page_number = true;
                        chars += span.char_count() as u64;
                    }
                }
            }
            if chars >= cb.char_count() as u64 {
                mark_header_footer(&ctx, cb, page);
                record_marked_block(&mut st, ci, cb);
                let pk2 = normalized_block_text(cb, page);
                if pk2.chars().count() >= 5 {
                    *text_counts.entry(pk2).or_insert(0) += 1;
                }
            }
        }
        if st.first_index < 0 {
            continue;
        }
        let first_block = &page.blocks[cands[st.first_index as usize]];
        samples.push((first_block.center_y(), st.chars as f64));
        for &bid in &cands[..st.first_index as usize] {
            let b = &page.blocks[bid];
            if b.kind.get() == kind {
                continue;
            }
            if kind == 1 && b.bottom_edge() < first_block.bottom_edge() {
                continue;
            }
            if b.bbox_width() >= pl.bounds.bbox_width() / 2.0 {
                continue;
            }
            if is_body_paragraph(&doc.stats, page, b) {
                continue;
            }
            if letter_count(&b.char_stats) > 0 && heading_score(b) >= st.max_heading_score + 1.0 {
                continue;
            }
            b.kind.set(kind);
            record_recurring_text(doc, deaccented_text(b, pl));
        }
    }

    // Pass 3: cutoff line + top-3 recurring text sweep.
    if (samples.len() as f64) < npages as f64 / 20.0 {
        return;
    }
    let cutoff = weighted_percentile(samples, if kind == 1 { 20.0 } else { 80.0 });
    let mut top: Vec<(String, u64)> = text_counts
        .into_iter()
        .filter(|(_, c)| (*c as f64) >= npages as f64 / 20.0)
        .collect();
    if top.is_empty() {
        return;
    }
    top.sort_by_key(|(_, c)| std::cmp::Reverse(*c)); // stable, like list.sort(key=-count)
    top.truncate(3);
    for pi0 in 0..npages {
        let page = &doc.pages[pi0];
        let pl = &page.layout;
        for &bid in &ctx.candidates[pi0] {
            let rb = &page.blocks[bid];
            if kind == 1 && rb.top_edge() < cutoff {
                break;
            }
            if kind == 2 && rb.bottom_edge() > cutoff {
                break;
            }
            if rb.kind.get() != 0 {
                continue;
            }
            let toks = tokenize_block(rb, pl);
            if let Some(stripped) = search_trie(&VOLUME_WORDS_TRIE, &toks) {
                let tail = toks.from(stripped.end);
                if tail.length > 0 && tail.token_at(0).is_some_and(|t| t.kind == 1) {
                    rb.kind.set(kind);
                    record_recurring_text(doc, deaccented_text(rb, pl));
                }
            }
            if letter_count(&rb.char_stats) < 5 {
                continue;
            }
            if page.index() <= 1 && heading_score(rb) > doc.stats.body_font_size + 1.0 {
                continue;
            }
            let text_key = normalized_block_text(rb, page);
            for (h, _) in &top {
                let thr = text_key.chars().count().min(h.chars().count()) as f64 / 2.0;
                if bounded_edit_distance(&text_key, h, thr) >= thr {
                    continue;
                }
                rb.kind.set(kind);
                record_recurring_text(doc, deaccented_text(rb, pl));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_distance() {
        assert_eq!(bounded_edit_distance("kitten", "sitting", 10.0), 3.0);
        assert_eq!(bounded_edit_distance("kitten", "sitting", 2.0), 2.0);
        assert_eq!(bounded_edit_distance("", "abc", 0.0), 3.0);
        assert_eq!(bounded_edit_distance("abc", "abc", 1.0), 0.0);
    }
}
