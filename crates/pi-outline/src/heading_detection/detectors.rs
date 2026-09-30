//! Numbered, labeled, chapter/appendix, and box heading detectors plus acceptability checks.
//!
//! ref: pageindex/flash/heading_detection/detectors.py

use pi_layout::labels::caption_text::skip_bracketed_word;
use pi_layout::model::block::{BlockId, heading_score};
use pi_layout::model::char_stats::{info_weight, is_upper_dominant, letter_count};
use pi_layout::model::numbering::numbering_value_ro;
use pi_layout::model::rects::{Bounded, x_aligned, y_overlaps};
use pi_layout::model::span_line::line_avg_char_width;
use pi_layout::model::to_number;
use pi_layout::tokens::token_types::is_punct_category;
use pi_layout::tokens::{TokenView, is_word_token, token_numeric_value, tokenize_block};

use super::candidates::{
    PageScanState, make_heading_candidate, make_numbered_candidate, make_plain_candidate,
};
use super::keyword_tables::{
    APPENDIX_KEYWORDS_TRIE, APPENDIX_SECTION_TRIE, BOX_KEYWORD_TRIE, CHAPTER_WORDS_TRIE,
    roman_numeral,
};
use super::text_checks::{
    has_substantive_content, letter_to_ordinal, matches_abstract, matches_references,
    similar_style, token_to_number,
};
use crate::model::{Cand, Num, clamp};

/// Python `int(x)` on a float (truncation toward zero).
fn py_int(x: f64) -> Num {
    x.trunc()
}

/// Identify "1.2.3" / "[1]" style numbered heading prefixes.
// ref: heading_detection/detectors.py::detect_numbered_heading
pub fn detect_numbered_heading(
    ps: &PageScanState,
    id: BlockId,
    tokens: &TokenView,
) -> Option<Cand> {
    let b = ps.b(id);
    let page_font = ps.pg().layout.stats.median_font_size;
    let mut items: Vec<Num> = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let index = index as i64;
        if token.kind == 1 {
            if items.len() >= 4 {
                break;
            }
            let val = token_numeric_value(token);
            if val.is_nan() || val <= 0.0 || val >= 20.0 {
                break;
            }
            if token.len >= 3 {
                break;
            }
            items.push(py_int(val));
            if !token.boundary {
                continue;
            }
            let at = tokens.token_at(index + 1);
            if index + 2 < tokens.length
                && let Some(a) = at
                && (is_word_token(a) || a.kind == 6)
                && tokens.token_at(index + 2).is_some_and(|t| t.kind == 1)
            {
                break;
            }
            if items.len() > 1
                || heading_score(b) > page_font + 1.0
                || (!b.caption_claimed.get()
                    && at.is_some_and(|a| {
                        a.first_cat == 2
                            || a.first_cat == 4
                            || a.last_cat == 2
                            || a.kind == 4
                            || a.text == "."
                            || a.text == "|"
                    }))
            {
                return make_numbered_candidate(
                    ps,
                    id,
                    items,
                    tokens.slice(0, index + 1),
                    tokens.from(index + 1),
                );
            }
            return None;
        }
        if matches!(token.text.as_str(), "." | "．" | "｡" | "。") || token.kind == 4 {
            match tokens.token_at(index - 1) {
                Some(p) if p.kind == 1 => {}
                _ => break,
            }
            if !token.boundary {
                continue;
            }
            return make_numbered_candidate(
                ps,
                id,
                items,
                tokens.slice(0, index + 1),
                tokens.from(index + 1),
            );
        }
        if items.is_empty() || token.kind != 2 {
            break;
        }
        if !matches!(token.first_cat, 2 | 4) {
            break;
        }
        if items.len() > 1 || token.len >= 3 || tokens.length - index >= 3 {
            return make_numbered_candidate(
                ps,
                id,
                items,
                tokens.slice(0, index),
                tokens.from(index),
            );
        }
        return None;
    }
    None
}

/// Python `needle in haystack` for strings, returning the code-point index (`str.find`).
fn py_find(hay: &str, needle: &str) -> Option<usize> {
    hay.find(needle).map(|byte| hay[..byte].chars().count())
}

