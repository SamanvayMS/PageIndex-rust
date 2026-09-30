//! Joins page glyphs into text items with spacing and style thresholds.
//!
//! ref: pageindex/flash/parser_pdfium_charlevel/merge.py::_merge_text_items

use crate::char_extract::off_page;
use crate::model::{FinChar, Item, TextObj};
use crate::text_normalize::{
    NEGATIVE_SPACE_FACTOR, NON_SPACE_GAP_FACTOR, SPACE_IN_FLOW_MAX_FACTOR,
    SPACE_IN_FLOW_MIN_FACTOR, TRACKING_SPACE_FACTOR, rtl_sign,
};

/// ref: text_normalize.py::_read_end
fn read_end(c: &FinChar, sign: f64) -> f64 {
    if sign > 0.0 {
        (c.ox + c.glyph_w).max(c.right)
    } else {
        c.ox
    }
}

/// ref: text_normalize.py::_read_gap
fn read_gap(prev_far: f64, c: &FinChar, sign: f64) -> f64 {
    if sign > 0.0 {
        c.ox - prev_far
    } else {
        prev_far - (c.ox + c.glyph_w)
    }
}

struct Chunk {
    item: Item,
    sign: f64,
    flush_id: usize,
    prev_text_x: Option<f64>,
    prev_oy: Option<f64>,
    tracking: f64,
    not_a_space: f64,
    negative: f64,
    flow_min: f64,
    flow_max: f64,
    height: f64,
    ws_pending: bool,
}

struct Merger<'a> {
    objects: &'a [TextObj],
    items: Vec<Item>,
    chunk: Option<Chunk>,
    two_last: [String; 2],
    two_last_pos: usize,
    last_ref: (Option<f64>, Option<f64>),
}

impl<'a> Merger<'a> {
    fn reset_last_chars(&mut self) {
        self.two_last = [" ".into(), " ".into()];
        self.two_last_pos = 0;
    }

    fn save_last_char(&mut self, ch: &str) -> bool {
        let next = (self.two_last_pos + 1) % 2;
        let ret = self.two_last[self.two_last_pos] != " " && self.two_last[next] == " ";
        self.two_last[self.two_last_pos] = ch.to_string();
        self.two_last_pos = next;
        ret
    }

    fn flush(&mut self) {
        if let Some(c) = self.chunk.take() {
            if !c.item.pieces.is_empty() {
                self.items.push(c.item);
            }
        }
    }

    fn chunk(&mut self) -> &mut Chunk {
        self.chunk.as_mut().expect("open chunk")
    }

    fn open_chunk(&mut self, m: &FinChar) {
        let sign = rtl_sign(&m.ch);
        let obj = &self.objects[m.obj];
        // `not (hs > 0)`: also catches NaN
        let hs = if obj.tz.abs() > 0.0 {
            obj.tz.abs()
        } else {
            1.0
        };
        let fs_x_tz = m.fs_x / hs;
        let mut c = Chunk {
            item: Item {
                pieces: Vec::new(),
                obj: m.obj,
                mtx0: Some(obj.mtx),
                left: m.left,
                right: m.right,
                top: m.top,
                bottom: m.bottom,
                fs: m.fs,
                glyph_w: m.glyph_w,
                font_name: m.font_name.clone(),
                font_key: m.font_key,
                weight: m.weight,
                v: m.vpen.map(|v| (v, v.x)),
            },
            sign,
            flush_id: m.obj,
            prev_text_x: Some(read_end(m, sign)),
            prev_oy: Some(m.oy),
            tracking: fs_x_tz * TRACKING_SPACE_FACTOR,
            not_a_space: fs_x_tz * NON_SPACE_GAP_FACTOR,
            negative: fs_x_tz * NEGATIVE_SPACE_FACTOR,
            flow_min: fs_x_tz * SPACE_IN_FLOW_MIN_FACTOR,
            flow_max: fs_x_tz * SPACE_IN_FLOW_MAX_FACTOR,
            height: m.fs,
            ws_pending: false,
        };
        if m.is_mn {
            (c.prev_text_x, c.prev_oy) = self.last_ref;
        } else {
            self.last_ref = (c.prev_text_x, c.prev_oy);
        }
        self.chunk = Some(c);
    }

