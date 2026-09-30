//! Font-change and body-embedded heading detectors.
//!
//! ref: pageindex/flash/heading_detection/style_detectors.py

use pi_layout::labels::caption_text::{is_uppercase_dominant, trie_matches_all};
use pi_layout::model::block::{Block, BlockId, alignment_code, dominant_style_of, is_caps_heavy};
use pi_layout::model::char_stats::{
    CharStats, info_weight, is_upper_dominant, letter_count, punct_count,
};
use pi_layout::model::rects::{Bounded, x_aligned};
use pi_layout::model::span_line::line_avg_char_width;
use pi_layout::phases::PageLayout;
use pi_layout::tokens::tries::is_comma_token;
use pi_layout::tokens::{is_trimmable_token, is_word_token, tokenize_block, trim_trailing_punct};

use super::candidates::{
    PageScanState, make_body_heading_candidate, make_heading_candidate, make_plain_candidate,
};
use super::detectors::{detect_labeled_heading, detect_numbered_heading, is_bibliography_entry};
use super::keyword_tables::{
    INTRODUCTION_SECTION_TRIE, KEYWORDS_SECTION_TRIE, SECTION_KEYWORDS_TRIE,
};
use super::text_checks::{matches_abstract, vertically_close};
use crate::model::Cand;

const INF: f64 = f64::INFINITY;

/// Mixed-case body text rather than a heading.
// ref: model/block.py::is_sentence_like
pub fn is_sentence_like(b: &Block, lay: &PageLayout) -> bool {
    let tokens = tokenize_block(b, lay);
    if tokens.length < 3 || is_caps_heavy(&b.char_stats) {
        return false;
    }
    let upper = b.char_stats.category_counts[2] as f64;
    if upper <= 2.0 || upper < tokens.length as f64 / 10.0 {
        return false;
    }
    let mut m = 0;
    for t in tokens.iter() {
        if t.kind != 2 || t.len <= 2 || t.first_cat == 4 {
            continue;
        }
        if t.first_cat == 2 {
            m += 1;
        } else if t.len >= 5 {
            return false;
        }
    }
    m >= 3
}

