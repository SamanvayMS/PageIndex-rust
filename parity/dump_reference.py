"""Dump per-stage golden outputs of the Python PageIndex Flash pipeline (reference @619cbd8).

For each PDF this writes parity/golden/<doc>/NN_<stage>.json:

  01_spans       per page: bbox, text, font, size, bold, italic, skew (before line building mutates bold)
  02_lines       final lines (after 2nd cluster_lines + line-number strip) with column index + page stats
  03_columns     column rects per page
  04_blocks      blocks per page: line ids, reading order, geometry, style fractions; doc stats
  05_classified  block types / body / caption flags after title + caption regions; doc title
  06_candidates  heading candidates returned by build_doc_heading_candidates (inside assemble_outline)
  07_outline     assembled outline after the validity gates (nested)
  08_tree_raw    outline_to_dict_tree output (before embedded bookmarks)
  09_tree_bookmarks  after apply_embedded_toc, with toc_source
  10_tree_optimized  after page_index_flash fallbacks + deterministic merge (optimize="merge", no LLM)

The orchestration below is a line-for-line copy of pageindex/flash/main.py::extract_toc and
phases/page_view.py::process_page with snapshot calls inserted; `--verify` re-runs the real
extract_toc and asserts the final result is identical, so drift in the copy is caught.

Obfuscated reference field names are renamed on output; see parity/FIELD_MAP.md.
Non-finite floats are encoded as the strings "inf", "-inf", "nan" (serde_json cannot read them).
"""
from __future__ import annotations

import argparse
import copy
import json
import math
import sys
import time
import tomllib
import unicodedata
from pathlib import Path

from pageindex.flash import api as flash_api
from pageindex.flash import heading_detection as hd_pkg
from pageindex.flash import main as M
from pageindex.flash.model import block_text, heading_score
from pageindex.flash.model.span_line import text_of_line
from pageindex.flash.phases import page_view as PV

HERE = Path(__file__).resolve().parent
SCHEMA_VERSION = 1


# --------------------------------------------------------------------------- #
# Serialization helpers
# --------------------------------------------------------------------------- #

def num(x):
    if isinstance(x, bool) or x is None:
        return x
    if isinstance(x, float):
        if math.isnan(x):
            return "nan"
        if math.isinf(x):
            return "inf" if x > 0 else "-inf"
    return x


def rect(r):
    # Rect(left, right, top, bottom); bottom is stored in `primary_slot`.
    return [num(r.left), num(r.right), num(r.top), num(r.primary_slot)]


def bbox(b):
    return rect(b.secondary_slot)


def span_d(s):
    return {"bbox": bbox(s), "text": s.text, "font_name": s.font_name, "font_size": num(s.font_size),
            "bold": bool(s.primary_slot), "italic": bool(s.measure_slot), "skew": num(s.previous_slot)}


def line_d(l, span_ix):
    return {"bbox": bbox(l), "text": text_of_line(l), "spans": [span_ix.get(id(s)) for s in l.primary_slot],
            "column": l.measure_slot, "bold_frac": num(l.weighted_ratio_primary),
            "italic_frac": num(l.weighted_ratio_secondary), "skew_frac": num(l.weighted_ratio_tertiary),
            "avg_font_size": num(l.metric_slot), "max_span_height": num(l.previous_slot),
            "numbering_kind": l.state_slot, "numbering_text": l.style_slot, "ink_density": num(l.cache_slot)}


def page_stats_d(ps):
    if ps is None:
        return None
    return {"line_count": ps.line_count, "total_line_weight": num(ps.secondary_slot),
            "median_overlap_gap": num(ps.tertiary_slot), "median_line_width": num(ps.previous_slot),
            "median_char_count": num(ps.style_slot), "median_density": num(ps.cache_slot),
            "median_center_y": num(ps.option_slot), "median_font_size": num(ps.primary_slot),
            "avg_char_width": num(ps.measure_slot), "dominant_font": ps.state_slot,
            "dominant_style": ps.auxiliary_slot}


def doc_stats_d(ds):
    return {"dominant_script": ds.tertiary_slot, "landscape_pages": ds.style_slot, "total_lines": ds.cache_slot,
            "total_weight": num(ds.state_slot), "max_page_weight": num(ds.previous_slot),
            "median_page_weight": num(ds.secondary_slot), "median_line_width": num(ds.option_slot),
            "p80_density": num(ds.auxiliary_slot), "median_center_y": num(ds.measure_slot),
            "body_font_size": num(ds.primary_slot)}


