//! Heading detectors, text checks, the acceptability gate and `find_section_openers`.
//!
//! ref: pageindex/flash/heading_detection/{detectors,candidates,text_checks,keyword_tables,page_scan}.py
//! Only what `find_section_openers` reaches is ported here (the per-page `scan_page_headings`
//! scan belongs to stage 06).

use std::collections::HashSet;
use std::sync::LazyLock;

use pi_pycompat::{pymath, unicode};

use super::candidate::{HeadingCandidate, OutlineContext, OutlineNode};
use super::neighbors::PageNeighborMap;
use crate::blocks::join_rules::{dict_list, dict_trie};
use crate::classification::keyword_tables::{APPENDIX_SECTION_TRIE, BOX_KEYWORD_TRIE};
use crate::labels::caption_text::{
    extract_structural_number, skip_bracketed_word, trie_matches_all,
};
use crate::model::block::{Block, BlockId, block_text, heading_score, strip_diacritics, tokens_of};
use crate::model::char_stats::{info_weight, is_upper_dominant, letter_count};
use crate::model::rects::{Bounded, y_overlaps};
use crate::model::span_line::line_avg_char_width;
use crate::phases::{DocPage, Document};
use crate::tokens::{
    Token, TokenView, Trie, clamp_value, is_trimmable_token, is_word_token, strip_trie_match,
    token_numeric_value, tokenize_block,
};

// --- keyword tables (ref: heading_detection/keyword_tables.py) ---

/// ref: heading_detection/keyword_tables.py:34
static ABSTRACT_KEYWORDS_TRIE: LazyLock<Trie> = LazyLock::new(|| dict_trie(&["abstract_keywords"]));
/// ref: heading_detection/keyword_tables.py:35
static REFERENCES_TRIE: LazyLock<Trie> = LazyLock::new(|| dict_trie(&["references"]));
/// ref: heading_detection/keyword_tables.py:40
static CHAPTER_WORDS_TRIE: LazyLock<Trie> = LazyLock::new(|| dict_trie(&["chapter_words"]));
/// ref: heading_detection/keyword_tables.py:41
static APPENDIX_KEYWORDS_TRIE: LazyLock<Trie> = LazyLock::new(|| dict_trie(&["appendix_keywords"]));
/// ref: heading_detection/keyword_tables.py:50
static ABSTRACT_KEYWORDS_SET: LazyLock<HashSet<String>> = LazyLock::new(|| {
    dict_list("abstract_keywords")
        .iter()
        .map(|s| strip_diacritics(&unicode::lower(s)))
        .collect()
});
/// ref: heading_detection/keyword_tables.py:51
static REFERENCES_SET: LazyLock<HashSet<String>> = LazyLock::new(|| {
    dict_list("references")
        .iter()
        .map(|s| unicode::lower(s))
        .collect()
});
/// ref: heading_detection/keyword_tables.py:63
static EQUATION_KEYWORDS_TRIE: LazyLock<Trie> = LazyLock::new(|| {
    Trie::build(
        [
            "equation",
            "equation.",
            "eqn",
            "eqn.",
            "eq",
            "eq.",
            "ecuación",
            "equação",
            "gleichung",
            "equazione",
            "ekvation",
            "yhtälö",
            "ligning",
            "persamaan",
            "denklem",
            "ecuația",
            "equació",
            "rovnica",
            "rovnice",
            "równanie",
            "vergelijking",
            "jednadžba",
            "jöfnu",
            "võrrand",
            "vienādojums",
            "lygtis",
            "enačba",
            "egyenlet",
            "phương trình",
            "εξίσωση",
            "方程",
            "방정식",
            "уравнение",
            "рівняння",
            "раўнанне",
            "једначина",
        ],
        true,
        false,
    )
});

/// ref: heading_detection/keyword_tables.py:79 `ENGLISH_WORD_TO_NUMBER`
fn english_number(s: &str) -> Option<f64> {
    const W: [&str; 20] = [
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
    ];
    W.iter().position(|&w| w == s).map(|i| i as f64 + 1.0)
}

