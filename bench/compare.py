"""Side-by-side report: Python reference vs pageindex-rs runs (index.csv, localizability.json,
optional retrieval summary.json). Writes bench/results/<out>.md.

  python bench/compare.py --python llmfree-619cbd8 --rust rust-llmfree \
      [--python-retrieval llm-619cbd8 --rust-retrieval llm-rust] --out compare-llmfree
"""
from __future__ import annotations

import argparse
import csv
import json
import statistics
from pathlib import Path

HERE = Path(__file__).resolve().parent
RES = HERE / "results"


def load(run: str):
    rows = {r["doc"]: r for r in csv.DictReader(open(RES / run / "index.csv"))}
    loc = json.loads((RES / run / "localizability.json").read_text()) if (RES / run / "localizability.json").exists() else []
    return rows, loc


def num(v):
    try:
        return float(v)
    except (TypeError, ValueError):
        return None


def summary(rows, loc):
    ok = [r for r in rows.values() if not r.get("error")]
    pages = sum(num(r["pages"]) or 0 for r in ok)
    ext = sum(num(r.get("extract_s")) or 0 for r in ok)
    spans = sorted(x["span"] for x in loc if x.get("span") is not None)
    return {
        "docs ok": f"{len(ok)}/{len(rows)}",
        "pages": int(pages),
        "extraction s": round(ext, 1),
        "ms/page (pooled)": round(1000 * ext / max(pages, 1), 1),
        "nodes (mean)": round(statistics.mean([num(r.get("merged_nodes")) or 0 for r in ok] or [0]), 1),
        "evidence span p50": spans[len(spans) // 2] if spans else None,
        "evidence span <= 3p": f"{sum(s <= 3 for s in spans) / max(len(spans), 1):.1%}",
        "evidence span <= 10p": f"{sum(s <= 10 for s in spans) / max(len(spans), 1):.1%}",
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--python", required=True)
    ap.add_argument("--rust", required=True)
    ap.add_argument("--python-retrieval")
    ap.add_argument("--rust-retrieval")
    ap.add_argument("--out", default="compare")
    a = ap.parse_args()
    (pr, pl), (rr, rl) = load(a.python), load(a.rust)
    sp, sr = summary(pr, pl), summary(rr, rl)
    lines = [f"# Python ({a.python}) vs Rust ({a.rust})", "", "| metric | Python | Rust |", "|---|---|---|"]
    lines += [f"| {k} | {sp[k]} | {sr[k]} |" for k in sp]
    for label, pa, ra in (("retrieval", a.python_retrieval, a.rust_retrieval),):
        if pa and ra:
            js = [json.loads((RES / x / "summary.json").read_text()) for x in (pa, ra)]
            lines += ["", f"## {label}", "", "| metric | Python | Rust |", "|---|---|---|"]
            lines += [f"| {k} | {js[0].get(k)} | {js[1].get(k)} |" for k in js[0] if k != "models"]
    both = sorted(set(pr) & set(rr))
    same_tree = 0
    per_doc = ["", "## Per document", "", "| doc | pages | Py ms/p | Rust ms/p | speedup | Py nodes | Rust nodes | toc Py/Rust |",
               "|---|---|---|---|---|---|---|---|"]
    for d in both:
        p, r = pr[d], rr[d]
        pm, rm = num(p.get("ms_per_page")), num(r.get("ms_per_page"))
        sp_ = f"{pm / rm:.1f}x" if pm and rm else ""
        same_tree += p.get("merged_nodes") == r.get("merged_nodes") and p.get("toc_source") == r.get("toc_source")
        per_doc.append(f"| {d} | {p.get('pages')} | {pm} | {rm} | {sp_} | {p.get('merged_nodes')} | "
                       f"{r.get('merged_nodes')} | {p.get('toc_source')}/{r.get('toc_source')} |")
    lines += ["", f"Docs with identical node count and toc_source: {same_tree}/{len(both)}"] + per_doc
    out = RES / f"{a.out}.md"
    out.write_text("\n".join(lines) + "\n")
    print("\n".join(lines[:16]))
    print(f"-> {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
