//! Block type with text, style, and alignment helpers.
//!
//! ref: pageindex/flash/model/block.py
//!
//! Blocks reference the page's final lines by [`LineId`] (index into `PageLayout::lines`, the
//! `02_lines` index). Classification state that later stages mutate while other blocks are
//! borrowed lives in `Cell`s; the reference's per-block caches are `OnceCell`s reset by
//! [`Block::add_line`], the only mutation that invalidates them.

use std::cell::{Cell, OnceCell};

use indexmap::IndexMap;
use pi_core::Rect;
use pi_pycompat::{pymath, unicode};

use super::char_stats::{
    CharStats, info_weight, is_upper_dominant, letter_count, merge_char_stats, punct_count,
};
use super::rects::{Bounded, EMPTY_RECT, center_aligned, left_aligned, rect_union, right_aligned};
use super::span_line::{SpanId, peek_text_of_line, style_key};
use crate::clustering::LineId;
use crate::phases::PageLayout;
use crate::tokens::{TokenView, tokenize_block};

/// Index of a block in a page's block arena.
pub type BlockId = usize;

/// ref: model/block.py::Block
#[derive(Debug, Clone)]
pub struct Block {
    /// ref slot: `secondary_slot` (Bounded)
    pub bbox: Rect,
    /// ref slot: `primary_slot`
    pub lines: Vec<LineId>,
    pub char_stats: CharStats,
    /// Every line center-aligned with the block so far. ref slot: `alignment_slot`
    pub center_aligned: bool,
    /// ref slot: `weighted_ratio_tertiary`
    pub bold_frac: f64,
    /// ref slot: `previous_slot`
    pub italic_frac: f64,
    pub weighted_skew: f64,
    pub weighted_font_size: f64,
    /// Character-weighted line ink density. ref slot: `weighted_ratio_primary`
    pub density_chars: f64,
    /// Area-weighted line ink density. ref slot: `weighted_ratio_secondary`
    pub density_area: f64,
    /// ref slot: `style_slot`
    pub max_line_height: f64,
    pub style_char_counts: IndexMap<String, u64>,
    /// Keyed by the half-up rounded font size (float keys compared with `==`).
    pub size_char_counts: Vec<(f64, u64)>,
    pub reading_order_index: Cell<usize>,
    pub orig_index: Cell<usize>,
    /// ref: `type`
    pub kind: Cell<u8>,
    pub isolated_centered: Cell<bool>,
    pub is_body_paragraph: Cell<bool>,
    /// ref slot: `measure_slot`
    pub caption_claimed: Cell<bool>,
    pub used_as_heading: Cell<bool>,
    /// ref slot: `state_slot`
    pub region_label: Cell<u8>,
    /// ref slot: `marker_slot`
    pub caption_label: Cell<u8>,
    /// Alignment code cache, 0 = uncomputed. ref slot: `metric_slot`
    pub alignment_cache: Cell<u8>,
    dominant_style_cache: OnceCell<String>,
    dominant_size_cache: OnceCell<f64>,
    pub(crate) token_text_cache: OnceCell<String>,
    deaccented_cache: OnceCell<String>,
    text_cache: OnceCell<String>,
    pub(crate) tokens_cache: OnceCell<TokenView>,
}

impl Default for Block {
    // ref: model/block.py::Block.__init__
    fn default() -> Self {
        Block {
            bbox: EMPTY_RECT,
            lines: Vec::new(),
            char_stats: CharStats::default(),
            center_aligned: true,
            bold_frac: 0.0,
            italic_frac: 0.0,
            weighted_skew: 0.0,
            weighted_font_size: 0.0,
            density_chars: 0.0,
            density_area: 0.0,
            max_line_height: 0.0,
            style_char_counts: IndexMap::new(),
            size_char_counts: Vec::new(),
            reading_order_index: Cell::new(0),
            orig_index: Cell::new(0),
            kind: Cell::new(0),
            isolated_centered: Cell::new(false),
            is_body_paragraph: Cell::new(false),
            caption_claimed: Cell::new(false),
            used_as_heading: Cell::new(false),
            region_label: Cell::new(0),
            caption_label: Cell::new(0),
            alignment_cache: Cell::new(0),
            dominant_style_cache: OnceCell::new(),
            dominant_size_cache: OnceCell::new(),
            token_text_cache: OnceCell::new(),
            deaccented_cache: OnceCell::new(),
            text_cache: OnceCell::new(),
            tokens_cache: OnceCell::new(),
        }
    }
}