/// ref: heading_detection/keyword_tables.py:85 `ROMAN_NUMERAL_MAP`
fn roman_number(s: &str) -> Option<f64> {
    crate::classification::body_text::roman_value(s).map(|v| v as f64)
}

/// ref: heading_detection/keyword_tables.py:93 `FORMULA_CHAR_WEIGHTS`
fn formula_weight(s: &str) -> Option<f64> {
    Some(match s {
        "=" => 10.0,
        "{" | "}" | "+" => 5.0,
        "/" | "*" => 3.0,
        "-" | "~" | "[" | "]" | "(" | ")" => 1.0,
        _ => return None,
    })
}

/// `DEAD_DIGIT_RE.match(s)` with stdlib `re`: `^.p\{Number\}+.$`, i.e. one non-newline char,
/// the literal `p{Number`, one or more `}`, one non-newline char, then the end (or a final
/// newline). ref: heading_detection/keyword_tables.py:60
fn dead_digit_match(s: &str) -> bool {
    let mut it = s.chars();
    let Some(c0) = it.next() else { return false };
    if c0 == '\n' {
        return false;
    }
    let rest = it.as_str();
    let Some(rest) = rest.strip_prefix("p{Number") else {
        return false;
    };
    let braces = rest.len() - rest.trim_start_matches('}').len();
    if braces == 0 {
        return false;
    }
    // `\}+` backtracks: the final `.` may consume the last brace.
    let tail = &rest[braces..];
    let ok_end = |t: &str| t.is_empty() || t == "\n";
    let mut chars = tail.chars();
    match chars.next() {
        Some(c) if c != '\n' && ok_end(chars.as_str()) => true,
        None if braces >= 2 => true, // `.` takes the last '}'
        Some('\n') if braces >= 2 && chars.as_str().is_empty() => true,
        _ => false,
    }
}

// --- text checks (ref: heading_detection/text_checks.py) ---

// ref: heading_detection/text_checks.py::matches_abstract
fn matches_abstract(tokens: &TokenView) -> bool {
    if trie_matches_all(&ABSTRACT_KEYWORDS_TRIE, tokens) {
        return true;
    }
    if tokens.length > 10 {
        return false;
    }
    let mut n = String::new();
    let mut nlen = 0;
    for t in tokens.iter() {
        if is_word_token(t) {
            continue;
        }
        if t.kind != 2 || nlen + t.len > 20 {
            return false;
        }
        let add = strip_diacritics(&unicode::lower(&t.text));
        nlen += add.chars().count();
        n.push_str(&add);
    }
    ABSTRACT_KEYWORDS_SET.contains(&n)
}

// ref: heading_detection/text_checks.py::matches_references
pub fn matches_references(tokens: &TokenView) -> bool {
    let Some(m) = REFERENCES_TRIE.prefix_match(tokens) else {
        if tokens.length <= 15 {
            let mut n = String::new();
            let mut nlen = 0;
            for t in tokens.iter() {
                if is_word_token(t) {
                    continue;
                }
                if t.kind != 2 || nlen + t.len > 20 {
                    return false;
                }
                let add = unicode::lower(&t.text);
                nlen += add.chars().count();
                n.push_str(&add);
            }
            return REFERENCES_SET.contains(&n);
        }
        return false;
    };
    if m.length == tokens.length {
        return true;
    }
    let rest = tokens.from(m.length);
    if rest.length == 1 && rest.token_at(0).is_some_and(is_word_token) {
        return true;
    }
    trie_matches_all(&REFERENCES_TRIE, &rest)
}