/// Font/position-based fallback heading classifier.
// ref: heading_detection/style_detectors.py::detect_font_heading
pub fn detect_font_heading(ps: &PageScanState, id: BlockId) -> Option<Cand> {
    let b = ps.b(id);
    let pg = ps.pg();
    let lay = &pg.layout;
    let body = ps.d.doc.stats.body_font_size;
    let p80 = ps.d.doc.stats.p80_density;
    let above = ps.ob(ps.neighbors.above(b));
    let top_gap = above.map_or(INF, |a| a.bottom_edge() - b.top_edge());
    let pred = ps.ob(ps.neighbors.right(b));
    let pred_gap = pred.map_or(INF, |p| b.bottom_edge() - p.top_edge());
    let keyword_match = ps.ob(ps.neighbors.body_above(b));
    let above_or_overlap_id = ps.neighbors.closest_body(b);
    let above_or_overlap = ps.ob(above_or_overlap_id);
    let size = b.weighted_font_size;

    if !(vertically_close(keyword_match, b)
        || vertically_close(above_or_overlap, b)
        || (b.bottom_edge() >= 0.8 * lay.bounds.bbox_height()
            && size >= body + 1.0
            && b.bold_frac > 0.9
            && above.is_none_or(|a| a.kind.get() == 1)))
    {
        return None;
    }

    let page = &lay.stats;
    let far = 10.0 * pi_pycompat::pymath::min(size, page.median_overlap_gap);
    if top_gap < INF && top_gap > far && above.is_some_and(|a| a.region_label.get() == 0) {
        return None;
    }
    if pred.is_some_and(|p| p.region_label.get() != 0) {
        return None;
    }

    let mut inner_reject = false;
    if above.is_some_and(|a| a.region_label.get() != 0) {
        inner_reject = pred_gap > 5.0 * page.median_overlap_gap
            || pred.is_some_and(|p| {
                !p.is_body_paragraph.get()
                    || p.bbox_width() < lay.bounds.bbox_width() / 5.0
                    || (p.char_count() as f64) < 0.5 * b.char_count() as f64
                    || p.density_area < 0.33
                    || (p.char_count() < 500 && p.density_area < 0.5 && alignment_code(p, lay) != 1)
                    || (p.char_count() < 250 && p.density_area < 0.5)
            });
    }
    if inner_reject
        || pred.is_some_and(|p| {
            p.density_chars < 0.67 * p80
                || (b.char_count() < 30 && p.char_count() < 300 && p.density_chars < 0.8 * p80)
        })
    {
        return None;
    }
    if is_comma_token(tokenize_block(b, lay).last()) {
        return None;
    }

    // Branch 1: tall first line + big font.
    if pred_gap < INF
        && pred_gap > 0.0
        && b.max_line_height >= body + 2.0
        && size >= body + 1.5
        && size >= page.median_font_size + 0.5
        && above
            .is_none_or(|a| b.max_line_height >= a.max_line_height && size >= a.weighted_font_size)
        && let Some(p) = pred
        && b.max_line_height >= p.max_line_height
        && size >= p.weighted_font_size
    {
        return Some(make_plain_candidate(ps, 0, id));
    }

    let caps_heavy = is_caps_heavy(&b.char_stats);
    let ds = dominant_style_of(b);
    if (pg.body_style_hashes.borrow().contains(ds)
        && !caps_heavy
        && (page.dominant_style == ds
            || (b.bold_frac < 0.9
                && b.italic_frac < 0.9
                && alignment_code(b, lay) != 3
                && !is_sentence_like(b, lay))))
        || (above.is_some()
            && pred.is_some_and(|p| size <= p.weighted_font_size && top_gap < pred_gap / 4.0))
    {
        return None;
    }

    // Branch 3: medium-confidence font-size heading.
    if pred_gap < INF
        && pred_gap > 0.0
        && size >= body + 0.5
        && let Some(p) = pred
        && b.max_line_height >= p.max_line_height
        && size >= p.weighted_font_size
        && p.weighted_font_size >= page.median_font_size - 0.5
        && b.bbox_width() < 0.95 * p.bbox_width()
        && p.bbox_width() >= 0.25 * lay.bounds.bbox_width()
    {
        return Some(make_plain_candidate(ps, 0, id));
    }

    let line_height = page.median_overlap_gap - page.median_font_size;
    // Branch 4: moderate-gap large-font heading.
    if pred_gap > line_height
        && pred_gap < 5.0 * line_height
        && above.is_none_or(|a| size >= a.weighted_font_size + 0.5)
        && let Some(p) = pred
        && size >= p.weighted_font_size + 0.5
        && size >= body - 0.5
        && b.bbox_width() < 0.95 * p.bbox_width()
        && p.is_body_paragraph.get()
        && p.weighted_font_size >= page.median_font_size - 0.5
        && p.char_stats.first_cat != 1
    {
        return Some(make_plain_candidate(ps, 0, id));
    }

    let cc = &b.char_stats.category_counts;
    let letters = cc[6] as f64;
    let num = letters + cc[8] as f64;
    let ratio = if cc[10] != 0 {
        num / cc[10] as f64
    } else if num > 0.0 {
        INF
    } else {
        f64::NAN
    };
    if letters > 1.0 && ratio > 0.3 {
        return None;
    }

    let symbols = punct_count(&b.char_stats) as f64;
    let letter_total = letter_count(&b.char_stats) as f64;
    let symbol_ratio = if letter_total != 0.0 {
        symbols / letter_total
    } else if symbols > 0.0 {
        INF
    } else {
        f64::NAN
    };
    if symbols >= 5.0 && symbol_ratio > 0.2
        || top_gap < 0.2 * size
        || top_gap < pi_pycompat::pymath::min(size, 0.7 * pred_gap)
    {
        return None;
    }

    let centered = b.char_stats.first_cat == 2;
    let neg = if caps_heavy {
        -0.2 * ps.d.last_span(ps.page, b).bbox_height()
    } else {
        0.0
    };

    // Branch A: tight criteria with neighbor analysis.
    let neighbor_heading_cue = pred_gap < INF
        && pred_gap > neg
        && size >= page.median_font_size - 0.1
        && pred.is_some_and(|p| {
            size >= p.weighted_font_size - 0.1
                && size >= body - 0.5
                && ((p.is_body_paragraph.get()
                    && b.bbox_width() < 0.95 * p.bbox_width()
                    && x_aligned(b, p, pi_pycompat::pymath::max(1.0, b.bbox_width() / 10.0))
                    && pred_gap < 6.0 * b.bbox_height())
                    || (top_gap < INF
                        && above.is_some_and(|a| {
                            a.is_body_paragraph.get()
                                && b.bbox_width() < 0.95 * a.bbox_width()
                                && x_aligned(
                                    b,
                                    a,
                                    pi_pycompat::pymath::max(1.0, b.bbox_width() / 10.0),
                                )
                                && top_gap < 6.0 * b.bbox_height()
                        })))
                && (centered || caps_heavy)
                && ((b.bold_frac > p.bold_frac
                    && b.bold_frac > 0.5
                    && (!ps.d.first_span(ps.page, p).bold
                        || above.is_some_and(|a| b.bold_frac > a.bold_frac)))
                    || caps_heavy)
        });
    let difference_style = pred.is_some_and(|p| {
        p.is_body_paragraph.get()
            && p.weighted_font_size > page.median_font_size - 0.5
            && pred_gap > 0.0
            && pred_gap < 3.0 * b.bbox_height()
            && dominant_style_of(b) != dominant_style_of(p)
    });
    let style_change_cue = match pred {
        Some(p) => {
            difference_style
                && above.is_some_and(|a| {
                    a.is_body_paragraph.get()
                        && top_gap > 0.0
                        && top_gap < 3.0 * b.bbox_height()
                        && centered
                        && dominant_style_of(a) == dominant_style_of(p)
                })
        }
        None => false,
    };
    if neighbor_heading_cue || style_change_cue {
        return Some(make_plain_candidate(ps, 0, id));
    }

    let topnum = above.is_none_or(|a| a.kind.get() == 1);
    let branch_c1 = topnum
        && centered
        && difference_style
        && pred_gap < b.bbox_height()
        && pred.is_some_and(|p| dominant_style_of(p) == page.dominant_style);
    let branch_c2 = topnum
        && centered
        && match (pred, above_or_overlap) {
            (Some(p), Some(ao)) => {
                ps.neighbors.right(b) != above_or_overlap_id
                    && p.bottom_edge() - ao.top_edge() < p.weighted_font_size
                    && dominant_style_of(ao) == page.dominant_style
                    && dominant_style_of(b) != page.dominant_style
                    && (size >= p.weighted_font_size + 0.5
                        || (caps_heavy && !is_upper_dominant(&p.char_stats)))
            }
            _ => false,
        };
    let branch_c3 = above.is_some_and(|a| {
        let aid = ps.neighbors.above(b).expect("above");
        (a.used_as_heading.get() || ps.pushed.contains(&aid))
            && (a.weighted_font_size >= size + 0.5
                || (is_upper_dominant(&a.char_stats) && !caps_heavy))
            && centered
            && difference_style
            && pred.is_some_and(|p| dominant_style_of(p) == page.dominant_style)
    });
    if branch_c1 || branch_c2 || branch_c3 {
        return Some(make_plain_candidate(ps, 0, id));
    }
    None
}

