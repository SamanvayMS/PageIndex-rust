//! Line tokenization into word, char, and number tokens.
//!
//! ref: pageindex/flash/tokens/tokenizer.py

use pi_pycompat::unicode;

use super::token_types::{
    SCRIPT_FAMILY_MAP, Token, TokenAnchor, TokenView, can_extend_token, gap_tolerance,
    is_word_category,
};
use crate::clustering::LineId;
use crate::model::block::Block;
use crate::model::char_stats::char_category;
use crate::model::rects::{Bounded, intervals_overlap};
use crate::model::span_line::{SpanId, line_avg_char_width, span_avg_char_width};
use crate::phases::PageLayout;

/// ref: tokens/tokenizer.py::LineTokenizer
struct LineTokenizer<'a> {
    page: &'a PageLayout,
    /// ref slot: `tertiary_slot`
    tokens: Vec<Token>,
    /// Last anchor span. ref slot: `secondary_slot`
    anchor_span: Option<SpanId>,
    /// Current line. ref slot: `cache_slot`
    line: Option<LineId>,
    /// ref slot: `auxiliary_slot`
    start_off: i64,
    /// ref slot: `option_slot`
    end_off: i64,
    /// ref slot: `marker_slot`
    anchors: Vec<TokenAnchor>,
    /// In-flight token text. ref slot: `primary_slot`
    text: String,
    text_len: usize,
    /// Pending whitespace boundary. ref slot: `previous_slot`
    pending_ws: bool,
    /// ref slot: `state_slot`
    last_cat: u8,
    /// ref slot: `style_slot`
    first_cat: u8,
    /// ref slot: `measure_slot`
    kind: u8,
}

impl<'a> LineTokenizer<'a> {
    fn new(page: &'a PageLayout) -> Self {
        LineTokenizer {
            page,
            tokens: Vec::new(),
            anchor_span: None,
            line: None,
            start_off: -1,
            end_off: -1,
            anchors: Vec::new(),
            text: String::new(),
            text_len: 0,
            pending_ws: false,
            last_cat: 0,
            first_cat: 0,
            kind: 0,
        }
    }

    // ref: tokens/tokenizer.py::LineTokenizer._close_anchor_range
    fn close_anchor_range(&mut self) {
        self.anchors.push(TokenAnchor {
            line: self.line,
            span: self.anchor_span,
            start_offset: self.start_off,
            end_offset: self.end_off,
        });
        self.start_off = -1;
        self.end_off = -1;
    }

    // ref: tokens/tokenizer.py::LineTokenizer._close_token
    fn close_token(&mut self, boundary: bool) {
        if self.start_off >= 0 {
            self.close_anchor_range();
        }
        self.tokens.push(Token {
            kind: self.kind,
            text: std::mem::take(&mut self.text),
            len: self.text_len,
            anchors: std::mem::take(&mut self.anchors),
            boundary,
            first_cat: self.first_cat,
            last_cat: self.last_cat,
        });
        self.text_len = 0;
        self.kind = 0;
        self.first_cat = 0;
        self.last_cat = 0;
    }

    // ref: tokens/tokenizer.py::LineTokenizer._accumulate_char
    fn accumulate(&mut self, ch: char, cat: u8) {
        if self.text_len == 1 && self.last_cat == 5 {
            // A lone mark binds to the following character.
            self.text.insert(0, ch);
            self.first_cat = cat;
        } else {
            if self.text.is_empty() {
                self.first_cat = cat;
            }
            self.text.push(ch);
            self.last_cat = cat;
        }
        self.text_len += 1;
        let fam = SCRIPT_FAMILY_MAP[cat as usize];
        if self.kind == 0 {
            self.kind = fam;
        } else if self.kind == 1 && fam != 1 {
            self.kind = 2;
        }
        self.pending_ws = false;
    }

    // ref: tokens/tokenizer.py::LineTokenizer._advance_char
    fn advance(&mut self, ch: char, idx: i64) {
        let cat = char_category(ch);
        if cat == 10 {
            self.pending_ws = true;
            return;
        }
        if self.pending_ws
            && !self.text.is_empty()
            && !(cat == 5 && is_word_category(self.last_cat))
        {
            self.close_token(true);
        }
        // Soft-hyphen rejoin across lines.
        if self.text.is_empty() && self.tokens.len() >= 2 && cat == 3 {
            let n = self.tokens.len();
            let (t, e) = (&self.tokens[n - 2], &self.tokens[n - 1]);
            let e_line = e.last_anchor().and_then(|a| a.line);
            if t.last_cat == 3 && !t.boundary && e.text == "-" && e_line != self.line {
                self.tokens.pop();
                let w = self.tokens.pop().expect("word token");
                self.kind = w.kind;
                self.text = w.text;
                self.text_len = w.len;
                self.anchors = w.anchors;
                self.first_cat = w.first_cat;
                self.last_cat = w.last_cat;
                self.pending_ws = false;
                self.accumulate(ch, cat);
                self.start_off = idx;
                self.end_off = idx;
                return;
            }
        }
        if !self.text.is_empty() {
            if can_extend_token(self.last_cat, cat, ch) {
                self.accumulate(ch, cat);
                if self.start_off < 0 {
                    self.start_off = idx;
                }
                self.end_off = idx;
            } else {
                self.close_token(false);
                self.accumulate(ch, cat);
                self.start_off = idx;
                self.end_off = idx;
            }
        } else {
            self.accumulate(ch, cat);
            self.start_off = idx;
            self.end_off = idx;
        }
    }