// ref: heading_detection/text_checks.py::is_equation_adjacent_line
fn is_equation_adjacent_line(line: Option<&Block>, block: &Block, page: &DocPage) -> bool {
    let Some(line) = line else { return false };
    if line.line_count() != 1 {
        return false;
    }
    if line.left_edge() < block.right_edge() || !y_overlaps(block, line) {
        return false;
    }
    if dead_digit_match(block_text(line, &page.layout)) {
        return true;
    }
    let mut toks = tokenize_block(line, &page.layout);
    if toks.length >= 3
        && let (Some(f), Some(l)) = (toks.first(), toks.last())
        && f.len <= 1
        && f.kind != 1
        && l.len <= 1
        && l.kind != 1
    {
        toks = toks.slice(1, toks.length - 1);
    }
    let toks = strip_trie_match(&toks, &EQUATION_KEYWORDS_TRIE);
    extract_structural_number(&toks, true).is_some_and(|m| m.length == toks.length)
}

// ref: heading_detection/text_checks.py::has_substantive_content
fn has_substantive_content(
    block: &Block,
    prev: Option<&Block>,
    next: Option<&Block>,
    page: &DocPage,
) -> bool {
    let pl = &page.layout;
    let mut e = 0.0;
    for t in tokenize_block(block, pl).iter() {
        let (Some(sid), Some(lid)) = (t.first_anchor_span(), t.line()) else {
            continue;
        };
        let anchor = &pl.spans[sid];
        let line = &pl.lines[lid];
        let size = line.max_span_height;
        let flag = anchor.top_edge() < line.bottom_edge() + 0.8 * size
            || anchor.bottom_edge() > line.top_edge() - 0.8 * size;
        if t.kind == 1 {
            e += if flag { 2.0 } else { 1.0 };
            continue;
        }
        if let Some(w) = formula_weight(&t.text) {
            e += if flag { 3.0 } else { 1.0 } * w;
            continue;
        }
        if t.kind == 6 {
            e += if flag { 3.0 } else { 1.0 } * 5.0;
            continue;
        }
        if t.len <= 3 && t.first_cat != 4 {
            if flag {
                e += if is_word_token(t) { 5.0 } else { 1.0 };
            }
            continue;
        }
        if flag {
            continue;
        }
        let lv = if anchor.bold { 2.0 } else { 1.0 } * t.len as f64;
        match t.first_cat {
            4 => e -= 2.0 * lv,
            2 => e -= lv,
            3 => e -= 0.5 * lv,
            _ => {}
        }
    }
    if e < 0.0 {
        return false;
    }
    if e >= 5.0 {
        return true;
    }
    is_equation_adjacent_line(prev, block, page) || is_equation_adjacent_line(next, block, page)
}

// ref: heading_detection/text_checks.py::token_to_number
fn token_to_number(t: Option<&Token>) -> Option<f64> {
    let t = t?;
    if t.kind == 1 {
        let v = token_numeric_value(t);
        return (!v.is_nan() && v > 0.0).then_some(v);
    }
    roman_number(&t.text).or_else(|| english_number(&unicode::lower(&t.text)))
}

// ref: heading_detection/text_checks.py::letter_to_ordinal
fn letter_to_ordinal(s: &str) -> Option<f64> {
    let mut it = s.chars();
    let c = it.next()?;
    if it.next().is_some() {
        return None;
    }
    let low = unicode::lower(&c.to_string());
    let first = low.chars().next()? as u32;
    let unit = if first > 0xFFFF {
        0xD800 + ((first - 0x10000) >> 10)
    } else {
        first
    };
    let v = unit as i64 - 96;
    (1..=8).contains(&v).then_some(v as f64)
}

// --- page scan state and candidate constructors (ref: heading_detection/candidates.py) ---

/// ref: heading_detection/candidates.py::PageScanState
pub struct PageScanState<'a> {
    pub doc: &'a Document,
    pub page: usize,
    pub prev_page: Option<usize>,
    pub neighbors: PageNeighborMap,
}

impl<'a> PageScanState<'a> {
    pub fn new(doc: &'a Document, pi0: usize) -> Self {
        let page = &doc.pages[pi0];
        PageScanState {
            doc,
            page: pi0,
            prev_page: (page.index() >= 2).then(|| page.index() - 2),
            neighbors: PageNeighborMap::new(page),
        }
    }

