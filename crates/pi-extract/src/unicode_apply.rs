//! Applies per-font Unicode maps to page chars and synthesizes dropped glyphs.
//!
//! ref: parser_pdfium_charlevel/unicode_apply.py

use std::collections::{BTreeSet, HashMap};

use pi_pycompat::difflib::{SequenceMatcher, Tag};

use crate::code_walk::{ShowCode, WalkResult, char_category, walk_codes};
use crate::font_unicode::{FontMap, font_unicode_map};
use crate::model::{RawChar, TextObj};
use crate::pdfobj::PdfDoc;
use crate::text_normalize::is_whitespace;

pub type FontMapCache = HashMap<i64, Option<FontMap>>;

/// `_SURROGATES.sub("�", map.get(code) or chr(code))`
fn target_str(map: &FontMap, code: u64) -> String {
    let v: Vec<u32> = match map.1.get(&code) {
        Some(v) if !v.is_empty() => v.clone(),
        _ => vec![code as u32],
    };
    v.into_iter()
        .map(|c| char::from_u32(c).unwrap_or('\u{FFFD}'))
        .collect()
}

fn targets_for(
    pdf: &PdfDoc,
    cache: &mut FontMapCache,
    font: Option<i64>,
    units: &[u32],
) -> Option<Vec<String>> {
    let x = font?;
    let entry = cache.entry(x).or_insert_with(|| font_unicode_map(pdf, x));
    let map = entry.as_ref()?;
    if map.0 == 1 {
        Some(units.iter().map(|&c| target_str(map, c as u64)).collect())
    } else {
        Some(
            (0..units.len().saturating_sub(1))
                .step_by(2)
                .map(|k| target_str(map, ((units[k] << 8) | units[k + 1]) as u64))
                .collect(),
        )
    }
}

fn key(i: f64) -> u64 {
    i.to_bits()
}

#[derive(Clone)]
struct Site {
    t: String,
    owner: usize,
    prev_i: Option<f64>,
    next_i: Option<f64>,
}

struct Ctx<'a> {
    chars: &'a mut Vec<RawChar>,
    objects: &'a [TextObj],
    by_index: HashMap<u64, usize>,
    by_obj: HashMap<usize, Vec<(f64, String)>>,
    targets: Vec<Option<Vec<String>>>,
    synth: Vec<Site>,
    failed: Vec<Vec<usize>>,
}

