//! Per-page working records of the extractor (the reference's char/object/chunk dicts).

/// A PDFium text object. ref: geometry.py::_collect_text_objs (dict fields in comments).
#[derive(Debug, Clone)]
pub struct TextObj {
    /// FPDF_FONT address: per-document font identity ("font_key"), also used to call back into
    /// PDFium during pass 1 (kept as an address so pass 2 can run on other threads).
    pub font_key: usize,
    pub fs_raw: f64,
    pub scale_x: f64,
    pub scale_y: f64,
    pub fs_eff: f64,
    pub l: f64,
    pub r: f64,
    pub b: f64,
    pub t: f64,
    pub area: f64,
    pub font_name: String,
    pub weight: i32,
    /// 0/90/180/270 cardinal, -1 oblique.
    pub rot: i32,
    pub mtx: [f64; 4],
    pub page_order: usize,
    pub vertical: bool,
    pub tz: f64,
}

/// A textpage char after pass 1. ref: char_extract.py::_extract_raw_chars (raw_chars dicts).
#[derive(Debug, Clone)]
pub struct RawChar {
    /// Textpage index; fractional for synthesized glyphs.
    pub i: f64,
    pub ch: String,
    pub is_gen: bool,
    pub is_ws: bool,
    pub is_mn: bool,
    pub is_cf: bool,
    pub ox: f64,
    pub oy: f64,
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
    pub box_top: f64,
    pub box_bottom: f64,
    pub cell_top: f64,
    pub cell_bot: f64,
    pub w_raw: f64,
    pub w_synth: Option<f64>,
    /// Index into the page's `TextObj` list.
    pub obj: usize,
    pub font_name: String,
    pub drop: bool,
}

/// Vertical-writing pen state. ref: char_extract.py::_finalize_chars (v_pen_x/v_pen_y/v_after).
#[derive(Debug, Clone, Copy)]
pub struct VPen {
    pub x: f64,
    pub y: f64,
    pub after: f64,
}

/// A finalized glyph fed to the merger. ref: char_extract.py::_finalize_chars
#[derive(Debug, Clone)]
pub struct FinChar {
    pub ch: String,
    pub is_ws: bool,
    pub is_mn: bool,
    pub is_cf: bool,
    pub ox: f64,
    pub oy: f64,
    pub glyph_w: f64,
    pub fs: f64,
    pub fs_x: f64,
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
    pub font_name: String,
    pub font_key: usize,
    pub weight: i32,
    pub obj: usize,
    pub vpen: Option<VPen>,
}

/// A merged text item ("chunk"). ref: merge.py (chunk dicts) and remerge.py.
#[derive(Debug, Clone)]
pub struct Item {
    pub pieces: Vec<String>,
    pub obj: usize,
    /// First glyph's object matrix; `None` for synthetic spaces and oblique items.
    pub mtx0: Option<[f64; 4]>,
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
    pub fs: f64,
    pub glyph_w: f64,
    pub font_name: String,
    pub font_key: usize,
    pub weight: i32,
    /// Vertical pen state (`v_pen_x`, `v_pen_y`, `v_after`, `v_last_x`) when opened on a
    /// vertical-CMap glyph.
    pub v: Option<(VPen, f64)>,
}
