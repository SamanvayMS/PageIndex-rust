//! Document-title detection.
//!
//! ref: pageindex/flash/title/ (dicts.py, scoring.py, detect.py)

use std::collections::HashSet;
use std::sync::LazyLock;

use pi_pycompat::pymath;

use crate::blocks::join_rules::dict_list;
use crate::model::block::{
    Block, BlockId, alignment_code, deaccented_text, dominant_style_of, heading_score,
};
use crate::model::char_stats::{letter_count, trim_unicode_ws};
use crate::model::rects::{Bounded, center_aligned, left_aligned, right_aligned};
use crate::phases::{BlockRef, DocPage, Document};
use crate::tokens::{
    Trie, clamp_value, de_norm, is_superscript_adjacent, is_word_token, jenkins_hash,
    tokenize_block,
};

/// ref: title/dicts.py:35 `TITLE_LABEL_TRIE`
static TITLE_LABEL_TRIE: LazyLock<Trie> =
    LazyLock::new(|| Trie::build(dict_list("title"), true, false));
/// ref: title/dicts.py:35 `INSTITUTION_WORDS` (raw strings, single-token membership)
static INSTITUTION_WORDS: LazyLock<HashSet<String>> =
    LazyLock::new(|| dict_list("institution_words").into_iter().collect());

/// Best title candidate: page (0-based position), contributing blocks, score.
/// ref: title/scoring.py::TitleCandidate
#[derive(Debug, Clone)]
pub struct TitleCandidate {
    pub page: usize,
    pub blocks: Vec<BlockId>,
    pub score: f64,
}

impl TitleCandidate {
    /// Blocks' token strings, trimmed, joined by single spaces once non-empty.
    // ref: title/scoring.py::TitleCandidate.to_string
    pub fn to_string_py(&self, doc: &Document) -> String {
        let page = &doc.pages[self.page];
        let mut out = String::new();
        for &b in &self.blocks {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(trim_unicode_ws(
                &tokenize_block(&page.blocks[b], &page.layout).to_string_py(),
            ));
        }
        out
    }
}

// ref: title/scoring.py::is_cover_like_page
pub fn is_cover_like_page(doc: &Document, page: &DocPage) -> bool {
    if page.has_caption.get() {
        return false;
    }
    let threshold = 0.5 * pymath::min(doc.stats.median_page_weight, 5e3);
    let tlw = page.layout.stats.total_line_weight;
    if page.index() <= 1 && tlw < threshold {
        return true;
    }
    let early = 1.0 + pymath::min(15.0, doc.pages.len() as f64 / 5.0);
    (page.index() as f64) < early && tlw < 0.8 * threshold
}

// ref: title/scoring.py::is_title_candidate_block
pub fn is_title_candidate_block(b: &Block) -> bool {
    letter_count(&b.char_stats) > 0
        && b.weighted_skew < 1.0
        && b.kind.get() == 0
        && b.char_count() < 400
        && b.bbox_height() < 2.0 * b.bbox_width()
}

/// ref: title/detect.py::TitleSearchState
struct TitleSearchState {
    visited: HashSet<BlockRef>,
    best: Option<TitleCandidate>,
}