    fn p(&self) -> &'a DocPage {
        &self.doc.pages[self.page]
    }

    fn b(&self, id: BlockId) -> &'a Block {
        &self.doc.pages[self.page].blocks[id]
    }

    /// `page.output_slot[i]` by original index, or `None`.
    fn output_at(&self, i: i64) -> Option<BlockId> {
        let out = &self.p().output;
        (i >= 0 && (i as usize) < out.len()).then(|| out[i as usize])
    }
}

// ref: heading_detection/candidates.py::make_heading_candidate
#[allow(clippy::too_many_arguments)]
fn make_heading_candidate(
    ps: &PageScanState,
    kind: u8,
    bid: BlockId,
    numbering: Vec<f64>,
    tokens: Option<TokenView>,
    title: Option<TokenView>,
    mut has_numbering: bool,
) -> HeadingCandidate {
    let page = ps.p();
    let pl = &page.layout;
    let block = ps.b(bid);
    let right = ps.neighbors.right(block.orig_index.get());
    if !has_numbering
        && let Some(r) = right
        && block.italic_frac > 0.9
        && let Some(t) = &title
        && t.last().is_some_and(is_trimmable_token)
    {
        let r = ps.b(r);
        let mh = &pl.lines[block.last_line()];
        if mh.bbox_width() > 0.7 * r.bbox_width()
            && (mh.right_edge() - r.right_edge()).abs() < 2.0 * line_avg_char_width(mh)
        {
            has_numbering = true;
        }
    }
    let prominent =
        kind == 7 || (!numbering.is_empty() && title.as_ref().is_some_and(matches_references));
    HeadingCandidate::new(
        ps.doc,
        ps.page,
        kind,
        bid,
        ps.neighbors.closest_body_above(block.orig_index.get()),
        numbering,
        tokens,
        title,
        has_numbering,
        prominent,
    )
}

// ref: heading_detection/candidates.py::make_plain_candidate
fn make_plain_candidate(ps: &PageScanState, kind: u8, bid: BlockId) -> HeadingCandidate {
    let toks = tokenize_block(ps.b(bid), &ps.p().layout);
    make_heading_candidate(ps, kind, bid, Vec::new(), None, Some(toks), false)
}

// ref: heading_detection/candidates.py::_di_count
fn di_count(tokens: &TokenView, line: usize) -> i64 {
    let mut n = 0;
    for i in 0..tokens.length {
        match tokens.token_at(i) {
            Some(t) if t.line() == Some(line) => n += 1,
            _ => break,
        }
    }
    n
}

// ref: heading_detection/candidates.py::make_numbered_candidate
fn make_numbered_candidate(
    ps: &PageScanState,
    bid: BlockId,
    items: Vec<f64>,
    tokens: TokenView,
    title: TokenView,
) -> Option<HeadingCandidate> {
    let page = ps.p();
    let pl = &page.layout;
    let block = ps.b(bid);
    if title.length <= 0 {
        return None;
    }
    if title.length == 1
        && let Some(ft) = title.first()
        && ft.first_cat != 2
        && ft.first_cat != 4
        && ft.last_cat != 2
        && !block.isolated_centered.get()
    {
        return None;
    }
    if items.len() == 1
        && items[0] == 1.0
        && block.top_edge() < 0.3 * pl.bounds.bbox_height()
        && ps.neighbors.right(block.orig_index.get()).is_none()
        && let Some(lt) = title.last()
        && lt
            .last_anchor()
            .is_some_and(|a| a.line == Some(block.last_line()))
    {
        return None;
    }
    let mut reject = false;
    if block.line_count() > 1 {
        let second_line = &pl.lines[block.lines[1]];
        if let (Some(fnt), Some(ftt)) = (tokens.first(), title.first())
            && let (Some(a), Some(b)) = (fnt.first_anchor_span(), ftt.first_anchor_span())
        {
            let left = pl.spans[a].left_edge();
            let title_left = pl.spans[b].left_edge();
            if !(left < title_left && second_line.left_edge() > (left + title_left) / 2.0) {
                let all = tokenize_block(block, pl);
                let trailing = all.from(di_count(&all, block.first_line()));
                match extract_structural_number(&trailing, true) {
                    None => reject = false,
                    Some(t) if t.length <= 0 => reject = false,
                    Some(_) if block.caption_claimed.get() => reject = true,
                    Some(t) => {
                        reject = if items.len() == 1 && t.length <= 2 {
                            match t.token_at(0) {
                                Some(t0) => {
                                    let v = token_numeric_value(t0);
                                    !v.is_nan() && v == items[0] + 1.0
                                }
                                None => false,
                            }
                        } else {
                            false
                        };
                    }
                }
            }
        }
    }
    if reject {
        return None;
    }
    Some(make_heading_candidate(
        ps,
        1,
        bid,
        items,
        Some(tokens),
        Some(title),
        false,
    ))
}