/// Heading-with-body two-line patterns.
// ref: heading_detection/style_detectors.py::detect_heading_with_body
pub fn detect_heading_with_body(ps: &PageScanState, id: BlockId) -> Option<Cand> {
    let b = ps.b(id);
    let lay = &ps.pg().layout;
    if b.line_count() < 2 {
        return None;
    }
    let tokens = tokenize_block(b, lay);
    let first_line_id = b.first_line();
    let second_line_id = b.lines[1];
    let first_line = &lay.lines[first_line_id];
    let second_line = &lay.lines[second_line_id];
    let mut split: i64 = 0;
    let mut letters = 0;
    let font = first_line
        .spans
        .first()
        .map(|&s| lay.spans[s].font_name.clone())
        .unwrap_or_default();
    let span_of =
        |t: &pi_layout::tokens::Token| &lay.spans[t.first_anchor_span().expect("anchor span")];
    if first_line.bold_frac > 0.0 && first_line.bold_frac < 1.0 {
        for (index, t) in tokens.iter().enumerate() {
            if t.line() != Some(first_line_id) || !span_of(t).bold {
                break;
            }
            if t.kind == 2 && t.len > 1 {
                letters += 1;
            }
            split = index as i64 + 1;
        }
    } else if ps.d.last_span(ps.page, b).font_name != font {
        let mut other_count = 0;
        for &lid in &b.lines {
            if lid != first_line_id && lay.spans[lay.lines[lid].spans[0]].font_name == font {
                other_count += 1;
            }
        }
        if other_count as f64 > b.line_count() as f64 / 4.0 {
            return None;
        }
        for (index, t) in tokens.iter().enumerate() {
            let line = t.line();
            if span_of(t).font_name != font
                || (line != Some(first_line_id) && line != Some(second_line_id))
            {
                break;
            }
            if t.kind == 2 && (t.len > 1 || t.first_cat == 4) {
                letters += 1;
            }
            split = index as i64 + 1;
        }
    }
    if split <= 0 || split >= tokens.length {
        return None;
    }
    let mut token = tokens.token_at(split - 1);
    let mut next_token = tokens.token_at(split);
    for _ in 0..2 {
        let (Some(t), Some(nt)) = (token, next_token) else {
            return None;
        };
        let last_anchor_line = t.last_anchor().and_then(|a| a.line);
        if !(is_word_token(nt) && last_anchor_line == nt.line() && (!t.boundary || nt.boundary)) {
            break;
        }
        split += 1;
        token = next_token;
        next_token = tokens.token_at(split);
        if token.is_none() || next_token.is_none() {
            return None;
        }
    }
    if letters <= 0 {
        return None;
    }
    let prefix = tokens.slice(0, split);

    let first = prefix.token_at(0);
    let first_anchor = first.map(span_of);
    if let Some(fa) = first_anchor {
        if !fa.bold && !fa.italic && b.italic_frac > 0.5 {
            return None;
        }
        if !fa.bold && fa.font_size < b.weighted_font_size + 1.0 {
            let rest = tokens.from(split);
            if rest.length <= 0 || rest.token_at(0).is_some_and(|t| t.first_cat == 3) {
                return None;
            }
        }
    }

    if let Some(m) = KEYWORDS_SECTION_TRIE.prefix_match(&prefix)
        && m.length >= letters
    {
        return None;
    }

    let heading_kind = detect_numbered_heading(ps, id, &prefix);
    if let Some(hk) = &heading_kind
        && hk.numbering.len() > 1
    {
        return Some(make_heading_candidate(
            ps,
            hk.kind,
            hk.block,
            hk.numbering.clone(),
            hk.prefix.clone(),
            hk.title.clone(),
            true,
        ));
    }
    if let Some(hk) = &heading_kind {
        let title = hk.title.as_ref().expect("numbered title");
        if trie_matches_all(&INTRODUCTION_SECTION_TRIE, title)
            || (is_uppercase_dominant(title) && !is_bibliography_entry(ps, id, -1))
        {
            return Some(make_heading_candidate(
                ps,
                hk.kind,
                hk.block,
                hk.numbering.clone(),
                hk.prefix.clone(),
                hk.title.clone(),
                true,
            ));
        }
    }

    if prefix.length > 3
        && let Some(hs) = detect_labeled_heading(ps, id, &prefix)
    {
        let title = trim_trailing_punct(hs.title.as_ref().expect("labeled title"));
        return Some(make_heading_candidate(
            ps,
            hs.kind,
            hs.block,
            hs.numbering.clone(),
            hs.prefix.clone(),
            Some(title),
            true,
        ));
    }

    if matches_abstract(&prefix) {
        return Some(make_body_heading_candidate(ps, 5, id, &prefix));
    }
    if trie_matches_all(&INTRODUCTION_SECTION_TRIE, &prefix) {
        return Some(make_body_heading_candidate(ps, 11, id, &prefix));
    }

    if first_line.avg_font_size < ps.d.doc.stats.body_font_size - 2.0 {
        return None;
    }
    if let Some(t) = token
        && is_word_token(t)
        && !is_trimmable_token(t)
    {
        return None;
    }

    if !is_upper_dominant(&b.char_stats)
        && b.is_body_paragraph.get()
        && info_weight(&b.char_stats) >= 100.0
    {
        let all_caps = CharStats::new(&prefix.to_string_py());
        if is_upper_dominant(&all_caps)
            && all_caps.category_counts[2] <= b.char_stats.category_counts[3]
        {
            if let Some(hk) = &heading_kind {
                return Some(make_heading_candidate(
                    ps,
                    hk.kind,
                    hk.block,
                    hk.numbering.clone(),
                    hk.prefix.clone(),
                    hk.title.clone(),
                    true,
                ));
            }
            if trie_matches_all(&SECTION_KEYWORDS_TRIE, &prefix) {
                return Some(make_body_heading_candidate(ps, 6, id, &prefix));
            }
            return Some(make_body_heading_candidate(ps, 0, id, &prefix));
        }
    }

    // Centered two-line heading branch.
    let above_id = ps.neighbors.above(b);
    let above = ps.ob(above_id);
    let gap = above.map_or(INF, |a| a.bottom_edge() - first_line.top_edge());
    let intersection = first_line.bottom_edge() - second_line.top_edge();
    let per_char = line_avg_char_width(first_line);
    let mut centered_flag = false;
    if let Some(fa) = first_anchor
        && fa.italic
        && let Some(nt) = next_token
        && !span_of(nt).italic
        && (gap > 1.1 * intersection
            || {
                let a = above.expect("finite gap implies an above block");
                tokenize_block(a, lay).last().is_some_and(is_word_token)
            }
            || {
                let a = above.expect("finite gap implies an above block");
                ps.d.last_line(ps.page, a).right_edge() < first_line.right_edge() - 8.0 * per_char
            })
        && (first_line.right_edge() > second_line.right_edge() - 4.0 * per_char
            || first_line.char_stats.last_cat != 6
            || second_line.char_stats.first_cat == 3)
    {
        for t in prefix.iter() {
            if t.first_cat == 2 {
                centered_flag = true;
                break;
            }
            if t.kind == 2 || t.boundary {
                break;
            }
        }
    }
    if centered_flag
        && token.is_some_and(is_trimmable_token)
        && next_token.is_some_and(|t| t.first_cat == 2)
    {
        if trie_matches_all(&SECTION_KEYWORDS_TRIE, &prefix) {
            return Some(make_body_heading_candidate(ps, 6, id, &prefix));
        }
        if letters > 1 {
            return Some(make_body_heading_candidate(ps, 0, id, &prefix));
        }
    }
    None
}
