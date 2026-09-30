//! Data model: rectangles, character statistics, spans, lines and numbering.
//!
//! ref: pageindex/flash/model/

pub mod block;
pub mod char_stats;
pub mod numbering;
pub mod rects;
pub mod span_line;

pub use char_stats::{CharStats, info_weight, is_upper_dominant, letter_count};
pub use numbering::{numbering_kind, numbering_text, numbering_value, to_number};
pub use rects::{Bounded, EMPTY_RECT, rect_union};
pub use span_line::{Line, Span, SpanId, append_span, last_span, text_of_line};
