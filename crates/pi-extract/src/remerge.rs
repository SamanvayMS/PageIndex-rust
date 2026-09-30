//! Re-merges rotated, oblique and vertical items after the first join pass.
//!
//! ref: pageindex/flash/parser_pdfium_charlevel/remerge.py

use std::collections::HashMap;

use crate::model::{FinChar, Item, TextObj};
use crate::text_normalize::{
    NEGATIVE_SPACE_FACTOR, SPACE_IN_FLOW_MAX_FACTOR, SPACE_IN_FLOW_MIN_FACTOR,
    TRACKING_SPACE_FACTOR,
};

/// Group item indices by object, preserving first-seen order of objects.
fn group_by_obj(
    items: &[Item],
    pred: impl Fn(&Item) -> bool,
) -> (Vec<usize>, HashMap<usize, Vec<usize>>) {
    let mut order = Vec::new();
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for (k, it) in items.iter().enumerate() {
        if pred(it) {
            let g = groups.entry(it.obj).or_default();
            if g.is_empty() {
                order.push(it.obj);
            }
            g.push(k);
        }
    }
    (order, groups)
}

/// ref: remerge.py::_merge_rotated_one
fn merge_rotated_one(items: &[Item], group: &[usize], rot: i32) -> Vec<Item> {
    let read_origin = |it: &Item| match rot {
        90 => it.bottom,
        270 => -it.top,
        180 => -it.right,
        _ => it.left,
    };
    let mut ordered: Vec<&Item> = group.iter().map(|&k| &items[k]).collect();
    ordered.sort_by(|a, b| read_origin(a).total_cmp(&read_origin(b)));
    let mut spans = Vec::new();
    let mut cur: Option<Item> = None;
    let mut pen = 0.0;
    for it in ordered {
        let fs = it.fs;
        let gw = it.glyph_w;
        let origin = read_origin(it);
        match cur.as_mut() {
            None => cur = Some(it.clone()),
            Some(c) => {
                let gap = origin - pen;
                if gap <= fs * SPACE_IN_FLOW_MAX_FACTOR {
                    if gap > fs * TRACKING_SPACE_FACTOR {
                        c.pieces.push(" ".into());
                    }
                    c.pieces.extend(it.pieces.iter().cloned());
                    c.left = c.left.min(it.left);
                    c.right = c.right.max(it.right);
                    c.top = c.top.max(it.top);
                    c.bottom = c.bottom.min(it.bottom);
                } else {
                    spans.push(cur.replace(it.clone()).expect("cur"));
                }
            }
        }
        pen = origin + gw;
    }
    spans.extend(cur);
    spans
}

/// Replace each grouped object's items (at its first occurrence) with `merged_for[obj]`.
fn splice(
    items: Vec<Item>,
    is_member: impl Fn(&Item) -> bool,
    mut merged_for: HashMap<usize, Vec<Item>>,
) -> Vec<Item> {
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        if is_member(&it) {
            if let Some(m) = merged_for.remove(&it.obj) {
                out.extend(m);
            }
        } else {
            out.push(it);
        }
    }
    out
}

/// ref: remerge.py::_remerge_rotated
pub fn remerge_rotated(items: Vec<Item>, objects: &[TextObj]) -> Vec<Item> {
    let is_rot = |it: &Item| matches!(objects[it.obj].rot, 90 | 180 | 270);
    let (order, groups) = group_by_obj(&items, is_rot);
    if order.is_empty() {
        return items;
    }
    let merged_for: HashMap<usize, Vec<Item>> = order
        .iter()
        .map(|&o| (o, merge_rotated_one(&items, &groups[&o], objects[o].rot)))
        .collect();
    splice(items, is_rot, merged_for)
}

struct Oblique {
    pieces: Vec<String>,
    obj: usize,
    fs: f64,
    font_name: String,
    font_key: usize,
    weight: i32,
    ox0: f64,
    oy0: f64,
    u0: f64,
    uend: f64,
    pen: f64,
    vlast: f64,
    lox: f64,
    loy: f64,
    lgw: f64,
}