// --- detectors (ref: heading_detection/detectors.py) ---

const PERIODS: [&str; 4] = [".", "\u{FF0E}", "\u{FF61}", "\u{3002}"];

// ref: heading_detection/detectors.py::detect_numbered_heading
fn detect_numbered_heading(
    ps: &PageScanState,
    bid: BlockId,
    tokens: &TokenView,
) -> Option<HeadingCandidate> {
    let block = ps.b(bid);
    let page_fs = ps.p().layout.stats.median_font_size;
    let mut items: Vec<f64> = Vec::new();
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
            items.push(val.trunc());
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
                || heading_score(block) > page_fs + 1.0
                || (!block.caption_claimed.get()
                    && at.is_some_and(|a| {
                        matches!(a.first_cat, 2 | 4)
                            || a.last_cat == 2
                            || a.kind == 4
                            || a.text == "."
                            || a.text == "|"
                    }))
            {
                return make_numbered_candidate(
                    ps,
                    bid,
                    items,
                    tokens.slice(0, index + 1),
                    tokens.from(index + 1),
                );
            }
            return None;
        }
        if PERIODS.contains(&token.text.as_str()) || token.kind == 4 {
            match tokens.token_at(index - 1) {
                Some(p) if p.kind == 1 => {}
                _ => break,
            }
            if !token.boundary {
                continue;
            }
            return make_numbered_candidate(
                ps,
                bid,
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
                bid,
                items,
                tokens.slice(0, index),
                tokens.from(index),
            );
        }
        return None;
    }
    None
}

// ref: heading_detection/detectors.py::detect_labeled_heading
fn detect_labeled_heading(
    ps: &PageScanState,
    bid: BlockId,
    tokens: &TokenView,
) -> Option<HeadingCandidate> {
    let block = ps.b(bid);
    let pl = &ps.p().layout;
    if tokens.length <= 1 {
        return None;
    }
    let first = tokens.token_at(0)?;
    let second = tokens.token_at(1)?;
    if let Some(roman) = roman_number(&first.text)
        && is_word_token(second)
        && ".\u{FF0E}\u{FF61}\u{3002}:)".contains(second.text.as_str())
    {
        let prefix = tokens.slice(0, 2);
        let rest = tokens.from(prefix.length);
        return Some(make_heading_candidate(
            ps,
            2,
            bid,
            vec![roman],
            Some(prefix),
            Some(rest),
            false,
        ));
    }
    const CJK: &str = "一二三四五六七八九十";
    if let Some(byte_pos) = CJK.find(first.text.as_str())
        && is_word_token(second)
    {
        let pos = CJK[..byte_pos].chars().count();
        let prefix = tokens.slice(0, 2);
        let rest = tokens.from(prefix.length);
        if rest.length <= 0 {
            return None;
        }
        return Some(make_heading_candidate(
            ps,
            3,
            bid,
            vec![pos as f64 + 1.0],
            Some(prefix),
            Some(rest),
            false,
        ));
    }
    if tokens.length <= 1
        || (block.char_stats.first_cat == 3
            && (block.line_count() > 1 || matches!(block.char_stats.last_cat, 6..=8)))
    {
        return None;
    }
    let value = letter_to_ordinal(&first.text)?;
    if !(second.text == "." || second.text == ")") {
        let (Some(fa), Some(sa)) = (first.first_anchor_span(), second.first_anchor_span()) else {
            return None;
        };
        let (f, s) = (&pl.spans[fa], &pl.spans[sa]);
        if !first.boundary
            || fa == sa
            || s.left_edge() < f.right_edge() + f.bbox_width()
            || heading_score(block) < pl.stats.median_font_size + 1.0
            || letter_count(&block.char_stats) as f64 / (tokens.length as f64) < 2.0
        {
            return None;
        }
    }
    let mut items = vec![value];
    let prefix = tokens.slice(0, if is_word_token(second) { 2 } else { 1 });
    let mut rest = tokens.from(prefix.length);
    if second.text == "."
        && !second.boundary
        && rest.length >= 2
        && let Some(fr) = rest.first()
        && fr.kind == 1
    {
        let h = token_numeric_value(fr);
        if h.is_nan() || h <= 0.0 || h >= 20.0 {
            return None;
        }
        items.push(h.trunc());
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
        bid,
        items,
        Some(prefix),
        Some(rest),
        false,
    ))
}