// ref: title/scoring.py::score_title_candidate
fn score_title_candidate(st: &mut TitleSearchState, doc: &Document, pi0: usize, index: usize) {
    let page = &doc.pages[pi0];
    let pl = &page.layout;
    let blocks = &page.reading;
    let tb = &page.blocks[blocks[index]];
    let mut group: Vec<BlockId> = vec![blocks[index]];
    if index + 1 < blocks.len() {
        let nx = &page.blocks[blocks[index + 1]];
        let ts = heading_score(tb);
        let height = tb.weighted_font_size;
        if ((ts - heading_score(nx)).abs() < 0.1
            && dominant_style_of(tb) == dominant_style_of(nx)
            && tb.bottom_edge() - nx.top_edge() < height)
            || (ts > doc.stats.body_font_size + 5.0
                && ts > pl.stats.median_font_size + 1.0
                && (height - nx.weighted_font_size).abs() < 0.1
                && tb.bottom_edge() - nx.top_edge() < 0.5 * height)
        {
            let tol = 0.1 * height;
            let a = alignment_code(tb, pl);
            let na = alignment_code(nx, pl);
            if (left_aligned(tb, nx, tol) && matches!(a, 1 | 2) && matches!(na, 1 | 2))
                || (right_aligned(tb, nx, tol) && matches!(a, 1 | 4) && matches!(na, 1 | 4))
                || (center_aligned(tb, nx, tol) && tb.center_aligned && nx.center_aligned)
            {
                group.push(blocks[index + 1]);
            }
        }
    }
    for &b in &group {
        st.visited.insert((pi0, b));
    }
    let previous = if index >= 1 {
        Some(&page.blocks[blocks[index - 1]])
    } else {
        None
    };

    let mut max_hs = 0.0;
    let mut max_width = 0.0;
    let mut consecutive = 0i64;
    let mut max_consecutive = 0i64;
    let mut bracket_count = 0i64;
    let mut total_tokens = 0i64;
    let mut right_pen = 1.0;
    let mut email_count = 0i64;
    for &gid in &group {
        let g = &page.blocks[gid];
        max_hs = pymath::max(max_hs, heading_score(g));
        max_width = pymath::max(max_width, g.bbox_width());
        let toks = tokenize_block(g, pl);
        for (i, t) in toks.iter().enumerate() {
            total_tokens += 1;
            if is_word_token(t) {
                consecutive += 1;
                max_consecutive = max_consecutive.max(consecutive);
                if t.boundary {
                    bracket_count += 1;
                }
                let i = i as i64;
                if t.text == "@" && i + 3 < toks.length {
                    let (n, d, a) = (
                        toks.token_at(i + 1),
                        toks.token_at(i + 2),
                        toks.token_at(i + 3),
                    );
                    if let (Some(n), Some(d), Some(a)) = (n, d, a)
                        && n.kind == 2
                        && d.text == "."
                        && a.kind == 2
                    {
                        email_count += 1;
                    }
                }
            } else {
                consecutive = 0;
            }
        }
        if alignment_code(g, pl) == 4 {
            right_pen /= g.line_count().max(1) as f64;
        }
    }
    if total_tokens <= 0 {
        return;
    }
    let tt = total_tokens as f64;
    let len_value = clamp_value(tt * tt / 16.0, 0.5, 1.0);
    let pw = pl.bounds.bbox_width();
    let mut wr = if pw != 0.0 {
        max_width / pw
    } else if max_width > 0.0 {
        f64::INFINITY
    } else {
        f64::NAN
    };
    wr *= wr;
    let bracket = bracket_count as f64 / tt;
    let bracket_factor =
        pymath::max(0.1, 1.0 - 9.0 * bracket * bracket) / (1i64.max(max_consecutive - 2)) as f64;
    let npages = doc.pages.len();
    let page_pos = pymath::max(
        0.1,
        1.0 - 2.0 * (page.index() as f64 - 1.0) / (npages.max(1)) as f64,
    );
    let pdr = pl.stats.total_line_weight / pymath::max(1e-6, doc.stats.median_page_weight);
    let density =
        pymath::max(0.5, 1.0 - pdr * pdr) * (1.0 + clamp_value((0.25 - pdr) / 0.15, 0.0, 1.0));
    let first = &page.blocks[group[0]];
    let top = if pl.bounds.top_edge() != 0.0 {
        pymath::max(0.1, first.top_edge() / pl.bounds.top_edge())
    } else {
        0.1
    };
    let mut abbrev = 0i64;
    for &gid in &group {
        let toks = tokenize_block(&page.blocks[gid], pl);
        let mut prev = None;
        for t in toks.iter() {
            if let Some(p) = prev
                && t.len <= 1
                && is_superscript_adjacent(p, t, pl)
            {
                abbrev += 1;
            }
            prev = Some(t);
        }
    }
    let factor = clamp_value(1.0 / (abbrev.max(1)) as f64, 0.3, 1.0);
    let norm = jenkins_hash(deaccented_text(first, pl));
    let rc = doc.recurring.borrow().get(&norm).copied().unwrap_or(0);
    let ratio = rc as f64 / npages.max(1) as f64;
    let adj = total_tokens - 3;
    let recurrence =
        1.0 - 0.5 * clamp_value(ratio / 0.3, 0.0, 1.0) * (1.0 / (adj * adj).max(1) as f64);
    let mut institution = 1.0;
    if is_cover_like_page(doc, page) {
        let mut hits = 0;
        for &gid in &group {
            for t in tokenize_block(&page.blocks[gid], pl).iter() {
                if INSTITUTION_WORDS.contains(&de_norm(&t.text, true)) {
                    hits += 1;
                }
            }
        }
        institution = 1.0 / (1 + hits) as f64;
    }
    let mut label = 1.0;
    if let Some(p) = previous {
        let pt = tokenize_block(p, pl);
        if pt.length <= 3 && TITLE_LABEL_TRIE.prefix_match(&pt).is_some() {
            label = 3.0;
        }
    }
    let email = 1.0 / ((1 + email_count) * (1 + email_count)) as f64;
    let score = max_hs
        * len_value
        * wr
        * right_pen
        * bracket_factor
        * page_pos
        * density
        * top
        * factor
        * recurrence
        * institution
        * label
        * email;
    if st.best.as_ref().is_none_or(|b| score > b.score) {
        st.best = Some(TitleCandidate {
            page: pi0,
            blocks: group,
            score,
        });
    }
}