impl Ctx<'_> {
    fn apply(&mut self, r: &WalkResult) {
        for (i, t) in &r.patches {
            if let Some(&p) = self.by_index.get(&key(*i)) {
                let c = &mut self.chars[p];
                c.ch = t.clone();
                (c.is_ws, c.is_mn, c.is_cf) = char_category(t);
            }
        }
        for i in &r.drops {
            if let Some(&p) = self.by_index.get(&key(*i)) {
                self.chars[p].drop = true;
            }
        }
    }

    fn reassign(&mut self, i: f64, owner: usize) {
        if let Some(&p) = self.by_index.get(&key(i)) {
            if self.chars[p].obj != owner {
                self.chars[p].obj = owner;
            }
        }
    }

    fn rewalk_window(&mut self, window: &[usize]) -> bool {
        let mut cv: Vec<(f64, String)> = window
            .iter()
            .flat_map(|w| self.by_obj.get(w).cloned().unwrap_or_default())
            .collect();
        cv.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let mut tt: Vec<String> = Vec::new();
        let mut owner: Vec<usize> = Vec::new();
        for &w in window {
            let t = self.targets[w].as_ref().expect("covered object");
            tt.extend(t.iter().cloned());
            owner.extend(std::iter::repeat_n(w, t.len()));
        }
        let commit = |ctx: &mut Self, res: Option<WalkResult>| -> bool {
            let Some(r) = res else { return false };
            ctx.apply(&r);
            for &(ci, ti) in &r.consumed {
                ctx.reassign(ci, owner[ti]);
            }
            for &(ti, pos) in &r.skips {
                ctx.synth.push(Site {
                    t: tt[ti].clone(),
                    owner: owner[ti],
                    prev_i: if pos > 0 { Some(cv[pos - 1].0) } else { None },
                    next_i: cv.get(pos).map(|c| c.0),
                });
            }
            true
        };
        if window.len() >= 2 && commit(self, walk_codes(&cv, &tt, false)) {
            return true;
        }
        if self.displacement_repair(&cv, &tt, &owner) {
            return true;
        }
        if !commit(self, walk_codes(&cv, &tt, true)) {
            self.failed.push(window.to_vec());
            return false;
        }
        true
    }

    fn displacement_repair(
        &mut self,
        cv: &[(f64, String)],
        tt: &[String],
        owner: &[usize],
    ) -> bool {
        if tt.iter().any(|t| t.chars().count() != 1) {
            return false;
        }
        let chs: Vec<char> = cv.iter().flat_map(|c| c.1.chars()).collect();
        let tts: Vec<char> = tt.iter().flat_map(|t| t.chars()).collect();
        let deficit = tts.len() as i64 - chs.len() as i64;
        if chs == tts || !(0..=8).contains(&deficit) {
            return false;
        }
        // Counter(chs) - Counter(tts) must be empty
        let mut cnt: HashMap<char, i64> = HashMap::new();
        for c in &chs {
            *cnt.entry(*c).or_default() += 1;
        }
        for c in &tts {
            *cnt.entry(*c).or_default() -= 1;
        }
        if cnt.values().any(|&v| v > 0) {
            return false;
        }
        let ops = SequenceMatcher::new(&tts, &chs, false).get_opcodes();
        let mut c2t: Vec<(usize, usize)> = Vec::new(); // insertion-ordered dict
        let mut loose_t: Vec<usize> = Vec::new();
        let mut loose_c: Vec<usize> = Vec::new();
        for o in ops {
            if o.tag == Tag::Equal {
                for r in 0..o.i2 - o.i1 {
                    c2t.push((o.j1 + r, o.i1 + r));
                }
            } else {
                loose_t.extend(o.i1..o.i2);
                loose_c.extend(o.j1..o.j2);
            }
        }
        if loose_c.len() > 24 {
            return false;
        }
        let mut used: BTreeSet<usize> = BTreeSet::new();
        for &ci in &loose_c {
            let cpos = self.by_index.get(&key(cv[ci].0)).copied();
            let mut cands: Vec<usize> = loose_t
                .iter()
                .copied()
                .filter(|t| !used.contains(t) && tts[*t] == chs[ci])
                .collect();
            if cands.is_empty() {
                return false;
            }
            if let Some(p) = cpos {
                if cands.len() > 1 {
                    let (ox, oy) = (self.chars[p].ox, self.chars[p].oy);
                    let dist = |t: usize| {
                        let o = &self.objects[owner[t]];
                        let dx = (o.l - ox).max(0.0).max(ox - o.r);
                        let dy = (o.b - oy).max(0.0).max(oy - o.t);
                        dx * dx + dy * dy
                    };
                    cands.sort_by(|a, b| dist(*a).total_cmp(&dist(*b)));
                }
            }
            c2t.push((ci, cands[0]));
            used.insert(cands[0]);
        }
        let leftover: Vec<usize> = loose_t
            .iter()
            .copied()
            .filter(|t| !used.contains(t))
            .collect();
        if !leftover.is_empty() {
            let t2c: HashMap<usize, usize> = c2t.iter().map(|&(c, t)| (t, c)).collect();
            let mut mapped: Vec<usize> = t2c.keys().copied().collect();
            mapped.sort_unstable();
            for ti in leftover {
                let p = mapped.partition_point(|&m| m < ti);
                let prev = (p > 0).then(|| mapped[p - 1]);
                let next = mapped.get(p).copied();
                self.synth.push(Site {
                    t: tts[ti].to_string(),
                    owner: owner[ti],
                    prev_i: prev.map(|t| cv[t2c[&t]].0),
                    next_i: next.map(|t| cv[t2c[&t]].0),
                });
            }
        }
        for &(ci, ti) in &c2t {
            self.reassign(cv[ci].0, owner[ti]);
        }
        true
    }
}

