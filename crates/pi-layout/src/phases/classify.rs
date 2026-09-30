//! Stage 05 orchestration: header/footer, watermark, TOC/boilerplate classification, body
//! paragraphs, title selection and echo marking, captions, section openers and caption regions.
//!
//! ref: pageindex/flash/main.py::extract_toc steps 5-10 (as copied in parity/dump_reference.py::run)

use pi_pycompat::unicode;

use super::{BlockRef, Document};
use crate::classification::body_text::is_body_paragraph;
use crate::classification::header_footer::{bounded_edit_distance, detect_header_footer};
use crate::classification::keyword_tables::normalize_text_key;
use crate::classification::toc_boilerplate::{mark_toc_and_boilerplate, mark_watermarks};
use crate::heading_detection::{OutlineNode, find_section_openers};
use crate::labels::caption_regions::{
    CaptionContext, CaptionedRegion, build_caption_regions, detect_captions,
};
use crate::model::block::{deaccented_text, dominant_style_of};
use crate::title::detect_title;

/// Stage 05 results besides the per-block / per-page flags set on the document.
#[derive(Debug)]
pub struct Classified {
    pub doc_title: Option<String>,
    /// 1-based title page, 0 when no title was found.
    pub title_page: usize,
    pub title_blocks: Vec<BlockRef>,
    pub caption_regions: Vec<CaptionedRegion>,
    pub section_openers: Vec<OutlineNode>,
    pub captions: CaptionContext,
}

/// Runs stage 05 on a document built by [`super::build_document`].
// ref: main.py::extract_toc (steps 5-10)
pub fn classify_document(doc: &Document) -> Classified {
    // 5) Header / footer / watermark / TOC pages.
    detect_header_footer(doc, 1);
    detect_header_footer(doc, 2);
    mark_watermarks(doc);
    mark_toc_and_boilerplate(doc);

    // 6) Body paragraphs, page body flags and body style hashes.
    for page in &doc.pages {
        for &bid in &page.output {
            let b = &page.blocks[bid];
            if b.kind.get() == 0 {
                let body = is_body_paragraph(&doc.stats, page, b);
                b.is_body_paragraph.set(body);
                if body {
                    page.has_body.set(true);
                    page.body_style_hashes
                        .borrow_mut()
                        .insert(dominant_style_of(b).to_string());
                }
            }
        }
    }

    // 7) Title selection and title-echo marking.
    let mut doc_title = None;
    let mut title_blocks = Vec::new();
    let winner = detect_title(doc);
    if let Some(w) = &winner {
        let title = w.to_string_py(doc);
        let wp = &doc.pages[w.page];
        title_blocks = w.blocks.iter().map(|&b| (w.page, b)).collect();
        wp.title_or_refs.set(true);
        for &b in &w.blocks {
            wp.blocks[b].kind.set(3);
        }
        let title_norm = unicode::lower(&normalize_text_key(&title));
        let tn_len = title_norm.chars().count();
        for page in &doc.pages {
            for &bid in &page.output {
                let cb = &page.blocks[bid];
                if cb.kind.get() != 0 {
                    continue;
                }
                let normalized = unicode::lower(deaccented_text(cb, &page.layout));
                let n_len = normalized.chars().count();
                if n_len > 20
                    && tn_len > 20
                    && (normalized.starts_with(&title_norm)
                        || title_norm.starts_with(&normalized)
                        || title_norm.ends_with(&normalized))
                {
                    page.title_or_refs.set(true);
                    cb.kind.set(3);
                    continue;
                }
                let threshold = 0.2 * n_len.min(tn_len) as f64;
                if bounded_edit_distance(&normalized, &title_norm, threshold) < threshold {
                    page.title_or_refs.set(true);
                    cb.kind.set(3);
                } else if cb.is_body_paragraph.get() {
                    break;
                }
            }
        }
        doc_title = Some(title);
    }

    // 8) Keyword-labeled captions.
    let mut captions = CaptionContext::default();
    detect_captions(doc, &mut captions);

    // 9) Section openers (starting after the title page).
    let title_page = winner.as_ref().map_or(0, |w| doc.pages[w.page].index());
    let section_openers = find_section_openers(doc, title_page);

    // 10) Caption regions claim their body blocks.
    let caption_regions = build_caption_regions(doc, &mut captions);
    for r in &caption_regions {
        let page = &doc.pages[r.page];
        let head = &page.blocks[r.head];
        head.region_label.set(head.caption_label.get());
        for &b in &r.body {
            page.blocks[b].caption_claimed.set(true);
        }
    }

    Classified {
        doc_title,
        title_page,
        title_blocks,
        caption_regions,
        section_openers,
        captions,
    }
}