/// Scores title-like block groups on early pages and returns the best candidate.
// ref: title/detect.py::detect_title
pub fn detect_title(doc: &Document) -> Option<TitleCandidate> {
    let mut st = TitleSearchState {
        visited: HashSet::new(),
        best: None,
    };
    let mut has_seen_da = false;
    let dfs = doc.stats.body_font_size;
    let mpw = doc.stats.median_page_weight;
    for (pi0, page) in doc.pages.iter().enumerate() {
        let pl = &page.layout;
        let pfs = pl.stats.median_font_size;
        let tlw = pl.stats.total_line_weight;
        if is_cover_like_page(doc, page)
            || (page.index() <= 1 && doc.pages.len() >= 10 && tlw < 0.8 * mpw)
        {
            for idx in 0..page.reading.len() {
                let bid = page.reading[idx];
                let b = &page.blocks[bid];
                if !is_title_candidate_block(b) || st.visited.contains(&(pi0, bid)) {
                    continue;
                }
                let s = heading_score(b);
                let iso = b.isolated_centered.get();
                if (s > dfs + 0.1 && s > pfs + 0.1)
                    || (s > dfs + 2.0 && s > pfs - 0.1)
                    || (s > dfs - 0.1 && s > pfs - 0.1 && iso)
                    || (s > dfs - 0.1 && s > pfs - 0.1 && page.index() <= 1 && tlw < 500.0)
                {
                    score_title_candidate(&mut st, doc, pi0, idx);
                }
            }
        } else {
            let mut local_done = false;
            for idx in 0..page.reading.len() {
                let bid = page.reading[idx];
                if st.visited.contains(&(pi0, bid)) {
                    continue;
                }
                let b = &page.blocks[bid];
                let s = heading_score(b);
                let iso = b.isolated_centered.get();
                let size_trigger = is_title_candidate_block(b)
                    && ((s > dfs + 0.1 && s > pfs + 0.1)
                        || (s > dfs + 2.0 && s > pfs - 0.1)
                        || (iso && s > pfs - 0.1)
                        || (page.index() == 1 && s > pfs + 2.0));
                if size_trigger {
                    score_title_candidate(&mut st, doc, pi0, idx);
                } else if b.is_body_paragraph.get() && !iso {
                    if !has_seen_da {
                        if b.bottom_edge() - pl.bounds.bottom_edge()
                            < 2.0 * pl.bounds.bbox_height() / 3.0
                        {
                            has_seen_da = false;
                        } else if b.line_count() >= 3 && alignment_code(b, pl) == 4 {
                            has_seen_da = true;
                        } else {
                            let toks = tokenize_block(b, pl);
                            let n = toks
                                .iter()
                                .filter(|t| is_word_token(t) || t.kind == 1)
                                .count();
                            has_seen_da = n as f64 >= toks.length as f64 / 3.0;
                        }
                        has_seen_da = !has_seen_da;
                    }
                    if has_seen_da {
                        local_done = true;
                        break;
                    }
                    local_done = true;
                    has_seen_da = true;
                }
            }
            if local_done {
                break;
            }
            if mpw < 400.0 {
                break;
            }
        }
    }
    st.best
}