// ref: heading_detection/detectors.py::detect_chapter_appendix
fn detect_chapter_appendix(ps: &PageScanState, bid: BlockId) -> Option<HeadingCandidate> {
    let block = ps.b(bid);
    let pl = &ps.p().layout;
    let dfs = ps.doc.stats.body_font_size;
    let ci = heading_score(block);
    if ci <= pl.stats.median_font_size + 0.1 {
        return None;
    }
    let flag = block.isolated_centered.get()
        || (ci > dfs + 0.1
            && (block.bold_frac > 0.9 || is_upper_dominant(&block.char_stats) || ci > 1.5 * dfs));
    let toks = tokens_of(block, pl);
    if flag && let Some(m) = CHAPTER_WORDS_TRIE.prefix_match(&toks) {
        let value = token_to_number(toks.token_at(m.length))?;
        let prefix = toks.slice(0, skip_bracketed_word(&toks, m.length + 1));
        let rest = toks.from(prefix.length);
        return Some(make_heading_candidate(
            ps,
            8,
            bid,
            vec![value],
            Some(prefix),
            Some(rest),
            false,
        ));
    }
    if flag && let Some(m) = APPENDIX_SECTION_TRIE.prefix_match(&toks) {
        let prefix = toks.slice(0, skip_bracketed_word(&toks, m.length));
        let rest = toks.from(prefix.length);
        return Some(make_heading_candidate(
            ps,
            9,
            bid,
            Vec::new(),
            Some(prefix),
            Some(rest),
            false,
        ));
    }
    if let Some(m) = APPENDIX_KEYWORDS_TRIE.prefix_match(&toks) {
        let next = toks.token_at(m.length);
        let val = token_to_number(next).or_else(|| next.and_then(|n| letter_to_ordinal(&n.text)));
        if !flag && val.is_none() {
            return None;
        }
        let items = val.map(|v| vec![v]).unwrap_or_default();
        let prefix = toks.slice(
            0,
            skip_bracketed_word(&toks, m.length + if val.is_some() { 1 } else { 0 }),
        );
        let rest = toks.from(prefix.length);
        return Some(make_heading_candidate(
            ps,
            10,
            bid,
            items,
            Some(prefix),
            Some(rest),
            false,
        ));
    }
    None
}

// ref: heading_detection/detectors.py::detect_box_heading
fn detect_box_heading(ps: &PageScanState, bid: BlockId) -> Option<HeadingCandidate> {
    let toks = tokens_of(ps.b(bid), &ps.p().layout);
    let m = BOX_KEYWORD_TRIE.prefix_match(&toks)?;
    let rest = toks.from(m.length);
    if rest.length <= 0 || rest.token_at(0).is_none_or(|t| t.kind != 1) {
        return None;
    }
    let val = token_numeric_value(rest.token_at(0)?);
    if val.is_nan() || val <= 0.0 {
        return None;
    }
    let prefix = toks.slice(0, skip_bracketed_word(&toks, m.length + 1));
    let rest = toks.from(prefix.length);
    Some(make_heading_candidate(
        ps,
        12,
        bid,
        vec![val.trunc()],
        Some(prefix),
        Some(rest),
        false,
    ))
}