impl Bounded for Block {
    fn rect(&self) -> &Rect {
        &self.bbox
    }
}

impl Block {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn char_count(&self) -> u32 {
        self.char_stats.total_chars
    }

    /// First line id (`Block.line()`).
    pub fn first_line(&self) -> LineId {
        self.lines[0]
    }

    /// ref: model/block.py::last_line_of
    pub fn last_line(&self) -> LineId {
        *self.lines.last().expect("block has lines")
    }

    // ref: model/block.py::Block.add_line
    pub fn add_line(&mut self, lid: LineId, page: &PageLayout) -> &mut Self {
        let line = &page.lines[lid];
        self.center_aligned =
            self.center_aligned && (self.lines.is_empty() || center_aligned(self, line, 1.0));
        self.lines.push(lid);
        let old = info_weight(&self.char_stats);
        let added = info_weight(&line.char_stats);
        let total = old + added;
        if total > 0.0 {
            self.bold_frac = (self.bold_frac * old + line.bold_frac * added) / total;
            self.italic_frac = (self.italic_frac * old + line.italic_frac * added) / total;
            self.weighted_skew = (self.weighted_skew * old + line.skew_frac * added) / total;
            self.weighted_font_size =
                (self.weighted_font_size * old + line.avg_font_size * added) / total;
            self.density_chars = (self.density_chars * old + line.ink_density * added) / total;
        }
        merge_char_stats(&mut self.char_stats, &line.char_stats);
        if line.char_count() == 0 {
            return self;
        }
        let area_before = self.area();
        self.max_line_height = pymath::max(self.max_line_height, line.max_span_height);
        self.bbox = rect_union(&self.bbox, &line.bbox);
        let area_after = self.area();
        if area_after > 0.0 {
            self.density_area =
                (self.density_area * area_before + line.ink_density * line.area()) / area_after;
        }
        for &sid in &line.spans {
            let span = &page.spans[sid];
            *self.style_char_counts.entry(style_key(span)).or_insert(0) += span.char_count() as u64;
            // floor(x * 10 + 0.5) / 10: half-up on the scaled size.
            let size_key = (span.font_size * 10.0 + 0.5).floor() / 10.0;
            match self
                .size_char_counts
                .iter_mut()
                .find(|(k, _)| *k == size_key)
            {
                Some(e) => e.1 += span.char_count() as u64,
                None => self
                    .size_char_counts
                    .push((size_key, span.char_count() as u64)),
            }
        }
        self.dominant_style_cache = OnceCell::new();
        self.dominant_size_cache = OnceCell::new();
        self.token_text_cache = OnceCell::new();
        self.deaccented_cache = OnceCell::new();
        self.text_cache = OnceCell::new();
        self.tokens_cache = OnceCell::new();
        self.alignment_cache.set(0);
        self
    }
}

/// ref: model/block.py::first_span_of
pub fn first_span_of(block: &Block, page: &PageLayout) -> SpanId {
    page.lines[block.first_line()].spans[0]
}

/// Last span of the block's last line (`last_span(last_line_of(block))`).
pub fn last_span_of(block: &Block, page: &PageLayout) -> SpanId {
    *page.lines[block.last_line()]
        .spans
        .last()
        .expect("line has spans")
}

/// ref: model/block.py::argmax_key — first key with the strictly largest value.
fn argmax_key<K: Clone>(items: impl IntoIterator<Item = (K, u64)>) -> Option<K> {
    let mut best = None;
    let mut best_v = f64::NEG_INFINITY;
    for (k, v) in items {
        if (v as f64) <= best_v {
            continue;
        }
        best = Some(k);
        best_v = v as f64;
    }
    best
}