impl Oblique {
    fn new(g: &FinChar, u: f64, v: f64, gw: f64) -> Self {
        Self {
            pieces: vec![g.ch.clone()],
            obj: g.obj,
            fs: g.fs,
            font_name: g.font_name.clone(),
            font_key: g.font_key,
            weight: g.weight,
            ox0: g.ox,
            oy0: g.oy,
            u0: u,
            uend: u + gw,
            pen: u + gw,
            vlast: v,
            lox: g.ox,
            loy: g.oy,
            lgw: gw,
        }
    }

    fn item(&self, pieces: Vec<String>, l: f64, r: f64, b: f64, t: f64) -> Item {
        Item {
            pieces,
            obj: self.obj,
            mtx0: None,
            left: l,
            right: r,
            top: t,
            bottom: b,
            fs: self.fs,
            glyph_w: 0.0,
            font_name: self.font_name.clone(),
            font_key: self.font_key,
            weight: self.weight,
            v: None,
        }
    }

    /// ref: remerge.py::_close_oblique
    fn close(&self) -> Item {
        let w = self.uend - self.u0;
        self.item(
            self.pieces.clone(),
            self.ox0,
            self.ox0 + w,
            self.oy0,
            self.oy0 + self.fs,
        )
    }

    /// ref: remerge.py::_oblique_space
    fn space(&self, adv: f64, ux: f64, uy: f64, scale: f64) -> Item {
        let px = self.lox + self.lgw * ux;
        let py = self.loy + self.lgw * uy;
        let w = adv.abs() / scale;
        self.item(vec![" ".into()], px, px + w, py, py)
    }
}

/// ref: remerge.py::_merge_oblique_one
fn merge_oblique_one(chs: &[&FinChar], objects: &[TextObj]) -> Vec<Item> {
    let [a, b, c, d] = objects[chs[0].obj].mtx;
    let h = a.hypot(b);
    let scale = if h == 0.0 { 1.0 } else { h };
    let (ux, uy) = (a / scale, b / scale);
    let along = |g: &FinChar| (a * g.ox + b * g.oy) / scale;
    let cross = |g: &FinChar| (c * g.ox + d * g.oy) / scale;
    let mut ordered: Vec<&FinChar> = chs.to_vec();
    ordered.sort_by(|p, q| along(p).total_cmp(&along(q)));
    let mut spans = Vec::new();
    let mut cur: Option<Oblique> = None;
    for g in ordered {
        if g.is_ws {
            continue;
        }
        let fs = g.fs;
        let gw = g.glyph_w;
        let u = along(g);
        let v = cross(g);
        let Some(cu) = cur.as_mut() else {
            cur = Some(Oblique::new(g, u, v, gw));
            continue;
        };
        let bgap = u - cu.pen;
        let shift = v - cu.vlast;
        if bgap < fs * NEGATIVE_SPACE_FACTOR || shift.abs() > fs {
            spans.push(cu.close());
            cur = Some(Oblique::new(g, u, v, gw));
            continue;
        }
        if bgap <= fs * TRACKING_SPACE_FACTOR {
            cu.pieces.push(g.ch.clone());
        } else if bgap <= fs * SPACE_IN_FLOW_MAX_FACTOR {
            cu.pieces.push(" ".into());
            cu.pieces.push(g.ch.clone());
        } else {
            spans.push(cu.close());
            spans.push(cu.space(bgap, ux, uy, scale));
            cur = Some(Oblique::new(g, u, v, gw));
            continue;
        }
        cu.uend = u + gw;
        cu.pen = u + gw;
        cu.vlast = v;
        (cu.lox, cu.loy, cu.lgw) = (g.ox, g.oy, gw);
    }
    if let Some(cu) = cur {
        spans.push(cu.close());
    }
    spans
}

/// ref: remerge.py::_remerge_oblique
pub fn remerge_oblique(items: Vec<Item>, fin: &[FinChar], objects: &[TextObj]) -> Vec<Item> {
    let mut order = Vec::new();
    let mut groups: HashMap<usize, Vec<&FinChar>> = HashMap::new();
    for g in fin {
        if objects[g.obj].rot == -1 {
            let e = groups.entry(g.obj).or_default();
            if e.is_empty() {
                order.push(g.obj);
            }
            e.push(g);
        }
    }
    if order.is_empty() {
        return items;
    }
    let merged_for: HashMap<usize, Vec<Item>> = order
        .iter()
        .map(|&o| (o, merge_oblique_one(&groups[&o], objects)))
        .collect();
    splice(items, |it| objects[it.obj].rot == -1, merged_for)
}