// ref: heading_detection/detectors.py::classify_heading
fn classify_heading(ps: &PageScanState, bid: BlockId) -> HeadingCandidate {
    let toks = tokens_of(ps.b(bid), &ps.p().layout);
    let h = detect_chapter_appendix(ps, bid)
        .or_else(|| detect_box_heading(ps, bid))
        .or_else(|| detect_numbered_heading(ps, bid, &toks))
        .or_else(|| detect_labeled_heading(ps, bid, &toks));
    if let Some(h) = h {
        return h;
    }
    let kind = if matches_references(&toks) {
        7
    } else if matches_abstract(&toks) {
        5
    } else {
        0
    };
    make_plain_candidate(ps, kind, bid)
}

// ref: heading_detection/detectors.py::is_acceptable_heading
fn is_acceptable_heading(ps: &PageScanState, c: &HeadingCandidate) -> bool {
    let doc = ps.doc;
    let page = ps.p();
    let pl = &page.layout;
    let h = ps.b(c.block);
    if h.bbox_height() >= 2.0 * h.bbox_width()
        || info_weight(&h.char_stats) <= 3.0
        || h.line_count() > 5
        || h.char_count() >= 300
    {
        return false;
    }
    let ph = pl.bounds.bbox_height();
    if h.bottom_edge() > 0.95 * ph {
        return false;
    }
    let score = heading_score(h);
    let dg = doc.stats.body_font_size;
    if score <= pl.stats.median_font_size + 0.5 && score <= dg + 0.5 && !c.is_prominent {
        return false;
    }
    let width = pl.bounds.bbox_width();
    if (h.left_edge() > 0.55 * width && score <= dg + 5.0)
        || h.left_edge() > 0.75 * width
        || (h.left_edge() > 0.4 * width
            && h.center_x() > 0.6 * width
            && pl.stats.total_line_weight > pymath::min(1000.0, doc.stats.median_page_weight))
    {
        return false;
    }
    let col = h.lines.first().map_or(-1, |&l| pl.lines[l].column);
    let cb = (col >= 0 && (col as usize) < pl.columns.len()).then(|| pl.columns[col as usize]);
    if h.bbox_width() < 0.2 * width
        && let Some(cb) = cb
        && cb.bbox_width() < 0.2 * width
        && cb.bbox_height() > 1.5 * cb.bbox_width()
    {
        return false;
    }
    let oi = h.orig_index.get();
    let neighbor = ps.neighbors.right(oi).map(|id| ps.b(id));
    let gap = neighbor.map_or(f64::INFINITY, |n| h.bottom_edge() - n.top_edge());
    let line_gap = pl.stats.median_overlap_gap - pl.stats.median_font_size;
    if gap < 0.9 * line_gap {
        return false;
    }
    let above = ps.neighbors.above(oi).map(|id| ps.b(id));
    let above_gap = above.map_or(f64::INFINITY, |a| a.bottom_edge() - h.top_edge());
    if above_gap < 0.9 * line_gap {
        return false;
    }
    let prev = ps.prev_page.map(|p| &doc.pages[p]);
    if let Some(pp) = prev
        && !pp.has_caption.get()
        && (pp.layout.stats.total_line_weight
            < clamp_value(doc.stats.median_page_weight, 200.0, 500.0)
            || !pp.has_body.get())
        && score > dg + 0.5
    {
        return true;
    }
    let drn = doc.stats.median_center_y;
    let ppl = prev.map_or(f64::NAN, |p| p.layout.stats.median_center_y);
    let pph = prev.map_or(f64::NAN, |p| p.layout.bounds.bbox_height());
    let centered = h.isolated_centered.get();
    if (ppl <= drn && (score <= dg + 1.5 || (score <= dg + 5.0 && !centered)))
        || h.density_chars < 0.5 * doc.stats.p80_density
    {
        return false;
    }
    let bn = ps.neighbors.closest_body_above(oi).map(|id| ps.b(id));
    if let Some(bn) = bn
        && h.bottom_edge() - bn.top_edge() > 2.0 * h.bbox_height()
        && bn.caption_label.get() != 0
    {
        return false;
    }
    let prev_block = ps.output_at(oi as i64 - 1).map(|id| ps.b(id));
    let next_block = ps.output_at(oi as i64 + 1).map(|id| ps.b(id));
    if has_substantive_content(h, prev_block, next_block, page) {
        return false;
    }
    (ppl > drn + 0.1 * pph
        && (score > dg + 2.0
            || (ppl > drn + 0.2 * pph
                && bn.is_some()
                && neighbor.is_some_and(|n| gap > n.weighted_font_size))))
        || (centered && neighbor.is_none_or(|n| n.caption_label.get() == 0))
        || score > 1.5 * dg
        || (c.numbering.len() == 1 && c.numbering[0] == 1.0 && above.is_none())
}

