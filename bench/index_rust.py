"""Index the FinanceBench (or any) PDFs with pageindex-rs and write the same index.csv /
localizability.json as bench/index_python.py, so bench/compare.py can put them side by side.

  cargo build --release -p pi-cli
  python bench/index_rust.py --run rust-llmfree            # FinanceBench, LLM-free (merge only)
  python bench/index_rust.py --run rust-edgar --pdf-dir bench/data/edgar

Runs `pageindex-rs index --no-summary --optimize merge --report <run>/report.jsonl
--tree-out <run>/trees --storage <run>/.pageindex <pdfs...>`; the .pageindex it writes is
readable by the Python SDK, so bench/retrieve_python.py --index-dir can answer over it.
"""
from __future__ import annotations

import argparse
import csv
import json
import os
import statistics
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
DATA = HERE / "data"
ROOT = HERE.parent

sys.path.insert(0, str(HERE))
from index_python import smallest_containing, walk  # noqa: E402


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", default=time.strftime("rust-%Y%m%d-%H%M%S"))
    ap.add_argument("--pdf-dir", type=Path)
    ap.add_argument("--docs", nargs="*")
    ap.add_argument("--binary", type=Path, default=ROOT / "target" / "release" / "pageindex-rs")
    ap.add_argument("--extra", nargs=argparse.REMAINDER, default=[],
                    help="extra args passed to `pageindex-rs index` (e.g. --summary --ocr auto)")
    args = ap.parse_args()

    if args.pdf_dir:
        questions = []
        pdf_of = {p.stem: p for p in sorted(args.pdf_dir.glob("*.pdf"))}
    else:
        questions = [json.loads(l) for l in (DATA / "financebench_open_source.jsonl").read_text().splitlines()
                     if l.strip()]
        pdf_of = {d: DATA / "pdfs" / f"{d}.pdf" for d in {q["doc_name"] for q in questions}}
    docs = sorted(pdf_of)
    if args.docs:
        docs = [d for d in docs if d in set(args.docs)]

    out = HERE / "results" / args.run
    (out / "trees").mkdir(parents=True, exist_ok=True)
    cmd = [str(args.binary), "index", "--no-summary", "--optimize", "merge",
           "--report", str(out / "report.jsonl"), "--tree-out", str(out / "trees"),
           "--storage", str(out / ".pageindex"), *args.extra, *[str(pdf_of[d]) for d in docs]]
    print(" ".join(cmd[:12]), "...", flush=True)
    t0 = time.perf_counter()
    subprocess.run(cmd, check=True, env={**os.environ})
    wall = time.perf_counter() - t0

    report = {json.loads(l)["doc"]: json.loads(l) for l in (out / "report.jsonl").read_text().splitlines() if l.strip()}
    rows, trees = [], {}
    for d in docs:
        r = report.get(f"{d}.pdf") or report.get(d) or {}
        row = {"doc": d, "pages": r.get("pages")}
        if r.get("error"):
            row["error"] = r["error"]
            rows.append(row)
            continue
        tree = json.loads((out / "trees" / f"{d}.json").read_text())
        trees[d] = tree
        row["extract_s"] = r.get("extract_s")
        row["ms_per_page"] = round(1000 * r.get("extract_s", 0) / max(r.get("pages") or 1, 1), 1)
        row["total_s"] = r.get("total_s")
        row["toc_source"] = tree.get("toc_source")
        nodes = list(walk(tree.get("structure", [])))
        row["merged_nodes"] = len(nodes)
        row["merged_depth"] = max((dd for _, dd in nodes), default=0)
        row["merged_leaf_pages_mean"] = round(statistics.mean(
            [n["end_index"] - n["start_index"] + 1 for n, _ in nodes if not n.get("nodes")] or [0]), 2)
        rows.append(row)
    cols = sorted({k for r in rows for k in r}, key=lambda k: (k != "doc", k))
    with open(out / "index.csv", "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=cols)
        w.writeheader()
        w.writerows(rows)
    loc = []
    for q in questions:
        if q["doc_name"] not in trees:
            continue
        for ev in q["evidence"]:
            page1 = int(ev["evidence_page_num"]) + 1
            best = smallest_containing(trees[q["doc_name"]].get("structure", []), page1)
            loc.append({"id": q["financebench_id"], "doc": q["doc_name"], "page": page1,
                        "span": best[0] if best else None, "depth": best[1] if best else None,
                        "node": best[2] if best else None})
    (out / "localizability.json").write_text(json.dumps(loc, indent=1, ensure_ascii=False))
    print(f"{len(docs)} docs in {wall:.1f}s wall -> {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