/// Roman, letter, CJK, and mixed-numbering headings.
// ref: heading_detection/detectors.py::detect_labeled_heading
pub fn detect_labeled_heading(ps: &PageScanState, id: BlockId, tokens: &TokenView) -> Option<Cand> {
    let b = ps.b(id);
    if tokens.length <= 1 {
        return None;
    }
    let first = tokens.token_at(0)?;
    let second = tokens.token_at(1)?;
    // Roman numeral path.
    if let Some(roman) = roman_numeral(&first.text)
        && is_word_token(second)
        && ".．｡。:)".contains(second.text.as_str())
    {
        let prefix = tokens.slice(0, 2);
        let rest = tokens.from(prefix.length);
        return Some(make_heading_candidate(
            ps,
            2,
            id,
            vec![roman as f64],
            Some(prefix),
            Some(rest),
            false,
        ));
    }
    // CJK number path.
    if let Some(pos) = py_find("一二三四五六七八九十", &first.text)
        && is_word_token(second)
    {
        let prefix = tokens.slice(0, 2);
        let rest = tokens.from(prefix.length);
        if rest.length <= 0 {
            return None;
        }
        return Some(make_heading_candidate(
            ps,
            3,
            id,
            vec![pos as f64 + 1.0],
            Some(prefix),
            Some(rest),
            false,
        ));
    }
    // Letter path.
    if tokens.length <= 1
        || (b.char_stats.first_cat == 3
            && (b.line_count() > 1 || is_punct_category(b.char_stats.last_cat)))
    {
        return None;
    }
    let letter_val = letter_to_ordinal(&first.text)?;
    if !(second.text == "." || second.text == ")") {
        let lay = &ps.pg().layout;
        let fa = first.first_anchor_span().expect("anchor span");
        let sa = second.first_anchor_span().expect("anchor span");
        let (fs, ss) = (&lay.spans[fa], &lay.spans[sa]);
        if !first.boundary
            || fa == sa
            || ss.left_edge() < fs.right_edge() + fs.bbox_width()
            || heading_score(b) < ps.pg().layout.stats.median_font_size + 1.0
            || (letter_count(&b.char_stats) as f64) / (tokens.length as f64) < 2.0
        {
            return None;
        }
    }
    let mut items: Vec<Num> = vec![letter_val as f64];
    let prefix = tokens.slice(0, if is_word_token(second) { 2 } else { 1 });
    let mut rest = tokens.from(prefix.length);
    if second.text == "."
        && !second.boundary
        && rest.length >= 2
        && let Some(fr) = rest.first()
        && fr.kind == 1
    {
        let heading = token_numeric_value(fr);
        if heading.is_nan() || heading <= 0.0 || heading >= 20.0 {
            return None;
        }
        items.push(py_int(heading));
        rest = rest.from(1);
        if rest.length > 0 && rest.first().is_some_and(is_word_token) {
            rest = rest.from(1);
        }
    }
    if rest.length <= 0 {
        return None;
    }
    let prefix = tokens.slice(0, tokens.length - rest.length);
    Some(make_heading_candidate(
        ps,
        4,
        id,
        items,
        Some(prefix),
        Some(rest),
        false,
    ))
}

/// "Chapter X" / "Appendix X" style headings.
// ref: heading_detection/detectors.py::detect_chapter_appendix
pub fn detect_chapter_appendix(ps: &PageScanState, id: BlockId) -> Option<Cand> {
    let b = ps.b(id);
    let score = heading_score(b);
    if score <= ps.pg().layout.stats.median_font_size + 0.1 {
        return None;
    }
    let body = ps.d.doc.stats.body_font_size;
    let flag = b.isolated_centered.get()
        || score > body + 0.1
            && (b.bold_frac > 0.9 || is_upper_dominant(&b.char_stats) || score > 1.5 * body);
    let tokens = ps.tokens(id);
    let mut m = None;
    if flag {
        m = CHAPTER_WORDS_TRIE.prefix_match(&tokens);
    }
    if flag && let Some(m) = &m {
        let value = token_to_number(tokens.token_at(m.length))?;
        let prefix = tokens.slice(0, skip_bracketed_word(&tokens, m.length + 1));
        let rest = tokens.from(prefix.length);
        return Some(make_heading_candidate(
            ps,
            8,
            id,
            vec![value],
            Some(prefix),
            Some(rest),
            false,
        ));
    }
    if flag && let Some(m) = APPENDIX_SECTION_TRIE.prefix_match(&tokens) {
        let prefix = tokens.slice(0, skip_bracketed_word(&tokens, m.length));
        let rest = tokens.from(prefix.length);
        return Some(make_heading_candidate(
            ps,
            9,
            id,
            Vec::new(),
            Some(prefix),
            Some(rest),
            false,
        ));
    }
    if let Some(m) = APPENDIX_KEYWORDS_TRIE.prefix_match(&tokens) {
        let next = tokens.token_at(m.length);
        // `token_to_number(t) or letter_to_ordinal(...)`: token_to_number never yields 0.
        let val = token_to_number(next).or_else(|| {
            next.and_then(|t| letter_to_ordinal(&t.text))
                .map(|v| v as f64)
        });
        if !flag && val.is_none() {
            return None;
        }
        let items = val.map(|v| vec![v]).unwrap_or_default();
        let skip = m.length + if val.is_some() { 1 } else { 0 };
        let prefix = tokens.slice(0, skip_bracketed_word(&tokens, skip));
        let rest = tokens.from(prefix.length);
        return Some(make_heading_candidate(
            ps,
            10,
            id,
            items,
            Some(prefix),
            Some(rest),
            false,
        ));
    }
    None
}