def block_d(b, line_ix, full=False):
    d = {"bbox": bbox(b), "text": block_text(b), "lines": [line_ix.get(id(l)) for l in b.primary_slot],
         "orig_index": b.orig_index, "reading_order_index": b.reading_order_index,
         "isolated_centered": b.isolated_centered, "center_aligned": b.alignment_slot,
         "bold_frac": num(b.weighted_ratio_tertiary), "italic_frac": num(b.previous_slot),
         "weighted_skew": num(b.weighted_skew), "weighted_font_size": num(b.weighted_font_size),
         "density_chars": num(b.weighted_ratio_primary), "density_area": num(b.weighted_ratio_secondary),
         "max_line_height": num(b.style_slot)}
    if full:
        d.update({"type": b.type, "is_body_paragraph": b.is_body_paragraph, "used_as_heading": b.used_as_heading,
                  "caption_claimed": b.measure_slot, "caption_label": b.marker_slot,
                  "region_label": b.state_slot, "heading_score": num(heading_score(b))})
    return d


class Registry:
    """Stable ids for objects across stages: spans/lines per page, blocks as (page, orig_index)."""

    def __init__(self):
        self.span_ix: list[dict[int, int]] = []
        self.line_ix: list[dict[int, int]] = []
        self.block_ref: dict[int, list[int]] = {}

    def block(self, b):
        return self.block_ref.get(id(b))


def candidate_d(c, reg: Registry):
    page = c.page
    page_no = getattr(page, "page_index", page)
    return {"type": c.type, "page": page_no, "block": reg.block(c.group_slot), "anchor": reg.block(c.tertiary_slot),
            "numbering": list(c.numbering), "prefix": str(c.secondary_slot) if c.secondary_slot is not None else None,
            "title": str(c.primary_slot) if c.primary_slot is not None else None,
            "has_numbering": c.has_numbering, "is_prominent": c.is_prominent, "script": c.state_slot,
            "y_frac": num(c.auxiliary_slot),
            "heading_score": num(heading_score(c.group_slot)) if c.group_slot is not None else None}


def outline_d(nodes, reg):
    return [{"heading": candidate_d(n.heading, reg), "children": outline_d(n.child_nodes, reg)} for n in nodes]


# --------------------------------------------------------------------------- #
# Instrumented pipeline (copy of reference orchestration + snapshots)
# --------------------------------------------------------------------------- #

def process_page_instrumented(spans, page_num, page_bbox):
    """Copy of phases/page_view.py::process_page @619cbd8."""
    page = PV.PageView(page_num, page_bbox)
    page.text = spans
    container = PV.LinesContainer()
    container.primary_slot = PV.build_initial_lines(spans, page_bbox)
    PV.cluster_lines(container, 0.75, [])
    page.primary_slot = PV.compute_page_stats(page_bbox, container.primary_slot)
    column_context = PV.ColumnDetectionContext(page_bbox, page.primary_slot, container.primary_slot)
    page.tertiary_slot = PV.detect_columns(column_context)
    cols = PV.columns_to_x_bounds(page.tertiary_slot)
    PV.cluster_lines(container, 0.5, cols)
    container.primary_slot = PV.strip_line_numbers(page_bbox, container.primary_slot)
    page.primary_slot = PV.compute_page_stats(page_bbox, container.primary_slot)
    page.lines = container.primary_slot
    return page