/// ref: remerge.py::_merge_vertical_one
fn merge_vertical_one(items: &[Item], group: &[usize]) -> Vec<Item> {
    // (item, v_pen_x, v_pen_y, v_height)
    let mut spans: Vec<Item> = Vec::new();
    let mut cur: Option<(Item, f64, f64, f64)> = None;
    let close = |(mut it, px, py, vh): (Item, f64, f64, f64)| {
        it.left = px;
        it.right = px + it.fs;
        it.top = py;
        it.bottom = py - vh.abs();
        it
    };
    let start = |it: &Item| {
        let (vp, _) = it.v.expect("vertical item");
        (it.clone(), vp.x, vp.y, vp.y - vp.after)
    };
    let mut after = 0.0;
    let mut last_x = 0.0;
    for &k in group {
        let it = &items[k];
        let (vp, _) = it.v.expect("vertical item");
        let fs = it.fs;
        let Some(c) = cur.as_mut() else {
            cur = Some(start(it));
            (after, last_x) = (vp.after, vp.x);
            continue;
        };
        let vgap = after - vp.y;
        let xshift = vp.x - last_x;
        let dir = if c.3 >= 0.0 { 1.0 } else { -1.0 };
        let width = c.0.fs;
        if vgap < dir * NEGATIVE_SPACE_FACTOR * fs || xshift.abs() > width {
            spans.push(close(cur.replace(start(it)).expect("cur")));
        } else if vgap <= dir * TRACKING_SPACE_FACTOR * fs {
            c.3 += vgap + (vp.y - vp.after);
            c.0.pieces.extend(it.pieces.iter().cloned());
        } else if dir * SPACE_IN_FLOW_MIN_FACTOR * fs <= vgap
            && vgap <= dir * SPACE_IN_FLOW_MAX_FACTOR * fs
        {
            c.0.pieces.push(" ".into());
            c.3 += vgap + (vp.y - vp.after);
            c.0.pieces.extend(it.pieces.iter().cloned());
        } else {
            let meta = cur.replace(start(it)).expect("cur");
            let sp = Item {
                pieces: vec![" ".into()],
                obj: meta.0.obj,
                mtx0: None,
                left: last_x,
                right: last_x,
                top: after,
                bottom: after - vgap.abs(),
                fs: meta.0.fs,
                glyph_w: 0.0,
                font_name: meta.0.font_name.clone(),
                font_key: meta.0.font_key,
                weight: meta.0.weight,
                v: None,
            };
            spans.push(close(meta));
            spans.push(sp);
        }
        (after, last_x) = (vp.after, vp.x);
    }
    if let Some(c) = cur {
        spans.push(close(c));
    }
    spans
}

/// ref: remerge.py::_remerge_vertical
pub fn remerge_vertical(items: Vec<Item>, objects: &[TextObj]) -> Vec<Item> {
    let is_vert = |it: &Item| {
        let o = &objects[it.obj];
        o.vertical && o.rot == 0 && it.v.is_some()
    };
    let (order, groups) = group_by_obj(&items, is_vert);
    if order.is_empty() {
        return items;
    }
    let merged: HashMap<usize, Vec<Item>> = order
        .iter()
        .map(|&o| (o, merge_vertical_one(&items, &groups[&o])))
        .collect();
    let mut paint = order.clone();
    paint.sort_by_key(|&o| objects[o].page_order);
    let mut out = Vec::with_capacity(items.len());
    let mut slot = 0;
    let mut seen = std::collections::HashSet::new();
    for it in items {
        if groups.contains_key(&it.obj) {
            if seen.insert(it.obj) {
                out.extend(merged[&paint[slot]].iter().cloned());
                slot += 1;
            }
        } else {
            out.push(it);
        }
    }
    out
}
