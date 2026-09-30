//! Embedded PDF bookmark (outline dictionary) consumption: a port of
//! `pageindex/flash/embedded_toc.py` (VectifyAI/PageIndex @619cbd8, MIT).
//!
//! Bookmarks are read with PDFium, validated, and classified into tiers:
//! - FULL: the bookmark tree replaces the detected hierarchy, then detected sections it lacks
//!   are grafted back in by page range after noise filtering;
//! - SKELETON: top-level entries pin the chapter frame and detected nodes are re-hung under
//!   them; with page text, deeper entries are verified and filled in and garbled detected
//!   titles are repaired from same-page bookmark strings;
//! - IGNORE: detection stands.
//!
//! Trees are JSON-like (`serde_json::Value`, key order preserved). The reference mutates dicts
//! in place and shares them between lists; here nodes live in an arena so that sharing and
//! in-place edits behave identically, including where a newly added `nodes` key lands.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use pi_pycompat::difflib::ratio_str;
use pi_pycompat::pystr;
use pi_pycompat::unicode::{Cat, category, lower};
use serde_json::{Map, Value};

// --------------------------------------------------------------------------------------------
// constants (embedded_toc.py:50-77)
// --------------------------------------------------------------------------------------------

/// Consumption tiers. ref: embedded_toc.py:50
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Ignore = 1,
    Skeleton = 2,
    Full = 3,
}

/// Same-page titles at or above this similarity refer to the same section. ref: :53
pub const REPAIR_SIMILARITY: f64 = 0.7;
/// A normalized title recurring this often across the detected tree is a running label. ref: :58
pub const BACKFILL_DUP_MIN: usize = 3;
/// A detected title longer than this (code points) is a paragraph lead. ref: :59
pub const BACKFILL_MAX_TITLE: usize = 100;
/// Roman-looking tokens that are words. ref: :69
const ROMAN_EXCLUDED: [&str; 7] = ["di", "div", "li", "liv", "mi", "mix", "xi"];
/// Words that number a content unit ("Chapter N" is a coarse frame). ref: :73
const CONTENT_UNITS: [&str; 21] = [
    "chapter", "chap", "ch", "part", "pt", "section", "sec", "book", "appendix", "unit", "lesson",
    "章", "部", "节", "篇", "卷", "第章", "第部", "第节", "第篇", "第卷",
];
/// pypdfium2 `PdfDocument.get_toc(max_depth=15)`.
pub const TOC_MAX_DEPTH: usize = 15;

// --------------------------------------------------------------------------------------------
// title normalization (embedded_toc.py:61-108)
// --------------------------------------------------------------------------------------------

/// Python `str.isalnum()` for one code point: the `[^\W_]` class of `_TOKEN` (:65).
/// isalpha (L*) or isdecimal/isdigit/isnumeric (N*), per the Unicode 14 tables.
fn is_py_alnum(c: char) -> bool {
    use Cat::*;
    matches!(category(c), Lu | Ll | Lt | Lm | Lo | Nd | Nl | No)
}