def run(pdf: Path, use_embedded_toc: bool = True) -> tuple[dict, dict]:
    """Copy of flash/main.py::extract_toc @619cbd8 with snapshots. Returns (result, stages)."""
    S: dict[str, object] = {}
    reg = Registry()

    parsed, page_meta = M.parse_charlevel_meta_parallel(str(pdf), workers=1)
    S["01_spans"] = [{"page": i + 1, "viewbox": [num(v) for v in (page_meta[i][0] or [])] or None,
                      "rotation": page_meta[i][1], "spans": [span_d(s) for s in spans]}
                     for i, spans in enumerate(parsed)]
    for spans in parsed:
        reg.span_ix.append({id(s): k for k, s in enumerate(spans)})

    pages = []
    for index_value, spans in enumerate(parsed):
        viewport_box_value, rot = page_meta[index_value]
        viewport_x0, viewport_y0, viewport_x1, viewport_y1 = viewport_box_value
        viewport_width, viewport_height = abs(viewport_x1 - viewport_x0), abs(viewport_y1 - viewport_y0)
        page_width, page_height = (viewport_height, viewport_width) if rot % 180 == 90 else (viewport_width, viewport_height)
        page_bbox = M.Rect(0, page_width, page_height, 0)
        page = process_page_instrumented(spans, page_num=index_value + 1, page_bbox=page_bbox)
        if viewport_box_value is not None:
            page.viewport_box, page.rot = viewport_box_value, rot
        pages.append(page)

    for i, page in enumerate(pages):
        reg.line_ix.append({id(l): k for k, l in enumerate(page.lines)})
    S["02_lines"] = [{"page": p.page_index, "page_bbox": rect(p.bounds), "stats": page_stats_d(p.primary_slot),
                      "lines": [line_d(l, reg.span_ix[i]) for l in p.lines]} for i, p in enumerate(pages)]
    S["03_columns"] = [{"page": p.page_index, "columns": [rect(c) if hasattr(c, "left") else bbox(c)
                                                         for c in (p.tertiary_slot or [])]} for p in pages]

    doc = M.DocumentState(pages)
    doc.secondary_slot = M.compute_doc_stats(pages)

    for page in pages:
        ctx = M.BlockClusterContext(doc.secondary_slot, page.bounds, page.primary_slot, page.lines, page.tertiary_slot)
        page.blocks = M.cluster_lines_into_blocks(ctx)
        M.assign_reading_order(page, page.blocks)
    for page in pages:
        for k, b in enumerate(page.output_slot or []):
            reg.block_ref[id(b)] = [page.page_index, k]
    S["04_blocks"] = {"doc_stats": doc_stats_d(doc.secondary_slot),
                      "pages": [{"page": p.page_index,
                                 "reading_order": [reg.block(b)[1] for b in (p.secondary_slot or [])],
                                 "blocks": [block_d(b, reg.line_ix[i]) for b in (p.output_slot or [])]}
                                for i, p in enumerate(pages)]}

    M.detect_header_footer(M.HeaderFooterContext(doc, 1))
    M.detect_header_footer(M.HeaderFooterContext(doc, 2))
    M.mark_watermarks(doc)
    M.mark_toc_and_boilerplate(doc)

    from pageindex.flash.model import dominant_style_of as span_style_hash
    for page in pages:
        for block in (page.output_slot or []):
            if block.type == 0:
                block.is_body_paragraph = M.is_body_paragraph(doc.secondary_slot, page, block)
                if block.is_body_paragraph:
                    page.state_slot = True
                    page.style_slot.add(span_style_hash(block))

    from pageindex.flash.classification import bounded_edit_distance, _normalize_text_key
    doc_title = None
    title_winner = M.detect_title(doc)
    title_blocks = []
    if title_winner is not None:
        doc_title = title_winner.to_string()
        title_blocks = [reg.block(b) for b in title_winner.output_slot]
        title_winner.page.auxiliary_slot = True
        for block in title_winner.output_slot:
            block.type = 3
        title_norm = _normalize_text_key(title_winner.to_string()).lower()
        for candidate_page in doc.primary_slot:
            for candidate_block in (candidate_page.output_slot or []):
                if candidate_block.type != 0:
                    continue
                normalized = M.deaccented_text(candidate_block).lower()
                if (len(normalized) > 20 and len(title_norm) > 20 and (
                        normalized.startswith(title_norm)
                        or title_norm.startswith(normalized)
                        or title_norm.endswith(normalized))):
                    candidate_page.auxiliary_slot = True
                    candidate_block.type = 3
                    continue
                threshold = 0.2 * min(len(normalized), len(title_norm))
                if bounded_edit_distance(normalized, title_norm, threshold) < threshold:
                    candidate_page.auxiliary_slot = True
                    candidate_block.type = 3
                elif candidate_block.is_body_paragraph:
                    break

    caption_context = M.CaptionContext(doc)
    M.detect_captions(caption_context)

    from pageindex.flash.heading_detection import find_section_openers as _find_section_openers
    title_page_idx = title_winner.page.page_index if title_winner is not None else 0
    section_openers = _find_section_openers(doc, title_page_idx)

    caption_regions = M.build_caption_regions(caption_context)
    for caption_region in caption_regions:
        head_block = caption_region.primary_slot
        head_block.state_slot = head_block.marker_slot
        for body_block in caption_region.output_slot:
            body_block.measure_slot = True

    S["05_classified"] = {
        "doc_title": doc_title, "title_page": title_page_idx, "title_blocks": title_blocks,
        "caption_regions": [{"head": reg.block(r.primary_slot), "body": [reg.block(b) for b in r.output_slot],
                             "type": r.type, "score": num(r.score)} for r in caption_regions],
        "section_openers": outline_d(section_openers, reg),
        "pages": [{"page": p.page_index, "has_caption": bool(p.measure_slot), "title_or_refs": bool(p.auxiliary_slot),
                   "has_body": bool(p.state_slot),
                   "blocks": [block_d(b, reg.line_ix[i], full=True) for b in (p.output_slot or [])]}
                  for i, p in enumerate(pages)],
    }

    # 06: candidates are built inside assemble_outline via a call-time import from the package.
    captured = {}
    orig_build = hd_pkg.build_doc_heading_candidates

    def build_wrapper(*a, **kw):
        out = orig_build(*a, **kw)
        captured["candidates"] = [candidate_d(c, reg) for c in out]
        return out

    hd_pkg.build_doc_heading_candidates = build_wrapper
    try:
        outline_nodes = M.assemble_outline(doc, section_openers)
    finally:
        hd_pkg.build_doc_heading_candidates = orig_build
    S["06_candidates"] = captured.get("candidates", [])
    assembled = outline_d(outline_nodes, reg)

    gate = {}
    if M.is_outline_valid(doc, outline_nodes):
        gate["outline_valid"] = True
        gate["chapter_valid"] = bool(M.is_chapter_outline_valid(doc, outline_nodes))
        if not gate["chapter_valid"]:
            outline_nodes = []
        has_abstract_or_references = False
    else:
        gate["outline_valid"] = False
        M.mark_outline_block_types(outline_nodes)
        page_count = len(doc.primary_slot)
        if page_count >= 3:
            gate["max_gap"] = num(M.compute_max_heading_gap(outline_nodes, 1)["max_gap"])
            if gate["max_gap"] > 0.85 * page_count:
                outline_nodes = []
        has_abstract_or_references = M.has_table_or_prominent(outline_nodes)
    S["07_outline"] = {"assembled": assembled, "gate": gate, "final": outline_d(outline_nodes, reg),
                       "has_abstract_or_references": has_abstract_or_references}
    structure = M.outline_to_dict_tree(outline_nodes, total_pages=len(pages)) if outline_nodes else []
    S["08_tree_raw"] = copy.deepcopy(structure)

    page_texts = ["\n".join(block_text(b) for b in (p.secondary_slot or [])) for p in pages]
    result = {"doc_name": pdf.name, "doc_title": doc_title, "structure": structure,
              "has_abstract_or_references_section": has_abstract_or_references,
              "page_texts": page_texts, "toc_source": "detected"}
    if use_embedded_toc:
        from pageindex.flash.embedded_toc import apply_embedded_toc
        result["structure"], result["toc_source"] = apply_embedded_toc(structure, str(pdf), len(pages), page_texts=page_texts)
    S["09_tree_bookmarks"] = {"toc_source": result["toc_source"], "structure": copy.deepcopy(result["structure"])}
    S["page_texts"] = page_texts
    return result, S