    fn emit_fake_space(&mut self, gap: f64) {
        self.reset_last_chars();
        let c = self.chunk.as_ref().expect("open chunk");
        let px = c.prev_text_x.unwrap_or(0.0);
        let baseline = c.prev_oy.unwrap_or(0.0);
        let w = gap.abs();
        let (l, r) = if c.sign >= 0.0 {
            (px, px + w)
        } else {
            (px - w, px)
        };
        let sp = Item {
            pieces: vec![" ".into()],
            obj: c.item.obj,
            mtx0: None,
            left: l,
            right: r,
            top: baseline,
            bottom: baseline,
            fs: c.item.fs,
            glyph_w: 0.0,
            font_name: c.item.font_name.clone(),
            font_key: c.item.font_key,
            weight: c.item.weight,
            v: None,
        };
        self.flush();
        self.items.push(sp);
    }

    fn extend_chunk(&mut self, m: &FinChar, leading_space: bool) {
        let c = self.chunk.as_mut().expect("open chunk");
        if leading_space {
            c.item.pieces.push(" ".into());
        }
        c.item.pieces.push(m.ch.clone());
        c.item.left = c.item.left.min(m.left);
        c.item.right = c.item.right.max(m.right);
        c.prev_text_x = Some(read_end(m, c.sign));
        c.prev_oy = Some(m.oy);
        self.last_ref = (c.prev_text_x, c.prev_oy);
        c.item.glyph_w = m.glyph_w;
        c.item.obj = m.obj;
        c.flush_id = m.obj;
        if let (Some(vp), Some((cv, last_x))) = (m.vpen, c.item.v.as_mut()) {
            cv.after = vp.after;
            *last_x = vp.x;
        }
    }

    fn style_break(c: &Chunk, m: &FinChar) -> bool {
        c.item.font_key != m.font_key || (m.fs - c.item.fs).abs() > 1e-6
    }