/// "Box N" headings.
// ref: heading_detection/detectors.py::detect_box_heading
pub fn detect_box_heading(ps: &PageScanState, id: BlockId) -> Option<Cand> {
    let tokens = ps.tokens(id);
    let m = BOX_KEYWORD_TRIE.prefix_match(&tokens)?;
    let rest = tokens.from(m.length);
    if rest.length <= 0 || rest.token_at(0).is_some_and(|t| t.kind != 1) {
        return None;
    }
    let val = token_numeric_value(rest.token_at(0).expect("len > 0"));
    if val.is_nan() || val <= 0.0 {
        return None;
    }
    let prefix = tokens.slice(0, skip_bracketed_word(&tokens, m.length + 1));
    let r = tokens.from(prefix.length);
    Some(make_heading_candidate(
        ps,
        12,
        id,
        vec![py_int(val)],
        Some(prefix),
        Some(r),
        false,
    ))
}

/// Sequential dispatch through the type detectors, falling back to a plain candidate.
// ref: heading_detection/detectors.py::classify_heading
pub fn classify_heading(ps: &PageScanState, id: BlockId) -> Cand {
    let tokens = ps.tokens(id);
    let h = detect_chapter_appendix(ps, id)
        .or_else(|| detect_box_heading(ps, id))
        .or_else(|| detect_numbered_heading(ps, id, &tokens))
        .or_else(|| detect_labeled_heading(ps, id, &tokens));
    if let Some(h) = h {
        return h;
    }
    let kind = if matches_references(&tokens) {
        7
    } else if matches_abstract(&tokens) {
        5
    } else {
        0
    };
    make_plain_candidate(ps, kind, id)
}