/// `_LATEX_SPAN.sub("", s)`: drop `$...$` spans (lazy, `.` excludes `\n`). ref: :64
fn strip_latex(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '$' && chars[j] != '\n' {
                j += 1;
            }
            if j < chars.len() && chars[j] == '$' {
                i = j + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// `_TOKEN.findall(_LATEX_SPAN.sub("", title.lower()))`.
fn tokens(title: &str) -> Vec<String> {
    let text = strip_latex(&lower(title));
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if is_py_alnum(c) {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn roman_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    // ref: :66, used with fullmatch
    RE.get_or_init(|| {
        regex::Regex::new(r"^m{0,3}(cm|cd|d?c{0,3})(xc|xl|l?x{0,3})(ix|iv|v?i{0,3})$").unwrap()
    })
}

/// `_is_roman(token)`. ref: :90
fn is_roman(token: &str) -> bool {
    roman_re().is_match(token) && !ROMAN_EXCLUDED.contains(&token)
}

/// `_roman_to_arabic(token)`. ref: :80
fn roman_to_arabic(token: &str) -> String {
    let value = |c: char| match c {
        'i' => 1,
        'v' => 5,
        'x' => 10,
        'l' => 50,
        'c' => 100,
        'd' => 500,
        'm' => 1000,
        _ => 0,
    };
    let (mut total, mut prev) = (0i64, 0i64);
    for c in token.chars().rev() {
        let v = value(c);
        total = if v < prev { total - v } else { total + v };
        prev = prev.max(v);
    }
    total.to_string()
}

/// `_normalize_title(title)`: lowercased, LaTeX spans dropped, punctuation stripped, roman
/// tokens as digits. ref: :94
pub fn normalize_title(title: &str) -> String {
    tokens(title)
        .into_iter()
        .map(|t| if is_roman(&t) { roman_to_arabic(&t) } else { t })
        .collect()
}

/// `_title_template(title)`: every counter (digit run or roman token) as `#`. ref: :102
pub fn title_template(title: &str) -> String {
    let mut out = String::new();
    for t in tokens(title) {
        if is_roman(&t) {
            out.push('#');
            continue;
        }
        let mut in_run = false;
        for c in t.chars() {
            if c.is_ascii_digit() {
                if !in_run {
                    out.push('#');
                }
                in_run = true;
            } else {
                out.push(c);
                in_run = false;
            }
        }
    }
    out
}

/// Case-insensitive match of one pattern letter under Python's Unicode IGNORECASE: the simple
/// lowercase of `c` equals `l`, plus sre's extra equivalences (ı ~ i, ſ ~ s).
fn ci(c: char, l: char) -> bool {
    c == l
        || c == l.to_ascii_uppercase()
        || (l == 'i' && (c == '\u{130}' || c == '\u{131}'))
        || (l == 's' && c == '\u{17F}')
}

/// `_GENERIC_TITLE.match(title)` for
/// `^(?:(?:page|slide|folie|document\s+page)\s*)?\d+$` with IGNORECASE. `$` also matches
/// before a final `\n`; `\s` and `\d` are Python's (str.isspace, Nd). ref: :61
pub fn is_generic_title(title: &str) -> bool {
    let chars: Vec<char> = title.chars().collect();
    let is_space = |c: char| pystr::is_py_space(c);
    let digits_to_end = |mut i: usize| -> bool {
        let start = i;
        while i < chars.len() && category(chars[i]) == Cat::Nd {
            i += 1;
        }
        i > start && (i == chars.len() || (i + 1 == chars.len() && chars[i] == '\n'))
    };
    let word = |i: usize, w: &str| -> Option<usize> {
        let mut j = i;
        for l in w.chars() {
            if j < chars.len() && ci(chars[j], l) {
                j += 1;
            } else {
                return None;
            }
        }
        Some(j)
    };
    let prefix_ends = || -> Vec<usize> {
        let mut ends = Vec::new();
        for w in ["page", "slide", "folie"] {
            if let Some(j) = word(0, w) {
                ends.push(j);
            }
        }
        if let Some(mut j) = word(0, "document") {
            let s = j;
            while j < chars.len() && is_space(chars[j]) {
                j += 1;
            }
            if j > s
                && let Some(k) = word(j, "page")
            {
                ends.push(k);
            }
        }
        ends
    };
    for mut j in prefix_ends() {
        while j < chars.len() && is_space(chars[j]) {
            j += 1;
        }
        if digits_to_end(j) {
            return true;
        }
    }
    digits_to_end(0)
}

// --------------------------------------------------------------------------------------------
// bookmark entries (embedded_toc.py:111-204)
// --------------------------------------------------------------------------------------------

/// A raw or validated bookmark: `{title, level, page}`, all 1-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub title: String,
    pub level: usize,
    pub page: i64,
}

/// Where the PDF comes from.
pub enum PdfSource<'a> {
    Path(&'a Path),
    Bytes(Vec<u8>),
}

mod raw {
    //! The pypdfium2 5.13 `PdfDocument.get_toc()` walk over raw PDFium bindings.
    #![allow(unsafe_code)]

    use std::collections::HashSet;

    use pi_extract::pdfium::Document;

    use super::{Entry, TOC_MAX_DEPTH};

    /// `PdfBookmark.get_title()`: UTF-16LE, terminator dropped, strict decoding.
    fn title(doc: &Document, bm: *mut std::ffi::c_void) -> Option<String> {
        // SAFETY: `bm` is a live bookmark handle of `doc`; the buffer is sized by PDFium.
        let n = unsafe {
            doc.b
                .FPDFBookmark_GetTitle(bm.cast(), std::ptr::null_mut(), 0)
        } as usize;
        let mut buf = vec![0u8; n];
        if n > 0 {
            unsafe {
                doc.b
                    .FPDFBookmark_GetTitle(bm.cast(), buf.as_mut_ptr().cast(), n as _)
            };
        }
        let bytes = &buf[..n.saturating_sub(2)];
        if !bytes.len().is_multiple_of(2) {
            return None;
        }
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
            .collect();
        String::from_utf16(&units).ok()
    }

    /// Entries as `read_bookmarks` builds them; `None` when a title fails to decode (the
    /// reference catches the `UnicodeDecodeError` and returns `[]`).
    pub fn read(doc: &Document) -> Option<Vec<Entry>> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        walk(doc, std::ptr::null_mut(), 0, &mut seen, &mut out)?;
        Some(out)
    }

    fn walk(
        doc: &Document,
        parent: *mut std::ffi::c_void,
        level: usize,
        seen: &mut HashSet<usize>,
        out: &mut Vec<Entry>,
    ) -> Option<()> {
        // SAFETY: valid document; `parent` is null (root) or a live bookmark handle.
        let mut bm = unsafe { doc.b.FPDFBookmark_GetFirstChild(doc.doc, parent.cast()) };
        while !bm.is_null() {
            if !seen.insert(bm as usize) {
                break; // circular bookmark reference
            }
            // read_bookmarks: dest -> page index, then the title
            let dest = unsafe { doc.b.FPDFBookmark_GetDest(doc.doc, bm) };
            let page_idx = if dest.is_null() {
                None
            } else {
                let v = unsafe { doc.b.FPDFDest_GetDestPageIndex(doc.doc, dest) };
                (v >= 0).then_some(v as i64)
            };
            let title = title(doc, bm.cast())?;
            if let Some(idx) = page_idx {
                let title = pi_pycompat::pystr::strip(&title);
                if !title.is_empty() {
                    out.push(Entry {
                        title: title.to_string(),
                        level: level + 1,
                        page: idx + 1,
                    });
                }
            }
            if level < TOC_MAX_DEPTH - 1 {
                walk(doc, bm.cast(), level + 1, seen, out)?;
            }
            bm = unsafe { doc.b.FPDFBookmark_GetNextSibling(doc.doc, bm) };
        }
        Some(())
    }
}

/// `read_bookmarks(doc_handle)`: raw entries; `[]` when there is no outline, the document does
/// not open, or the outline is unreadable. ref: :111
pub fn read_bookmarks(pdf: PdfSource<'_>) -> Vec<Entry> {
    let bytes = match pdf {
        PdfSource::Path(p) => match std::fs::read(p) {
            Ok(b) => b,
            Err(_) => return Vec::new(),
        },
        PdfSource::Bytes(b) => b,
    };
    // PDFium is not thread-safe: `Document` holds pi_extract's process-wide PDFium lock.
    match pi_extract::pdfium::Document::open(bytes) {
        Ok(doc) => raw::read(&doc).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// `validate_bookmarks(entries, n_pages)`: drop out-of-range and backwards targets, then
/// re-stack levels over the survivors. ref: :150
pub fn validate_bookmarks(entries: &[Entry], n_pages: i64) -> Vec<Entry> {
    let mut kept = Vec::new();
    let mut last_page = 0;
    for e in entries {
        if !(1 <= e.page && e.page <= n_pages) || e.page < last_page {
            continue;
        }
        kept.push(e);
        last_page = e.page;
    }
    let mut out = Vec::with_capacity(kept.len());
    let mut stack: Vec<usize> = Vec::new();
    for e in kept {
        while stack.last().is_some_and(|&l| l >= e.level) {
            stack.pop();
        }
        stack.push(e.level);
        out.push(Entry {
            title: e.title.clone(),
            level: stack.len(),
            page: e.page,
        });
    }
    out
}

/// `classify_bookmarks(entries, n_pages)`. ref: :178
pub fn classify_bookmarks(entries: &[Entry], n_pages: i64) -> Tier {
    let n = entries.len();
    if n < 3 {
        return Tier::Ignore;
    }
    let generic = entries
        .iter()
        .filter(|e| is_generic_title(&e.title))
        .count();
    if 2 * generic >= n {
        return Tier::Ignore;
    }
    // Counter.most_common(1): the highest count, first inserted on ties
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for e in entries {
        let t = title_template(&e.title);
        match index.get(&t) {
            Some(&i) => counts[i].1 += 1,
            None => {
                index.insert(t.clone(), counts.len());
                counts.push((t, 1));
            }
        }
    }
    let (top, top_count) = counts
        .iter()
        .fold(&counts[0], |best, c| if c.1 > best.1 { c } else { best });
    if top.contains('#')
        && 2 * top_count >= n
        && (top.matches('#').count() >= 2
            || !CONTENT_UNITS.contains(&top.replace('#', "").as_str()))
    {
        return Tier::Ignore;
    }
    let distinct: std::collections::HashSet<String> =
        entries.iter().map(|e| normalize_title(&e.title)).collect();
    if 4 * distinct.len() <= n {
        return Tier::Ignore;
    }
    let max_level = entries.iter().map(|e| e.level).max().unwrap_or(0);
    if max_level >= 2 && n as i64 >= 6.max(n_pages.div_euclid(20)) {
        return Tier::Full;
    }
    Tier::Skeleton
}

// --------------------------------------------------------------------------------------------
// node arena
// --------------------------------------------------------------------------------------------

type Id = usize;
const NODES: &str = "nodes";

#[derive(Debug, Clone, Default)]
struct Node {
    /// Keys in dict order; `"nodes"` holds a `Null` placeholder while the key exists.
    fields: Map<String, Value>,
    kids: Vec<Id>,
}

#[derive(Debug, Default)]
struct Arena {
    nodes: Vec<Node>,
}

impl Arena {
    fn add_value(&mut self, v: &Value) -> Id {
        let mut node = Node::default();
        if let Value::Object(m) = v {
            for (k, val) in m {
                if k == NODES {
                    node.fields.insert(k.clone(), Value::Null);
                    if let Value::Array(items) = val {
                        node.kids = items.iter().map(|c| self.add_value(c)).collect();
                    }
                } else {
                    node.fields.insert(k.clone(), val.clone());
                }
            }
        }
        self.push(node)
    }

    fn push(&mut self, n: Node) -> Id {
        self.nodes.push(n);
        self.nodes.len() - 1
    }

    fn value(&self, id: Id) -> Value {
        let n = &self.nodes[id];
        let mut out = Map::new();
        for (k, v) in &n.fields {
            if k == NODES {
                out.insert(
                    k.clone(),
                    Value::Array(n.kids.iter().map(|&c| self.value(c)).collect()),
                );
            } else {
                out.insert(k.clone(), v.clone());
            }
        }
        Value::Object(out)
    }

    /// `{"title", "node_id": "", "start_index", "end_index", "nodes": []}`
    fn new_node(&mut self, title: &str, page: i64) -> Id {
        let mut f = Map::new();
        f.insert("title".into(), Value::String(title.to_string()));
        f.insert("node_id".into(), Value::String(String::new()));
        f.insert("start_index".into(), page.into());
        f.insert("end_index".into(), page.into());
        f.insert(NODES.into(), Value::Null);
        self.push(Node {
            fields: f,
            kids: Vec::new(),
        })
    }

    /// `dict(node, nodes=[])`: a shallow copy with an empty child list.
    fn copy_leaf(&mut self, id: Id) -> Id {
        let mut fields = self.nodes[id].fields.clone();
        fields.insert(NODES.into(), Value::Null);
        self.push(Node {
            fields,
            kids: Vec::new(),
        })
    }

    /// `node.get("nodes") or []`
    fn kids(&self, id: Id) -> Vec<Id> {
        let n = &self.nodes[id];
        if n.fields.contains_key(NODES) {
            n.kids.clone()
        } else {
            Vec::new()
        }
    }

    /// `node["nodes"] = kids`
    fn set_kids(&mut self, id: Id, kids: Vec<Id>) {
        self.nodes[id].fields.insert(NODES.into(), Value::Null);
        self.nodes[id].kids = kids;
    }

    /// `node.setdefault("nodes", [])`
    fn ensure_kids(&mut self, id: Id) {
        if !self.nodes[id].fields.contains_key(NODES) {
            self.set_kids(id, Vec::new());
        }
    }

    fn get_i64(&self, id: Id, key: &str) -> i64 {
        self.nodes[id]
            .fields
            .get(key)
            .and_then(Value::as_i64)
            .unwrap_or(0)
    }

    fn start(&self, id: Id) -> i64 {
        self.get_i64(id, "start_index")
    }

    fn title(&self, id: Id) -> String {
        match self.nodes[id].fields.get("title") {
            Some(Value::String(s)) => s.clone(),
            _ => String::new(),
        }
    }

    fn set(&mut self, id: Id, key: &str, v: Value) {
        self.nodes[id].fields.insert(key.to_string(), v);
    }

    /// Insert before the first sibling starting after `node` (`insert_by_page`, :549 / :469).
    fn insert_by_page(&mut self, parent: Id, node: Id, page: i64) {
        let pos = {
            let kids = &self.nodes[parent].kids;
            kids.iter()
                .position(|&s| self.start(s) > page)
                .unwrap_or(kids.len())
        };
        self.nodes[parent].kids.insert(pos, node);
    }

    /// `_subtree_max_start(node)`. ref: :275
    fn subtree_max_start(&self, id: Id) -> i64 {
        self.kids(id)
            .into_iter()
            .map(|c| self.subtree_max_start(c))
            .fold(self.start(id), i64::max)
    }

    fn preorder(&self, roots: &[Id], out: &mut Vec<Id>) {
        for &r in roots {
            out.push(r);
            self.preorder(&self.kids(r), out);
        }
    }
}

/// `_finalize(roots, n_pages)`: end fill (shared boundary), union promotion, node ids, empty
/// `nodes` removal. ref: :207
fn finalize(a: &mut Arena, roots: &[Id], n_pages: i64) -> Vec<Value> {
    let mut flat = Vec::new();
    a.preorder(roots, &mut flat);
    for i in 0..flat.len() {
        let boundary = if i + 1 < flat.len() {
            a.start(flat[i + 1])
        } else {
            n_pages
        };
        let end = a.start(flat[i]).max(boundary);
        a.set(flat[i], "end_index", end.into());
    }
    fn promote(a: &mut Arena, nodes: &[Id]) -> i64 {
        let mut end = 0;
        for &n in nodes {
            let kids = a.kids(n);
            if !kids.is_empty() {
                let e = a.get_i64(n, "end_index").max(promote(a, &kids));
                a.set(n, "end_index", e.into());
            }
            end = end.max(a.get_i64(n, "end_index"));
        }
        end
    }
    promote(a, roots);
    for (i, &n) in flat.iter().enumerate() {
        a.set(n, "node_id", Value::String(format!("{i:04}")));
        let node = &mut a.nodes[n];
        if node.fields.contains_key(NODES) && node.kids.is_empty() {
            node.fields.shift_remove(NODES);
        }
    }
    roots.iter().map(|&r| a.value(r)).collect()
}

/// `_build_bookmark_nodes(entries)`. ref: :240
fn build_bookmark_nodes(a: &mut Arena, entries: &[Entry]) -> Vec<Id> {
    let mut roots = Vec::new();
    let mut stack: Vec<Id> = Vec::new();
    for e in entries {
        let node = a.new_node(&e.title, e.page);
        while stack.len() >= e.level {
            stack.pop();
        }
        match stack.last() {
            Some(&p) => a.nodes[p].kids.push(node),
            None => roots.push(node),
        }
        stack.push(node);
    }
    roots
}

/// `bookmarks_to_structure(entries, n_pages)`. ref: :258
pub fn bookmarks_to_structure(entries: &[Entry], n_pages: i64) -> Vec<Value> {
    let mut a = Arena::default();
    let roots = build_bookmark_nodes(&mut a, entries);
    finalize(&mut a, &roots, n_pages)
}

fn similarity(a: &str, b: &str) -> f64 {
    ratio_str(a, b)
}

/// `_same_heading(node, chapter)`. ref: :263
fn same_heading(a: &Arena, node: Id, chapter: Id) -> bool {
    if a.start(node) != a.start(chapter) {
        return false;
    }
    let (nn, cn) = (
        normalize_title(&a.title(node)),
        normalize_title(&a.title(chapter)),
    );
    if nn.is_empty() || cn.is_empty() {
        return false;
    }
    nn == cn || nn.ends_with(&cn) || cn.ends_with(&nn) || similarity(&nn, &cn) >= REPAIR_SIMILARITY
}

/// `_title_in_text(entry, page_norms)`. ref: :286
fn title_in_text(e: &Entry, page_norms: &[String]) -> bool {
    let norm = normalize_title(&e.title);
    if norm.is_empty() {
        return false;
    }
    let start = (e.page - 2).max(0);
    let end = (page_norms.len() as i64).min(e.page + 1);
    (start..end).any(|p| page_norms[p as usize].contains(&norm))
}

/// `_repair_titles(structure, entries)`. ref: :298
fn repair_titles(a: &mut Arena, roots: &[Id], entries: &[Entry]) {
    let mut by_page: HashMap<i64, Vec<&Entry>> = HashMap::new();
    for e in entries {
        by_page.entry(e.page).or_default().push(e);
    }
    fn visit(a: &mut Arena, n: Id, by_page: &HashMap<i64, Vec<&Entry>>) {
        let nn = normalize_title(&a.title(n));
        let mut best: Option<&Entry> = None;
        let mut best_ratio = REPAIR_SIMILARITY;
        for &e in by_page.get(&a.start(n)).map(Vec::as_slice).unwrap_or(&[]) {
            let en = normalize_title(&e.title);
            if nn.is_empty() || en.is_empty() || nn.ends_with(&en) || en.ends_with(&nn) {
                continue;
            }
            let ratio = similarity(&nn, &en);
            if ratio >= best_ratio {
                best = Some(e);
                best_ratio = ratio;
            }
        }
        if let Some(e) = best {
            a.set(n, "title", Value::String(e.title.clone()));
        }
        for c in a.kids(n) {
            visit(a, c, by_page);
        }
    }
    for &r in roots {
        visit(a, r, &by_page);
    }
}

/// `_find_entry_node(root, entry)`. ref: :330
fn find_entry_node(a: &Arena, root: Id, e: &Entry) -> Option<Id> {
    let en = normalize_title(&e.title);
    fn visit(a: &Arena, n: Id, e: &Entry, en: &str) -> Option<Id> {
        if (a.start(n) - e.page).abs() <= 1 {
            let nn = normalize_title(&a.title(n));
            if !en.is_empty()
                && !nn.is_empty()
                && (nn.ends_with(en)
                    || en.ends_with(&nn)
                    || similarity(&nn, en) >= REPAIR_SIMILARITY)
            {
                return Some(n);
            }
        }
        a.kids(n).into_iter().find_map(|c| visit(a, c, e, en))
    }
    visit(a, root, e, &en)
}

/// Index of the last range whose start page is <= `page` (ranges sorted by start).
fn range_index(starts: &[i64], page: i64) -> Option<usize> {
    let mut idx = None;
    for (i, &s) in starts.iter().enumerate() {
        if page >= s {
            idx = Some(i);
        } else {
            break;
        }
    }
    idx
}

/// `merge_bookmark_skeleton(structure, entries, n_pages, page_texts)` (SKELETON tier).
/// ref: :351
pub fn merge_bookmark_skeleton(
    structure: &[Value],
    entries: &[Entry],
    n_pages: i64,
    page_texts: Option<&[String]>,
) -> Vec<Value> {
    let chapters: Vec<&Entry> = entries.iter().filter(|e| e.level == 1).collect();
    if chapters.is_empty() {
        return structure.to_vec();
    }
    let mut a = Arena::default();
    let roots: Vec<Id> = structure.iter().map(|v| a.add_value(v)).collect();
    let has_texts = page_texts.is_some_and(|p| !p.is_empty());
    if has_texts {
        repair_titles(&mut a, &roots, entries);
    }
    let chapter_nodes: Vec<Id> = chapters
        .iter()
        .map(|e| a.new_node(&e.title, e.page))
        .collect();
    let starts: Vec<i64> = chapters.iter().map(|e| e.page).collect();
    let range_end: Vec<i64> = (0..chapters.len())
        .map(|i| chapters.get(i + 1).map_or(n_pages + 1, |c| c.page))
        .collect();
    let mut front: Vec<Id> = Vec::new();

    struct Ctx<'c> {
        chapter_nodes: &'c [Id],
        starts: &'c [i64],
        range_end: &'c [i64],
    }
    fn place(a: &mut Arena, cx: &Ctx<'_>, front: &mut Vec<Id>, node: Id) {
        let children = a.kids(node);
        let Some(idx) = range_index(cx.starts, a.start(node)) else {
            if a.subtree_max_start(node) < cx.starts[0] {
                front.push(node);
            } else {
                let leaf = a.copy_leaf(node);
                front.push(leaf);
                for c in children {
                    place(a, cx, front, c);
                }
            }
            return;
        };
        let target = cx.chapter_nodes[idx];
        if a.subtree_max_start(node) < cx.range_end[idx] {
            if same_heading(a, node, target) {
                a.nodes[target].kids.extend(children);
            } else {
                a.nodes[target].kids.push(node);
            }
            return;
        }
        if !same_heading(a, node, target) {
            let leaf = a.copy_leaf(node);
            a.nodes[target].kids.push(leaf);
        }
        for c in children {
            place(a, cx, front, c);
        }
    }
    let cx = Ctx {
        chapter_nodes: &chapter_nodes,
        starts: &starts,
        range_end: &range_end,
    };
    for &r in &roots {
        place(&mut a, &cx, &mut front, r);
    }

    if let Some(texts) = page_texts.filter(|_| has_texts) {
        let page_norms: Vec<String> = texts.iter().map(|t| normalize_title(t)).collect();
        let mut node_for_entry: HashMap<usize, Id> = HashMap::new();
        let mut chapter_pos = 0;
        let mut stack: Vec<(usize, usize)> = Vec::new();
        for (index, e) in entries.iter().enumerate() {
            while stack.last().is_some_and(|&(l, _)| l >= e.level) {
                stack.pop();
            }
            let parent_index = stack.last().map(|&(_, i)| i);
            stack.push((e.level, index));
            if e.level == 1 {
                node_for_entry.insert(index, chapter_nodes[chapter_pos]);
                chapter_pos += 1;
                continue;
            }
            let Some(idx) = range_index(&starts, e.page) else {
                continue;
            };
            if let Some(existing) = find_entry_node(&a, chapter_nodes[idx], e) {
                node_for_entry.insert(index, existing);
                continue;
            }
            if is_generic_title(&e.title) || !title_in_text(e, &page_norms) {
                continue;
            }
            let node = a.new_node(&e.title, e.page);
            let parent = parent_index
                .and_then(|p| node_for_entry.get(&p).copied())
                .unwrap_or(chapter_nodes[idx]);
            a.ensure_kids(parent);
            a.insert_by_page(parent, node, e.page);
            node_for_entry.insert(index, node);
        }
    }
    let all: Vec<Id> = front
        .into_iter()
        .chain(chapter_nodes.iter().copied())
        .collect();
    finalize(&mut a, &all, n_pages)
}

/// `merge_bookmark_tree(structure, entries, n_pages)` (FULL tier). ref: :480
pub fn merge_bookmark_tree(structure: &[Value], entries: &[Entry], n_pages: i64) -> Vec<Value> {
    let mut a = Arena::default();
    let det_roots: Vec<Id> = structure.iter().map(|v| a.add_value(v)).collect();
    let roots = build_bookmark_nodes(&mut a, entries);
    let mut flat = Vec::new();
    a.preorder(&roots, &mut flat);
    if flat.is_empty() {
        return structure.to_vec();
    }
    let starts: Vec<i64> = flat.iter().map(|&n| a.start(n)).collect();
    let range_end: Vec<i64> = (0..flat.len())
        .map(|p| starts.get(p + 1).copied().unwrap_or(n_pages + 1))
        .collect();

    // title counts over the detected tree, before pruning
    let mut det_flat = Vec::new();
    a.preorder(&det_roots, &mut det_flat);
    let mut title_counts: HashMap<String, usize> = HashMap::new();
    for &n in &det_flat {
        *title_counts
            .entry(normalize_title(&a.title(n)))
            .or_default() += 1;
    }
    let filtered_out = |a: &Arena, n: Id| -> bool {
        let title = a.title(n);
        let norm = normalize_title(&title);
        if !norm.is_empty() && title_counts.get(&norm).copied().unwrap_or(0) >= BACKFILL_DUP_MIN {
            return true;
        }
        title.chars().count() > BACKFILL_MAX_TITLE
    };
    fn prune(a: &mut Arena, nodes: &[Id], filtered_out: &dyn Fn(&Arena, Id) -> bool) -> Vec<Id> {
        let mut kept = Vec::new();
        for &n in nodes {
            let kids = a.kids(n);
            let children = prune(a, &kids, filtered_out);
            if filtered_out(a, n) {
                kept.extend(children);
            } else {
                a.set_kids(n, children);
                kept.push(n);
            }
        }
        kept
    }
    let pruned = prune(&mut a, &det_roots, &filtered_out);

    let mut front: Vec<Id> = Vec::new();
    struct Ctx<'c> {
        flat: &'c [Id],
        starts: &'c [i64],
        range_end: &'c [i64],
    }
    fn place(a: &mut Arena, cx: &Ctx<'_>, front: &mut Vec<Id>, node: Id) {
        let children = a.kids(node);
        let Some(idx) = range_index(cx.starts, a.start(node)) else {
            if a.subtree_max_start(node) < cx.starts[0] {
                front.push(node);
            } else {
                let leaf = a.copy_leaf(node);
                front.push(leaf);
                for c in children {
                    place(a, cx, front, c);
                }
            }
            return;
        };
        let target = cx.flat[idx];
        if a.subtree_max_start(node) < cx.range_end[idx] {
            if same_heading(a, node, target) {
                for c in children {
                    let p = a.start(c);
                    a.insert_by_page(target, c, p);
                }
            } else {
                let p = a.start(node);
                a.insert_by_page(target, node, p);
            }
            return;
        }
        if !same_heading(a, node, target) {
            let leaf = a.copy_leaf(node);
            let p = a.start(leaf);
            a.insert_by_page(target, leaf, p);
        }
        for c in children {
            place(a, cx, front, c);
        }
    }
    let cx = Ctx {
        flat: &flat,
        starts: &starts,
        range_end: &range_end,
    };
    for r in pruned {
        place(&mut a, &cx, &mut front, r);
    }
    let all: Vec<Id> = front.into_iter().chain(roots).collect();
    finalize(&mut a, &all, n_pages)
}

/// `apply_embedded_toc` on already-read bookmark entries: validate, classify, apply.
pub fn apply_bookmark_entries(
    structure: &[Value],
    raw_entries: &[Entry],
    n_pages: i64,
    page_texts: Option<&[String]>,
) -> (Vec<Value>, &'static str) {
    let entries = validate_bookmarks(raw_entries, n_pages);
    match classify_bookmarks(&entries, n_pages) {
        Tier::Full => (
            merge_bookmark_tree(structure, &entries, n_pages),
            "bookmarks",
        ),
        Tier::Skeleton => (
            merge_bookmark_skeleton(structure, &entries, n_pages, page_texts),
            "hybrid",
        ),
        Tier::Ignore => (structure.to_vec(), "detected"),
    }
}

/// `apply_embedded_toc(structure, doc_handle, n_pages, page_texts)`: classify the document's
/// bookmarks and apply the matching tier. Returns `(structure, toc_source)` with toc_source
/// `"bookmarks"` (FULL), `"hybrid"` (SKELETON) or `"detected"` (IGNORE, input untouched).
/// ref: :588
pub fn apply_embedded_toc(
    structure: &[Value],
    pdf: PdfSource<'_>,
    n_pages: i64,
    page_texts: Option<&[String]>,
) -> (Vec<Value>, &'static str) {
    apply_bookmark_entries(structure, &read_bookmarks(pdf), n_pages, page_texts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization() {
        assert_eq!(
            normalize_title("II. The General Assembly"),
            "2thegeneralassembly"
        );
        assert_eq!(
            normalize_title("2. The General Assembly"),
            "2thegeneralassembly"
        );
        assert_eq!(normalize_title("Mix of $x^2$ di"), "mixofdi");
        assert_eq!(normalize_title("snake_case x_1"), "snakecase101");
        assert_eq!(title_template("Chapter 12"), "chapter#");
        assert_eq!(title_template("Part IV-3a"), "part##a");
        assert_eq!(title_template("A1B22"), "a#b#");
        assert_eq!(strip_latex("a $b\n c$ d $e$ f"), "a $b\n ce$ f");
    }

    #[test]
    fn generic_titles() {
        for t in [
            "12",
            "Page 3",
            "PAGE3",
            "slide 4",
            "ſlide 4",
            "Folİe 2",
            "document   page 7",
            "5\n",
            "٣",
        ] {
            assert!(is_generic_title(t), "{t:?}");
        }
        for t in [
            "Page",
            "Page 3a",
            "documentpage 1",
            "Chapter 1",
            "",
            "5\n\n",
            "page 3 ",
        ] {
            assert!(!is_generic_title(t), "{t:?}");
        }
    }

    fn e(title: &str, level: usize, page: i64) -> Entry {
        Entry {
            title: title.into(),
            level,
            page,
        }
    }

    #[test]
    fn classify_tiers() {
        let pages: Vec<Entry> = (1..=6).map(|i| e(&format!("Page {i}"), 1, i)).collect();
        assert_eq!(classify_bookmarks(&pages, 10), Tier::Ignore);
        let chapters: Vec<Entry> = (1..=6).map(|i| e(&format!("Chapter {i}"), 1, i)).collect();
        assert_eq!(classify_bookmarks(&chapters, 10), Tier::Skeleton);
        let codes: Vec<Entry> = (1..=6)
            .map(|i| e(&format!("file {i} v{i}"), 1, i))
            .collect();
        assert_eq!(classify_bookmarks(&codes, 10), Tier::Ignore);
        let mut deep = chapters.clone();
        deep.push(e("Intro", 2, 6));
        assert_eq!(classify_bookmarks(&deep, 10), Tier::Full);
        assert_eq!(classify_bookmarks(&deep, 200), Tier::Skeleton);
    }

    #[test]
    fn validate_restacks_levels() {
        let raw = vec![
            e("A", 1, 2),
            e("B", 3, 3),
            e("C", 2, 1),
            e("D", 5, 4),
            e("E", 1, 99),
        ];
        let v = validate_bookmarks(&raw, 10);
        let got: Vec<_> = v.iter().map(|x| (x.title.as_str(), x.level)).collect();
        assert_eq!(got, vec![("A", 1), ("B", 2), ("D", 3)]);
    }
}
