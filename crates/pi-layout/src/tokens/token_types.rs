//! Token types, anchors, and token-view utilities.
//!
//! ref: pageindex/flash/tokens/token_types.py

use std::rc::Rc;

use crate::clustering::LineId;
use crate::model::numbering::to_number;
use crate::model::span_line::SpanId;
use crate::stats::scripts::char_script_bucket;

/// ref: tokens/token_types.py:10 `SCRIPT_FAMILY_MAP`
pub const SCRIPT_FAMILY_MAP: [u8; 13] = [0, 1, 2, 2, 2, 2, 3, 4, 5, 6, 7, 7, 8];

/// ref: tokens/token_types.py::_build_gap_tolerance_grid
pub fn gap_tolerance(prev_last_cat: u8, next_first_cat: u8) -> f64 {
    match (prev_last_cat, next_first_cat) {
        (1..=4, 5 | 6) => 0.16,
        (3, 2) | (6, 2) | (6, 3) | (8, 2) | (8, 3) => 0.1,
        _ => 0.0,
    }
}

/// Word-y categories: letter / digit / mark. ref: model/char_stats.py::is_word_category
pub fn is_word_category(c: u8) -> bool {
    matches!(c, 1 | 2 | 3 | 5)
}

/// ref: model/char_stats.py::is_punct_category
pub fn is_punct_category(c: u8) -> bool {
    matches!(c, 6..=8)
}

// ref: tokens/token_types.py::can_extend_token
pub fn can_extend_token(last_cat: u8, cat: u8, ch: char) -> bool {
    if cat == 4 && char_script_bucket(ch) == 5 {
        return false;
    }
    if cat == last_cat && !is_punct_category(cat) {
        return true;
    }
    is_word_category(last_cat) && is_word_category(cat)
}

/// ref: tokens/token_types.py::TokenAnchor
#[derive(Debug, Clone, PartialEq)]
pub struct TokenAnchor {
    pub line: Option<LineId>,
    pub span: Option<SpanId>,
    pub start_offset: i64,
    /// ref slot: `primary_slot`
    pub end_offset: i64,
}

/// ref: tokens/token_types.py::Token
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    /// 1 digits, 2 letters, 3 punct-ish, 4 symbol-ish, ... (script family of the first char).
    pub kind: u8,
    pub text: String,
    /// `len(token.str)` in code points.
    pub len: usize,
    pub anchors: Vec<TokenAnchor>,
    /// Followed by whitespace. ref slot: `boundary_slot`
    pub boundary: bool,
    /// ref slot: `primary_slot`
    pub first_cat: u8,
    /// ref slot: `secondary_slot`
    pub last_cat: u8,
}

impl Token {
    /// Line of the first anchor. ref: tokens/token_types.py::Token.line
    pub fn line(&self) -> Option<LineId> {
        self.anchors.first().and_then(|a| a.line)
    }

    // ref: tokens/token_types.py::first_anchor_span
    pub fn first_anchor_span(&self) -> Option<SpanId> {
        self.anchors.first().and_then(|a| a.span)
    }

    // ref: tokens/token_types.py::last_token_anchor
    pub fn last_anchor(&self) -> Option<&TokenAnchor> {
        self.anchors.last()
    }
}

// ref: tokens/token_types.py::is_char_token
pub fn is_char_token(t: &Token) -> bool {
    t.kind == 1 || t.kind == 2
}

// ref: tokens/token_types.py::is_word_token
pub fn is_word_token(t: &Token) -> bool {
    matches!(t.kind, 3..=5)
}

// ref: tokens/token_types.py::is_trimmable_token
pub fn is_trimmable_token(t: &Token) -> bool {
    t.kind == 3 || t.kind == 4 || t.text == ":"
}

// ref: tokens/token_types.py::token_numeric_value
pub fn token_numeric_value(t: &Token) -> f64 {
    to_number(&t.text)
}

/// Sliceable, directional view over a shared token array.
/// ref: tokens/token_types.py::TokenView
#[derive(Debug, Clone)]
pub struct TokenView {
    pub tokens: Rc<Vec<Token>>,
    pub start: i64,
    pub end: i64,
    pub dir: i64,
    pub length: i64,
}

impl TokenView {
    pub fn new(tokens: Rc<Vec<Token>>, start: i64, end: i64, dir: i64) -> Self {
        let length = if dir != 0 {
            (end - start).div_euclid(dir)
        } else {
            0
        };
        TokenView {
            tokens,
            start,
            end,
            dir,
            length,
        }
    }

    // ref: tokens/token_types.py::wrap_tokens
    pub fn wrap(tokens: Vec<Token>) -> Self {
        let n = tokens.len() as i64;
        TokenView::new(Rc::new(tokens), 0, n, 1)
    }

    pub fn len(&self) -> usize {
        self.length.max(0) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.length <= 0
    }

    // ref: tokens/token_types.py::TokenView.token_at
    pub fn token_at(&self, i: i64) -> Option<&Token> {
        if i < 0 || i >= self.length {
            return None;
        }
        Some(&self.tokens[(self.start + i * self.dir) as usize])
    }

    pub fn iter(&self) -> impl Iterator<Item = &Token> + '_ {
        (0..self.length.max(0)).map(move |i| &self.tokens[(self.start + i * self.dir) as usize])
    }

    // ref: tokens/token_types.py::TokenView.slice
    pub fn slice(&self, a: i64, b: i64) -> TokenView {
        let d = self.dir;
        let mut s = if a > 0 {
            self.start + a * d
        } else if a < 0 {
            self.end + a * d
        } else {
            self.start
        };
        if s * d < self.start * d {
            s = self.start;
        }
        if s * d > self.end * d {
            s = self.end;
        }
        let mut e = if b > 0 {
            self.start + b * d
        } else if b < 0 {
            self.end + b * d
        } else {
            self.end
        };
        if e * d < s * d {
            e = s;
        }
        if e * d > self.end * d {
            e = self.end;
        }
        TokenView::new(self.tokens.clone(), s, e, d)
    }

    /// `slice(a)` (to the end).
    pub fn from(&self, a: i64) -> TokenView {
        self.slice(a, 0)
    }

    // ref: tokens/token_types.py::TokenView.reverse
    pub fn reverse(&self) -> TokenView {
        TokenView::new(
            self.tokens.clone(),
            self.end - self.dir,
            self.start - self.dir,
            -self.dir,
        )
    }

    // ref: tokens/token_types.py::TokenView.__str__
    pub fn to_string_py(&self) -> String {
        let mut out = String::new();
        for t in self.iter() {
            out.push_str(&t.text);
            if t.boundary {
                out.push(' ');
            }
        }
        out
    }

    // ref: tokens/token_types.py::first_token
    pub fn first(&self) -> Option<&Token> {
        if self.length > 0 {
            Some(&self.tokens[self.start as usize])
        } else {
            None
        }
    }

    // ref: tokens/token_types.py::last_token
    pub fn last(&self) -> Option<&Token> {
        if self.length > 0 {
            Some(&self.tokens[(self.end - self.dir) as usize])
        } else {
            None
        }
    }
}