/// The "is this an acceptable heading?" gate.
// ref: heading_detection/detectors.py::is_acceptable_heading
pub fn is_acceptable_heading(ps: &PageScanState, c: &Cand) -> bool {
    let h = ps.b(c.block);
    let pg = ps.pg();
    let st = &pg.layout.stats;
    let ds = &ps.d.doc.stats;
    if h.bbox_height() >= 2.0 * h.bbox_width()
        || info_weight(&h.char_stats) <= 3.0
        || h.line_count() > 5
        || h.char_count() >= 300
    {
        return false;
    }
    let page_height = pg.layout.bounds.bbox_height();
    if h.bottom_edge() > 0.95 * page_height {
        return false;
    }
    let score = heading_score(h);
    let doc_group = ds.body_font_size;
    if score <= st.median_font_size + 0.5 && score <= doc_group + 0.5 && !c.is_prominent {
        return false;
    }
    let width = pg.layout.bounds.bbox_width();
    if (h.left_edge() > 0.55 * width && score <= doc_group + 5.0)
        || h.left_edge() > 0.75 * width
        || (h.left_edge() > 0.4 * width
            && h.center_x() > 0.6 * width
            && st.total_line_weight > pi_pycompat::pymath::min(1000.0, ds.median_page_weight))
    {
        return false;
    }
    let ci = ps.d.column_index(ps.page, h);
    let col = (0 <= ci && (ci as usize) < pg.layout.columns.len())
        .then(|| &pg.layout.columns[ci as usize]);
    if h.bbox_width() < 0.2 * width
        && let Some(col) = col
        && col.bbox_width() < 0.2 * width
        && col.bbox_height() > 1.5 * col.bbox_width()
    {
        return false;
    }
    let neighbor = ps.ob(ps.neighbors.right(h));
    let gap = neighbor.map_or(f64::INFINITY, |n| h.bottom_edge() - n.top_edge());
    let line_gap = st.median_overlap_gap - st.median_font_size;
    if gap < 0.9 * line_gap {
        return false;
    }
    let above = ps.ob(ps.neighbors.above(h));
    let above_gap = above.map_or(f64::INFINITY, |a| a.bottom_edge() - h.top_edge());
    if above_gap < 0.9 * line_gap {
        return false;
    }
    let prev = ps.prev.map(|p| ps.d.page(p));
    if let Some(pp) = prev
        && !pp.has_caption.get()
        && (pp.layout.stats.total_line_weight < clamp(ds.median_page_weight, 200.0, 500.0)
            || !pp.has_body.get())
        && score > doc_group + 0.5
    {
        return true;
    }
    let doc_center = ds.median_center_y;
    let prev_center = prev.map_or(f64::NAN, |pp| pp.layout.stats.median_center_y);
    let prev_height = prev.map_or(f64::NAN, |pp| pp.layout.bounds.bbox_height());
    let centered = h.isolated_centered.get();
    if prev_center <= doc_center
        && (score <= doc_group + 1.5 || (score <= doc_group + 5.0 && !centered))
        || h.density_chars < 0.5 * ds.p80_density
    {
        return false;
    }
    let body_neighbor = ps.ob(ps.neighbors.closest_body(h));
    if let Some(bn) = body_neighbor
        && h.bottom_edge() - bn.top_edge() > 2.0 * h.bbox_height()
        && bn.caption_label.get() != 0
    {
        return false;
    }
    let o = h.orig_index.get() as i64;
    let prev_block = ps.ob(ps.output_at(o - 1));
    let next_block = ps.ob(ps.output_at(o + 1));
    if has_substantive_content(pg, h, prev_block, next_block) {
        return false;
    }
    (prev_center > doc_center + 0.1 * prev_height
        && (score > doc_group + 2.0
            || (prev_center > doc_center + 0.2 * prev_height
                && body_neighbor.is_some()
                && neighbor.is_some_and(|n| gap > n.weighted_font_size))))
        || (centered && neighbor.is_none_or(|n| n.caption_label.get() == 0))
        || score > 1.5 * doc_group
        || (c.numbering.len() == 1 && c.numbering[0] == 1.0 && above.is_none())
}

/// Candidate plus the rejection gates.
// ref: heading_detection/detectors.py::try_classify_heading
pub fn try_classify_heading(ps: &PageScanState, id: BlockId) -> Option<Cand> {
    let c = classify_heading(ps, id);
    if c.numbering.len() > 1 {
        return None;
    }
    if matches!(c.kind, 8..=10) {
        return Some(c);
    }
    if c.kind == 12 {
        return None;
    }
    is_acceptable_heading(ps, &c).then_some(c)
}

/// Block is too wide / central to be a heading.
// ref: heading_detection/detectors.py::is_too_wide_for_heading
pub fn is_too_wide_for_heading(ps: &PageScanState, id: BlockId) -> bool {
    let width = ps.b(id).bbox_width();
    if width > 0.7 * ps.pg().layout.bounds.bbox_width() / 2.0
        || width > 0.7 * ps.d.doc.stats.median_line_width
    {
        return true;
    }
    let pi = ps.d.page_index(ps.page) as i64;
    let n = ps.d.pages().len() as i64;
    let mut count = 0;
    for h in (pi - 1)..(pi + 2) {
        if 0 < h && h <= n {
            let page = ps.d.page((h - 1) as usize);
            if width > 0.7 * page.layout.stats.median_line_width {
                count += 1;
            }
        }
    }
    count >= 2
}