    // ref: tokens/tokenizer.py::LineTokenizer.add_line
    fn add_line(&mut self, lid: LineId) {
        let page = self.page;
        let prev_boundary = self.tokens.last().map(|t| t.boundary);
        if !self.text.is_empty() {
            let b = self.text != "-" || prev_boundary.is_none_or(|b| b);
            self.close_token(b);
        }
        self.line = Some(lid);
        let line = &page.lines[lid];
        let mut pending: Option<SpanId> = None;
        for index in 0..line.spans.len() {
            let sid = line.spans[index];
            let span = &page.spans[sid];
            if span.char_count() == 0 {
                continue;
            }
            if pending.is_none() && span.char_count() == 1 && span.char_stats.first_cat == 5 {
                pending = Some(sid);
                continue;
            }
            if index + 1 < line.spans.len()
                && span.char_count() == 1
                && span.char_stats.first_cat == 11
            {
                let next = &page.spans[line.spans[index + 1]];
                if span.left_edge() >= next.left_edge() && span.center_x() < next.right_edge() {
                    continue;
                }
            }
            match self.anchor_span {
                Some(aid) if !self.text.is_empty() => {
                    let a = &page.spans[aid];
                    if span.left_edge() <= a.right_edge() + 0.1 * span_avg_char_width(a)
                        && ((span.bottom_edge() - a.bottom_edge()).abs() < 0.1
                            || (span.center_y() - a.center_y()).abs() < 0.1)
                        && a.bold == span.bold
                    {
                        self.close_anchor_range();
                        self.anchor_span = Some(sid);
                        self.pending_ws = false;
                    } else {
                        let g = gap_tolerance(a.char_stats.last_cat, span.char_stats.first_cat);
                        let g = if g == 0.0 { 0.12 } else { g };
                        let tol = g * line_avg_char_width(line);
                        let close = self.pending_ws
                            || (a.bottom_edge() - span.bottom_edge()).abs() > 1.0
                            || span.left_edge() < a.right_edge() - 1.0
                            || span.left_edge() > a.right_edge() + tol;
                        self.close_token(close);
                        self.anchor_span = Some(sid);
                    }
                }
                _ => self.anchor_span = Some(sid),
            }
            for (ci, ch) in span.text.chars().enumerate() {
                let composed = if ci == 0 {
                    pending.and_then(|pid| {
                        let p = &page.spans[pid];
                        if intervals_overlap(
                            p.left_edge(),
                            p.right_edge(),
                            span.left_edge(),
                            span.right_edge(),
                        ) {
                            let mark = p.trimmed_text.chars().next()?;
                            let s: String = [ch, mark].iter().collect();
                            unicode::normalize(unicode::Form::Nfc, &s).chars().next()
                        } else {
                            None
                        }
                    })
                } else {
                    None
                };
                match composed {
                    Some(c) => self.advance(c, 0),
                    None => self.advance(ch, ci as i64),
                }
                pending = None;
            }
        }
    }

    // ref: tokens/tokenizer.py::LineTokenizer.tokens
    fn finish(mut self) -> TokenView {
        if !self.text.is_empty() {
            self.close_token(true);
        }
        TokenView::wrap(self.tokens)
    }
}

/// Tokens of all lines of a block, cached on the block.
// ref: tokens/tokenizer.py::tokenize_block
pub fn tokenize_block(block: &Block, page: &PageLayout) -> TokenView {
    block
        .tokens_cache
        .get_or_init(|| tokenize_lines(&block.lines, page))
        .clone()
}

/// Tokenize a sequence of lines (a block's lines).
pub fn tokenize_lines(lines: &[LineId], page: &PageLayout) -> TokenView {
    let mut t = LineTokenizer::new(page);
    for &lid in lines {
        t.add_line(lid);
    }
    t.finish()
}

// ref: tokens/tokenizer.py::clamp_value
pub fn clamp_value(v: f64, lo: f64, hi: f64) -> f64 {
    let m = if hi < v { hi } else { v };
    if lo > m { lo } else { m }
}

// ref: tokens/tokenizer.py::is_superscript_adjacent
pub fn is_superscript_adjacent(t: &Token, next: &Token, page: &PageLayout) -> bool {
    let (Some(last), Some(nsid)) = (t.last_anchor(), next.first_anchor_span()) else {
        return false;
    };
    let Some(csid) = last.span else { return false };
    if nsid == csid || last.line != next.line() {
        return false;
    }
    let (c, r) = (&page.spans[csid], &page.spans[nsid]);
    r.bbox_height() < c.bbox_height() && r.bottom_edge() > c.bottom_edge() + 0.1 * c.bbox_height()
}