    fn step(&mut self, m: &FinChar, view_box: Option<&[f64; 4]>) {
        if m.is_cf {
            if let Some(c) = self.chunk.as_mut() {
                if let Some(px) = c.prev_text_x.as_mut() {
                    *px += c.sign * m.glyph_w;
                    self.last_ref = (c.prev_text_x, c.prev_oy);
                }
            }
            return;
        }
        if m.is_ws {
            self.save_last_char(" ");
            if let Some(c) = self.chunk.as_mut() {
                c.ws_pending = true;
            }
            return;
        }
        if m.is_mn {
            if let Some(c) = self.chunk.as_ref() {
                if Self::style_break(c, m) || (m.obj != c.flush_id && !c.ws_pending) {
                    self.flush();
                }
            }
            if self.chunk.is_none() {
                self.open_chunk(m);
                let lead = self.save_last_char(&m.ch);
                let c = self.chunk();
                if lead {
                    c.item.pieces.push(" ".into());
                }
                c.item.pieces.push(m.ch.clone());
            } else {
                let lead = self.save_last_char(&m.ch);
                let c = self.chunk();
                if lead {
                    c.item.pieces.push(" ".into());
                }
                c.item.pieces.push(m.ch.clone());
            }
            return;
        }
        if off_page(m.ox, m.oy, view_box) {
            return;
        }
        if self.chunk.is_none() {
            self.open_chunk(m);
            self.save_last_char(&m.ch);
            self.chunk().item.pieces.push(m.ch.clone());
            return;
        }
        let ws_bridge = {
            let c = self.chunk();
            let w = c.ws_pending;
            c.ws_pending = false;
            w
        };
        let (prev_text_x, style_break) = {
            let c = self.chunk.as_ref().expect("open chunk");
            (c.prev_text_x, Self::style_break(c, m))
        };
        let Some(prev_text_x) = prev_text_x else {
            if style_break {
                self.flush();
                self.open_chunk(m);
                let lead = self.save_last_char(&m.ch);
                let c = self.chunk();
                if lead {
                    c.item.pieces.push(" ".into());
                }
                c.item.pieces.push(m.ch.clone());
            } else {
                let lead = self.save_last_char(&m.ch);
                self.extend_chunk(m, lead);
            }
            return;
        };
        let (
            sign,
            flush_id,
            prev_oy,
            height,
            tracking,
            not_a_space,
            negative,
            flow_min,
            flow_max,
            fs,
            cobj,
        ) = {
            let c = self.chunk.as_ref().expect("open chunk");
            (
                c.sign,
                c.flush_id,
                c.prev_oy.unwrap_or(0.0),
                c.height,
                c.tracking,
                c.not_a_space,
                c.negative,
                c.flow_min,
                c.flow_max,
                c.item.fs,
                c.item.obj,
            )
        };
        if style_break || (m.obj != flush_id && !ws_bridge) {
            let boundary_gap = read_gap(prev_text_x, m, sign);
            let same_line = (m.oy - prev_oy).abs() <= height;
            let keep_lead;
            if same_line && boundary_gap > tracking {
                self.emit_fake_space(boundary_gap);
                keep_lead = false;
            } else {
                keep_lead = same_line && not_a_space < boundary_gap && boundary_gap <= tracking;
                self.flush();
            }
            self.open_chunk(m);
            let lead = self.save_last_char(&m.ch);
            let c = self.chunk();
            if lead && keep_lead {
                c.item.pieces.push(" ".into());
            }
            c.item.pieces.push(m.ch.clone());
            return;
        }
        let advance = read_gap(prev_text_x, m, sign);
        let dy = m.oy - prev_oy;
        if m.obj == cobj && dy.abs() < 0.1 * height && -0.9 * fs <= advance && advance < -0.2 * fs {
            let lead = self.save_last_char(&m.ch);
            let c = self.chunk();
            if lead {
                c.item.pieces.push(" ".into());
            }
            c.item.pieces.push(m.ch.clone());
            c.item.left = c.item.left.min(m.left);
            c.item.right = c.item.right.max(m.right);
            c.prev_oy = Some(m.oy);
            let r = (c.prev_text_x, c.prev_oy);
            self.last_ref = r;
            return;
        }
        if advance < negative || dy.abs() > height {
            self.reset_last_chars();
            self.flush();
            self.open_chunk(m);
            self.save_last_char(&m.ch);
            self.chunk().item.pieces.push(m.ch.clone());
            return;
        }
        if advance <= not_a_space {
            self.reset_last_chars();
        }
        if advance <= tracking {
            let lead = self.save_last_char(&m.ch);
            self.extend_chunk(m, lead);
            return;
        }
        if flow_min <= advance && advance <= flow_max {
            self.reset_last_chars();
            self.chunk().item.pieces.push(" ".into());
            let lead = self.save_last_char(&m.ch);
            self.extend_chunk(m, lead);
            return;
        }
        self.reset_last_chars();
        self.emit_fake_space(advance);
        self.open_chunk(m);
        self.save_last_char(&m.ch);
        self.chunk().item.pieces.push(m.ch.clone());
    }
}

/// ref: merge.py::_merge_text_items
pub fn merge_text_items(
    chars: &[FinChar],
    objects: &[TextObj],
    view_box: Option<&[f64; 4]>,
) -> Vec<Item> {
    let mut m = Merger {
        objects,
        items: Vec::new(),
        chunk: None,
        two_last: [" ".into(), " ".into()],
        two_last_pos: 0,
        last_ref: (None, None),
    };
    for c in chars {
        m.step(c, view_box);
    }
    m.flush();
    m.items
}