/// Neighbor-aware gate: true means the caller rejects the block.
// ref: heading_detection/detectors.py::passes_neighbor_check
pub fn passes_neighbor_check(ps: &PageScanState, id: BlockId) -> bool {
    if is_too_wide_for_heading(ps, id) {
        return false;
    }
    let b = ps.b(id);
    let o = b.orig_index.get() as i64;
    let cand = ps.output_at(o - 1);
    let overlap = cand.is_some_and(|c| y_overlaps(b, ps.b(c)));
    if overlap && is_too_wide_for_heading(ps, cand.expect("overlap")) {
        return false;
    }
    let cand = ps.output_at(o + 1);
    let next_overlap = cand.is_some_and(|c| y_overlaps(b, ps.b(c)));
    if next_overlap && is_too_wide_for_heading(ps, cand.expect("overlap")) {
        return false;
    }
    if !overlap && !next_overlap {
        return false;
    }
    let right = ps.neighbors.right(b);
    if let Some(r) = right
        && is_too_wide_for_heading(ps, r)
    {
        return false;
    }
    if let Some(r) = right.map(|r| ps.b(r))
        && !r.is_body_paragraph.get()
        && r.line_count() > 3
        && r.bbox_height() > 0.8 * r.bbox_width()
    {
        return true;
    }
    let above_or_overlap = ps.neighbors.closest_body(b);
    let threshold = 4.0 * line_avg_char_width(ps.d.first_line(ps.page, b));
    if let Some(a) = above_or_overlap
        && right != Some(a)
        && x_aligned(b, ps.b(a), threshold)
        && ps.b(a).region_label.get() == 0
        && is_too_wide_for_heading(ps, a)
    {
        return false;
    }
    if let Some(k) = ps.neighbors.body_above(b)
        && x_aligned(b, ps.b(k), threshold)
        && ps.b(k).region_label.get() == 0
        && is_too_wide_for_heading(ps, k)
    {
        return false;
    }
    true
}

/// Competing labeled-heading sibling check.
// ref: heading_detection/detectors.py::has_competing_labeled_heading
pub fn has_competing_labeled_heading(ps: &PageScanState, c: &Cand, other: BlockId) -> bool {
    let ob = ps.b(other);
    if ob.kind.get() != 0 || ob.char_count() >= 500 {
        return false;
    }
    let b = ps.b(c.block);
    if !similar_style(b, ob) || (b.top_edge() - ob.top_edge()).abs() >= 5.0 * b.bbox_height() {
        return false;
    }
    let Some(oc) = detect_labeled_heading(ps, other, &ps.tokens(other)) else {
        return false;
    };
    if c.kind != oc.kind {
        return false;
    }
    (oc.numbering[0] - c.numbering[0]).abs() >= 1.0
}

/// Plausible year (1700..2100).
// ref: heading_detection/detectors.py::is_year_string
pub fn is_year_string(text: &str) -> bool {
    let v = to_number(text);
    !v.is_nan() && 1700.0 < v && v < 2100.0
}

/// Whether a block looks like a bibliography entry.
// ref: heading_detection/detectors.py::is_bibliography_entry
pub fn is_bibliography_entry(ps: &PageScanState, id: BlockId, mut other_number: i64) -> bool {
    let b = ps.b(id);
    let lay = &ps.pg().layout;
    if other_number < 0 {
        other_number = 0;
        for &lid in &b.lines {
            let v = numbering_value_ro(&lay.lines[lid], &lay.spans);
            if !v.is_nan() && 0.0 < v && v <= 9999.0 {
                other_number += 1;
            }
        }
    }
    if other_number < 2 && b.char_count() as f64 / (other_number.max(1) as f64) > 300.0 {
        return false;
    }
    let (mut year, mut digit, mut word, mut period_after_word, mut word_state) =
        (0i64, 0i64, 0i64, 0i64, 0i64);
    let tokens = tokenize_block(b, lay);
    for t in tokens.iter() {
        if is_word_token(t) {
            if t.kind == 3 && word_state == 1 {
                period_after_word += 1;
            }
            word_state = 0;
        } else if t.kind == 1 {
            let k = token_numeric_value(t);
            if 0.0 < k && k < 1000.0 {
                digit += 1;
            } else if is_year_string(&t.text) {
                year += 1;
            }
        } else if t.kind == 2 {
            word += 1;
            word_state += 1;
        }
    }
    if (word as f64) < 0.1 * tokens.length as f64 {
        return false;
    }
    let n = other_number as f64;
    digit as f64 >= 1.5 * n || year as f64 >= 0.5 * n || period_after_word as f64 >= 0.5 * n
}