// ref: heading_detection/detectors.py::try_classify_heading
fn try_classify_heading(ps: &PageScanState, bid: BlockId) -> Option<HeadingCandidate> {
    let c = classify_heading(ps, bid);
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

/// First valid heading at the top of each page after the title page, clique-filtered; accepted
/// openers become type 7 / used-as-heading, nearby duplicates type 12.
// ref: heading_detection/page_scan.py::find_section_openers
pub fn find_section_openers(doc: &Document, start_page_idx: usize) -> Vec<OutlineNode> {
    let mut items: Vec<HeadingCandidate> = Vec::new();
    for pi0 in start_page_idx..doc.pages.len() {
        let page = &doc.pages[pi0];
        let ps = PageScanState::new(doc, pi0);
        let mut cand = None;
        if !page.title_or_refs.get() {
            for &bid in &page.output {
                let b = &page.blocks[bid];
                if b.char_count() == 0 || b.weighted_skew > 1.0 || b.kind.get() != 0 {
                    continue;
                }
                if b.is_body_paragraph.get()
                    || b.top_edge() < 0.5 * page.layout.bounds.bbox_height()
                {
                    break;
                }
                if b.caption_label.get() != 0 {
                    break;
                }
                if let Some(c) = try_classify_heading(&ps, bid) {
                    cand = Some(c);
                    break;
                }
                if b.line_count() > 2 {
                    break;
                }
            }
        }
        if let Some(c) = cand {
            items.push(c);
        }
    }
    if items.len() <= 1 {
        return Vec::new();
    }
    let all: Vec<usize> = (0..items.len()).collect();
    let bundle = OutlineContext::new(doc, &items, &all);
    let mut accepted = OutlineContext::default();
    let mut out = Vec::new();
    for i in 0..items.len() {
        let c = &items[i];
        let b = c.block(doc);
        if accepted.has_nearby_duplicate(doc, &items, c) {
            b.kind.set(12);
            continue;
        }
        if bundle.has_conflict(doc, &items, c) {
            continue;
        }
        accepted.add(doc, &items, i);
        out.push(OutlineNode {
            heading: c.clone(),
            children: Vec::new(),
        });
        b.kind.set(7);
        b.used_as_heading.set(true);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dead_digit() {
        assert!(dead_digit_match("xp{Number}y"));
        assert!(dead_digit_match("xp{Number}}}y\n"));
        assert!(dead_digit_match("xp{Number}}"));
        assert!(!dead_digit_match("xp{Number}"));
        assert!(!dead_digit_match("12"));
    }

    #[test]
    fn ordinals() {
        assert_eq!(letter_to_ordinal("C"), Some(3.0));
        assert_eq!(letter_to_ordinal("i"), None);
        assert_eq!(letter_to_ordinal("ab"), None);
    }
}