/// ref: unicode_apply.py::_apply_font_unicode
pub fn apply_font_unicode(
    chars: &mut Vec<RawChar>,
    objects: &[TextObj],
    show_codes: &[ShowCode],
    pdf: &PdfDoc,
    cache: &mut FontMapCache,
) {
    if show_codes.is_empty() {
        return;
    }
    let by_index: HashMap<u64, usize> = chars
        .iter()
        .enumerate()
        .map(|(p, c)| (key(c.i), p))
        .collect();
    if objects.len() == show_codes.len() {
        let mut by_obj: HashMap<usize, Vec<(f64, String)>> = HashMap::new();
        for c in chars.iter().filter(|c| !c.is_gen) {
            by_obj.entry(c.obj).or_default().push((c.i, c.ch.clone()));
        }
        let targets: Vec<Option<Vec<String>>> = show_codes
            .iter()
            .map(|(f, u, _)| targets_for(pdf, cache, *f, u))
            .collect();
        let mut ctx = Ctx {
            chars,
            objects,
            by_index,
            by_obj,
            targets,
            synth: Vec::new(),
            failed: Vec::new(),
        };
        let mut desynced = Vec::new();
        for k in 0..objects.len() {
            let Some(t) = ctx.targets[k].clone() else {
                continue;
            };
            let cs = ctx.by_obj.get(&k).cloned().unwrap_or_default();
            match walk_codes(&cs, &t, false) {
                None => desynced.push(k),
                Some(r) => ctx.apply(&r),
            }
        }
        let mut synth_windows: Vec<(Vec<usize>, usize, usize)> = Vec::new();
        let run_window = |ctx: &mut Ctx, w: &[usize], sw: &mut Vec<(Vec<usize>, usize, usize)>| {
            let before = ctx.synth.len();
            if ctx.rewalk_window(w) && ctx.synth.len() > before {
                sw.push((w.to_vec(), before, ctx.synth.len()));
            }
        };
        let mut window: Vec<usize> = Vec::new();
        for &k in &desynced {
            if let Some(&last) = window.last() {
                let gap: Vec<usize> = (last + 1..k).collect();
                if gap.len() <= 2
                    && gap
                        .iter()
                        .all(|g| ctx.targets.get(*g).is_some_and(|t| t.is_some()))
                {
                    window.extend(gap);
                    window.push(k);
                    continue;
                }
                run_window(&mut ctx, &window, &mut synth_windows);
            }
            window = vec![k];
        }
        if !window.is_empty() {
            run_window(&mut ctx, &window, &mut synth_windows);
        }
        let mut cand: Vec<Vec<usize>> = ctx.failed.clone();
        cand.extend(synth_windows.iter().map(|(w, _, _)| w.clone()));
        if cand.len() >= 2 {
            let stash = ctx.synth.clone();
            for (_, s, e) in synth_windows.iter().rev() {
                ctx.synth.drain(*s..*e);
            }
            ctx.failed.clear();
            let mega: Vec<usize> = cand
                .into_iter()
                .flatten()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            if !ctx.rewalk_window(&mega) {
                ctx.synth = stash;
            }
        }
        if !ctx.synth.is_empty() {
            let mut census: HashMap<char, i64> = HashMap::new();
            for t in ctx.targets.iter().flatten() {
                for s in t {
                    for c in s.chars() {
                        *census.entry(c).or_default() += 1;
                    }
                }
            }
            for c in ctx.chars.iter().filter(|c| !c.is_gen && !c.drop) {
                for ch in c.ch.chars() {
                    *census.entry(ch).or_default() -= 1;
                }
            }
            let mut kept = Vec::new();
            for s in &ctx.synth {
                if s.t.chars().all(|c| is_whitespace(c as u32)) {
                    continue;
                }
                if s.t
                    .chars()
                    .all(|c| census.get(&c).copied().unwrap_or(0) > 0)
                {
                    for c in s.t.chars() {
                        *census.entry(c).or_default() -= 1;
                    }
                    kept.push(s.clone());
                }
            }
            if !kept.is_empty() {
                synthesize_dropped_glyphs(&kept, ctx.chars, &ctx.by_index, objects);
            }
        }
        return;
    }
    // Page mode
    let seq: Vec<(f64, String)> = chars
        .iter()
        .filter(|c| !c.is_gen)
        .map(|c| (c.i, c.ch.clone()))
        .collect();
    let mut targets = Vec::new();
    for (f, u, _) in show_codes {
        if u.is_empty() {
            continue;
        }
        match targets_for(pdf, cache, *f, u) {
            Some(t) => targets.extend(t),
            None => return,
        }
    }
    let Some(r) = walk_codes(&seq, &targets, false) else {
        return;
    };
    let mut ctx = Ctx {
        chars,
        objects,
        by_index,
        by_obj: HashMap::new(),
        targets: Vec::new(),
        synth: Vec::new(),
        failed: Vec::new(),
    };
    ctx.apply(&r);
}