def optimized(result: dict) -> dict:
    """page_index_flash(summary=False, optimize="merge") post-processing on an extract_toc result."""
    result = copy.deepcopy(result)
    structure = result.get("structure", [])
    if not structure:
        structure = flash_api._page_nodes(result.get("page_texts") or [])
        result["structure"] = structure
        result["toc_source"] = "pages" if structure else "unreadable"
    elif structure[0]["start_index"] > 1:
        flash_api._add_preface(structure)
    pages = result.pop("page_texts", None) or []
    if result.get("toc_source") == "pages" and len(structure) > flash_api.FLAT_TREE_MAX_NODES:
        return result
    if structure:
        result["optimize"] = flash_api._optimize(structure, pages, False, None)
    return result


def dump(pdf: Path, out: Path, verify: bool) -> dict:
    t0 = time.perf_counter()
    result, stages = run(pdf)
    elapsed = time.perf_counter() - t0
    stages["10_tree_optimized"] = optimized(result)
    out.mkdir(parents=True, exist_ok=True)
    for name, payload in stages.items():
        body = {"schema": SCHEMA_VERSION, "reference": "619cbd8", "python": sys.version.split()[0],
                "unicode": unicodedata.unidata_version, "doc": pdf.name, "stage": name, "data": payload}
        (out / f"{name}.json").write_text(json.dumps(body, ensure_ascii=False, indent=None, separators=(",", ":"),
                                                     allow_nan=False))
    info = {"doc": pdf.name, "pages": len(stages["01_spans"]), "seconds": round(elapsed, 3)}
    if verify:
        ref = M.extract_toc(str(pdf), workers=1)
        info["verified"] = ref == result
        if not info["verified"]:
            raise SystemExit(f"{pdf.name}: instrumented pipeline diverged from extract_toc")
    return info


def corpus_docs(path: Path, ids: list[str] | None) -> list[tuple[str, Path]]:
    cfg = tomllib.loads(path.read_text())
    root = path.parent
    docs = []
    for d in cfg["doc"]:
        if ids and d["id"] not in ids:
            continue
        p = Path(d["path"])
        docs.append((d["id"], p if p.is_absolute() else (root / p).resolve()))
    return docs


def main() -> int:
    ap = argparse.ArgumentParser()
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--corpus", type=Path)
    g.add_argument("--pdf", type=Path, nargs="+")
    ap.add_argument("--ids", nargs="*")
    ap.add_argument("--out", type=Path, default=HERE / "golden")
    ap.add_argument("--verify", action="store_true", help="also run real extract_toc and assert equality")
    args = ap.parse_args()
    docs = corpus_docs(args.corpus, args.ids) if args.corpus else [(p.stem, p) for p in args.pdf]
    for doc_id, pdf in docs:
        info = dump(pdf, args.out / doc_id, args.verify)
        print(json.dumps(info), flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