// ref: model/block.py::dominant_style_of
pub fn dominant_style_of(block: &Block) -> &str {
    block.dominant_style_cache.get_or_init(|| {
        argmax_key(block.style_char_counts.iter().map(|(k, v)| (k.clone(), *v))).unwrap_or_default()
    })
}

// ref: model/block.py::dominant_font_size
pub fn dominant_font_size(block: &Block) -> f64 {
    *block
        .dominant_size_cache
        .get_or_init(|| argmax_key(block.size_char_counts.iter().copied()).unwrap_or(0.0))
}

// ref: model/block.py::is_caps_heavy (for anything with char stats)
pub fn is_caps_heavy(cs: &CharStats) -> bool {
    is_upper_dominant(cs)
        || (cs.category_counts[2] as f64) >= pymath::max(2.0, cs.total_chars as f64)
}

// ref: model/block.py::heading_score
pub fn heading_score(block: &Block) -> f64 {
    dominant_font_size(block)
        + if is_caps_heavy(&block.char_stats) {
            2.0
        } else {
            0.0
        }
        + if block.bold_frac > 0.5 { 1.0 } else { 0.0 }
}

// ref: model/block.py::case_signal
pub fn case_signal(cs: &CharStats) -> i32 {
    let is_punct = matches!(cs.first_cat, 6..=8);
    if is_upper_dominant(cs)
        && !is_punct
        && letter_count(cs) as f64 > 3.0 * cs.total_chars as f64 / 4.0
        && punct_count(cs) < 5
    {
        return 1;
    }
    if cs.category_counts[3] > 0 {
        return -1;
    }
    0
}

// ref: model/block.py::alignment_code
pub fn alignment_code(block: &Block, page: &PageLayout) -> u8 {
    let cached = block.alignment_cache.get();
    if cached != 0 || block.lines.is_empty() {
        return cached;
    }
    let (mut left, mut right, mut any) = (true, true, true);
    for &lid in &block.lines {
        let l = &page.lines[lid];
        let tol = pymath::max(1.0, l.bbox_width() / 20.0);
        let la = left_aligned(block, l, tol);
        let ra = right_aligned(block, l, tol);
        if !la {
            left = false;
        }
        if !ra {
            right = false;
        }
        if !(la || ra) {
            any = false;
        }
    }
    let code = if left && !right {
        2
    } else if right && !left {
        4
    } else if any {
        1
    } else if block.center_aligned {
        3
    } else {
        5
    };
    block.alignment_cache.set(code);
    code
}

// ref: model/block.py::block_text
pub fn block_text<'a>(block: &'a Block, page: &PageLayout) -> &'a str {
    block.text_cache.get_or_init(|| {
        let mut out = String::new();
        for (i, &lid) in block.lines.iter().enumerate() {
            out.push_str(&peek_text_of_line(&page.lines[lid], &page.spans));
            if i + 1 < block.lines.len() {
                out.push(' ');
            }
        }
        out
    })
}

// ref: model/block.py::deaccented_text
pub fn deaccented_text<'a>(block: &'a Block, page: &PageLayout) -> &'a str {
    block
        .deaccented_cache
        .get_or_init(|| strip_diacritics(block_text(block, page)))
}

/// NFD, drop U+0300..U+036F, NFC.
// ref: model/block.py::_strip_diacritics
pub fn strip_diacritics(text: &str) -> String {
    if text.is_ascii() {
        return text.to_string();
    }
    let nfd = unicode::normalize(unicode::Form::Nfd, text);
    let stripped: String = nfd
        .chars()
        .filter(|c| !('\u{0300}'..='\u{036F}').contains(c))
        .collect();
    unicode::normalize(unicode::Form::Nfc, &stripped)
}

/// `tokenize_block(block)` shorthand re-export point.
pub fn tokens_of(block: &Block, page: &PageLayout) -> TokenView {
    tokenize_block(block, page)
}