/// ref: unicode_apply.py::_synthesize_dropped_glyphs
fn synthesize_dropped_glyphs(
    sites: &[Site],
    chars: &mut Vec<RawChar>,
    by_index: &HashMap<u64, usize>,
    objects: &[TextObj],
) {
    let mut groups: Vec<Vec<&Site>> = Vec::new();
    for s in sites {
        match groups.last_mut() {
            Some(g)
                if g[0].prev_i == s.prev_i && g[0].next_i == s.next_i && g[0].owner == s.owner =>
            {
                g.push(s)
            }
            _ => groups.push(vec![s]),
        }
    }
    for g in groups {
        let owner = g[0].owner;
        let o = &objects[owner];
        let get = |i: Option<f64>| {
            i.and_then(|i| by_index.get(&key(i)))
                .map(|&p| chars[p].clone())
        };
        let prev = get(g[0].prev_i);
        let nxt = get(g[0].next_i);
        let text: Vec<char> = g.iter().flat_map(|s| s.t.chars()).collect();
        let n = text.len();
        if n == 0 {
            continue;
        }
        let (pen, base_y) = match (&prev, &nxt) {
            (Some(p), _) => (p.right, p.oy),
            (None, Some(x)) => (x.ox, x.oy),
            _ => (o.l, o.b),
        };
        let mut total = 0.0;
        if let (Some(_), Some(x)) = (&prev, &nxt) {
            if (x.oy - base_y).abs() < 0.5 && x.ox > pen {
                total = x.ox - pen;
            }
        }
        let adv = total / n as f64;
        let (base, sgn) = match (&prev, &nxt) {
            (Some(p), _) if p.obj == owner => (p.i, 1.0),
            (_, Some(x)) if x.obj == owner => (x.i, -1.0),
            (Some(p), _) => (p.i, 1.0),
            (None, Some(x)) => (x.i, -1.0),
            _ => (-1.0, 1.0),
        };
        for (k, ch) in text.iter().enumerate() {
            let s = ch.to_string();
            let (is_ws, is_mn, is_cf) = char_category(&s);
            let left = pen + adv * k as f64;
            let step = if sgn > 0.0 {
                (k + 1) as f64
            } else {
                (n - k) as f64
            };
            chars.push(RawChar {
                i: base + sgn * step * 1e-3,
                ch: s,
                is_gen: false,
                is_ws,
                is_mn,
                is_cf,
                ox: left,
                oy: base_y,
                left,
                right: left + adv,
                top: base_y + o.fs_eff,
                bottom: base_y,
                box_top: base_y,
                box_bottom: base_y,
                cell_top: base_y,
                cell_bot: base_y,
                w_raw: 0.0,
                w_synth: Some(adv),
                obj: owner,
                font_name: o.font_name.clone(),
                drop: false,
            });
        }
    }
}
