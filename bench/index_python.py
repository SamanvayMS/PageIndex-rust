"""Index the FinanceBench PDFs with the Python PageIndex reference and record LLM-free baselines.

Per doc: flash extraction wall time (summary=False, optimize=False), deterministic merge-optimized
tree (optimize="merge"), tree stats, toc_source. Per question: evidence localizability, i.e. the
page span of the smallest node that contains each gold evidence page.
"""
from __future__ import annotations

import argparse
import concurrent.futures as cf
import copy
import csv
import json
import statistics
import sys
import time
from pathlib import Path

import pypdfium2 as pdfium

from pageindex.flash import api as flash_api
from pageindex.flash.main import extract_toc

HERE = Path(__file__).resolve().parent
DATA = HERE / "data"


def walk(nodes, depth=1):
    for n in nodes:
        yield n, depth
        yield from walk(n.get("nodes", []), depth + 1)


def smallest_containing(structure, page1: int):
    best = None
    for n, depth in walk(structure):
        if n["start_index"] <= page1 <= n["end_index"]:
            span = n["end_index"] - n["start_index"] + 1
            if best is None or span < best[0] or (span == best[0] and depth > best[1]):
                best = (span, depth, n["title"])
    return best


def page_count(pdf: Path) -> int:
    doc = pdfium.PdfDocument(str(pdf))
    try:
        return len(doc)
    finally:
        doc.close()


def flash_like(pdf: str):
    """page_index_flash(summary=False) with a single extraction.

    Returns (raw, merged): raw == optimize=False, merged == optimize="merge". Mirrors
    pageindex/flash/api.py::page_index_flash at 619cbd8 (fallbacks, preface, flat-tree refusal).
    Extraction runs serially (workers=1): on this 4-core container the spawn pool is slower.
    """
    t0 = time.perf_counter()
    result = extract_toc(flash_api._validate_pdf(pdf), workers=1)
    extract_s = time.perf_counter() - t0
    structure = result.get("structure", [])
    if not structure:
        structure = flash_api._page_nodes(result.get("page_texts") or [])
        result["structure"] = structure
        result["toc_source"] = "pages" if structure else "unreadable"
    elif structure[0]["start_index"] > 1:
        flash_api._add_preface(structure)
    pages = result.pop("page_texts", None) or []
    raw = copy.deepcopy(result)
    merged = copy.deepcopy(result)
    t0 = time.perf_counter()
    refused = result.get("toc_source") == "pages" and len(structure) > flash_api.FLAT_TREE_MAX_NODES
    if merged["structure"] and not refused:
        merged["optimize"] = flash_api._optimize(merged["structure"], pages, False, None)
    merge_s = time.perf_counter() - t0
    return raw, merged, extract_s, merge_s, pages


def index_one(job):
    doc, out, pdf = job
    out, pdf = Path(out), Path(pdf)
    pages = page_count(pdf)
    row = {"doc": doc, "pages": pages}
    try:
        raw, merged, extract_s, merge_s, page_texts = flash_like(str(pdf))
    except Exception as e:  # noqa: BLE001 - record and continue
        row["error"] = f"{type(e).__name__}: {e}"[:300]
        return row, None
    row["extract_s"] = round(extract_s, 3)
    row["merge_s"] = round(merge_s, 3)
    row["ms_per_page"] = round(1000 * extract_s / max(pages, 1), 1)
    row["empty_text_pages"] = sum(1 for t in page_texts if not t.strip())
    for tag, res in (("raw", raw), ("merged", merged)):
        nodes = list(walk(res.get("structure", [])))
        row[f"{tag}_nodes"] = len(nodes)
        row[f"{tag}_depth"] = max((d for _, d in nodes), default=0)
        row[f"{tag}_top"] = len(res.get("structure", []))
        row[f"{tag}_leaf_pages_mean"] = round(statistics.mean(
            [n["end_index"] - n["start_index"] + 1 for n, _ in nodes if not n.get("nodes")] or [0]), 2)
    row["toc_source"] = raw.get("toc_source")
    (out / "trees" / f"{doc}.raw.json").write_text(json.dumps(raw, indent=1, ensure_ascii=False))
    (out / "trees" / f"{doc}.merged.json").write_text(json.dumps(merged, indent=1, ensure_ascii=False))
    (out / "trees" / f"{doc}.pages.json").write_text(json.dumps(page_texts, ensure_ascii=False))
    return row, merged


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", default=time.strftime("%Y%m%d-%H%M%S"))
    ap.add_argument("--limit", type=int)
    ap.add_argument("--docs", nargs="*")
    ap.add_argument("--pdf-dir", type=Path, help="index every PDF in this directory instead of FinanceBench")
    ap.add_argument("--jobs", type=int, default=3, help="docs indexed concurrently (each serial)")
    args = ap.parse_args()

    out = HERE / "results" / args.run
    (out / "trees").mkdir(parents=True, exist_ok=True)
    if args.pdf_dir:
        # Any directory of PDFs (e.g. bench/data/edgar from fetch_edgar.py); no gold questions.
        questions = []
        pdf_of = {p.stem: p for p in sorted(args.pdf_dir.glob("*.pdf"))}
    else:
        questions = [json.loads(l) for l in (DATA / "financebench_open_source.jsonl").read_text().splitlines()
                     if l.strip()]
        pdf_of = {d: DATA / "pdfs" / f"{d}.pdf" for d in {q["doc_name"] for q in questions}}
    docs = sorted(pdf_of)
    if args.docs:
        docs = [d for d in docs if d in set(args.docs)]
    if args.limit:
        docs = docs[: args.limit]

    jobs = [(doc, str(out), str(pdf_of[doc])) for doc in docs]
    rows, trees = [], {}
    with cf.ProcessPoolExecutor(args.jobs) as ex:
        for i, (row, merged) in enumerate(ex.map(index_one, jobs), 1):
            rows.append(row)
            if merged is not None:
                trees[row["doc"]] = merged
            if "error" in row:
                print(f"[{i}/{len(docs)}] {row['doc']}: ERROR {row['error']}", flush=True)
            else:
                print(f"[{i}/{len(docs)}] {row['doc']}: {row['pages']}p {row['extract_s']}s "
                      f"({row['ms_per_page']} ms/p) src={row['toc_source']} "
                      f"nodes raw={row['raw_nodes']} merged={row['merged_nodes']}", flush=True)

    cols = sorted({k for r in rows for k in r}, key=lambda k: (k != "doc", k))
    with open(out / "index.csv", "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=cols)
        w.writeheader()
        w.writerows(rows)

    # Evidence localizability. FinanceBench evidence_page_num is 0-based.
    loc = []
    for q in questions:
        if q["doc_name"] not in trees:
            continue
        for ev in q["evidence"]:
            page1 = int(ev["evidence_page_num"]) + 1
            best = smallest_containing(trees[q["doc_name"]].get("structure", []), page1)
            loc.append({
                "id": q["financebench_id"], "doc": q["doc_name"], "page": page1,
                "span": best[0] if best else None, "depth": best[1] if best else None,
                "node": best[2] if best else None,
            })
    (out / "localizability.json").write_text(json.dumps(loc, indent=1, ensure_ascii=False))
    print(f"wrote {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
