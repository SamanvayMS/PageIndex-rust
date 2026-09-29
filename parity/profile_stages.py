"""Spike C: per-stage wall time of the reference Flash pipeline (cProfile cumulative, workers=1).

  parity/.venv/bin/python parity/profile_stages.py --corpus parity/corpus.toml --ids prml earthmover fb_3m_2018_10k
"""
from __future__ import annotations

import argparse
import cProfile
import json
import pstats
import time
import tomllib
from pathlib import Path

from pageindex.flash.main import extract_toc

HERE = Path(__file__).resolve().parent

# (label, function name, file suffix) in pipeline order; nested rows are indented in the label.
STAGES = [
    ("parse (chars -> spans)", "parse_charlevel_meta", "parser_pdfium_charlevel/pipeline.py"),
    ("  pass1 (pdfium walk + tokenizer)", "_page_pass1", "pipeline.py"),
    ("    raw chars", "_extract_raw_chars", "char_extract.py"),
    ("    content-stream tokenize", "_tokenize_show_operators", "content_stream.py"),
    ("    text-object index", "_collect_text_objs", "geometry.py"),
    ("    char->object lookup", "_find_obj_for_char", "geometry.py"),
    ("    font unicode", "_apply_font_unicode", "unicode_apply.py"),
    ("  pass2", "_page_pass2", "pipeline.py"),
    ("  spans (merge/remerge)", "_page_spans", "pipeline.py"),
    ("lines+columns (process_page)", "process_page", "phases/page_view.py"),
    ("  build_initial_lines", "build_initial_lines", "clustering/build.py"),
    ("  cluster_lines", "cluster_lines", "clustering/build.py"),
    ("  compute_page_stats", "compute_page_stats", "stats/aggregates.py"),
    ("  detect_columns", "detect_columns", "columns/splitting.py"),
    ("doc stats", "compute_doc_stats", "stats/aggregates.py"),
    ("blocks", "cluster_lines_into_blocks", "blocks/build.py"),
    ("reading order", "assign_reading_order", "phases/page_view.py"),
    ("header/footer", "detect_header_footer", "classification/header_footer.py"),
    ("watermarks", "mark_watermarks", "classification/toc_boilerplate.py"),
    ("toc/boilerplate", "mark_toc_and_boilerplate", "classification/toc_boilerplate.py"),
    ("body paragraphs", "is_body_paragraph", "classification/body_text.py"),
    ("title", "detect_title", "title/detect.py"),
    ("captions", "detect_captions", "labels/caption_regions.py"),
    ("section openers", "find_section_openers", "heading_detection/page_scan.py"),
    ("caption regions", "build_caption_regions", "labels/caption_regions.py"),
    ("outline assembly", "assemble_outline", "outline_assembly/assembly.py"),
    ("  heading candidates", "build_doc_heading_candidates", "heading_detection/page_scan.py"),
    ("tree", "outline_to_dict_tree", "outline_assembly/assembly.py"),
    ("embedded bookmarks", "apply_embedded_toc", "embedded_toc.py"),
]


def profile(pdf: Path) -> dict:
    pr = cProfile.Profile()
    t0 = time.perf_counter()
    pr.enable()
    extract_toc(str(pdf), workers=1)
    pr.disable()
    wall = time.perf_counter() - t0
    st = pstats.Stats(pr).stats  # {(file, line, name): (cc, nc, tt, ct, callers)}
    out = {"wall_s": round(wall, 3), "stages": {}}
    for label, name, suffix in STAGES:
        ct = sum(v[3] for (f, _, n), v in st.items() if n == name and f.endswith(suffix))
        out["stages"][label] = round(ct, 3)
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", type=Path, default=HERE / "corpus.toml")
    ap.add_argument("--ids", nargs="+", required=True)
    args = ap.parse_args()
    cfg = tomllib.loads(args.corpus.read_text())
    docs = {d["id"]: (args.corpus.parent / d["path"]).resolve() for d in cfg["doc"]}
    results = {}
    for i in args.ids:
        import pypdfium2 as pdfium

        n = len(pdfium.PdfDocument(str(docs[i])))
        r = profile(docs[i])
        r["pages"] = n
        results[i] = r
        print(json.dumps({i: r}), flush=True)
    ids = list(results)
    lines = ["| stage | " + " | ".join(f"{i} ms/page" for i in ids) + " |", "|---|" + "---|" * len(ids)]
    lines.append("| **wall (profiled)** | " + " | ".join(f"**{1000 * results[i]['wall_s'] / results[i]['pages']:.1f}**" for i in ids) + " |")
    for label, _, _ in STAGES:
        row = [f"{1000 * results[i]['stages'][label] / results[i]['pages']:.1f}" for i in ids]
        lines.append(f"| {label.replace('  ', '&nbsp;&nbsp;')} | " + " | ".join(row) + " |")
    print("\n".join(lines))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
