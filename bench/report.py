"""Summarize a bench run (index.csv + localizability.json [+ retrieval summary]) into report.md."""
from __future__ import annotations

import argparse
import csv
import json
import statistics
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(p * len(xs)))] if xs else None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("run")
    ap.add_argument("--retrieval-run")
    args = ap.parse_args()
    out = HERE / "results" / args.run
    rows = list(csv.DictReader(open(out / "index.csv")))
    ok = [r for r in rows if not r.get("error")]
    f = lambda r, k: float(r[k])
    pages = sum(int(r["pages"]) for r in ok)
    secs = sum(f(r, "extract_s") for r in ok)
    lines = [f"# Python PageIndex baseline: `{args.run}`", ""]
    lines += ["## Indexing (LLM-free flash, serial extraction, 3 docs concurrently on 4 cores)", "",
              "| metric | value |", "|---|---|",
              f"| docs ok / total | {len(ok)} / {len(rows)} |",
              f"| pages | {pages} |",
              f"| extraction total | {secs:.1f} s |",
              f"| ms/page (pooled) | {1000 * secs / max(pages, 1):.1f} |",
              f"| ms/page per doc p50 / p95 | {pct([f(r, 'ms_per_page') for r in ok], .5)} / {pct([f(r, 'ms_per_page') for r in ok], .95)} |",
              f"| merge pass total | {sum(f(r, 'merge_s') for r in ok):.2f} s |",
              f"| toc_source | {dict(Counter(r['toc_source'] for r in ok))} |",
              f"| nodes raw / merged (mean) | {statistics.mean(f(r, 'raw_nodes') for r in ok):.1f} / {statistics.mean(f(r, 'merged_nodes') for r in ok):.1f} |",
              f"| depth merged (mean, max) | {statistics.mean(f(r, 'merged_depth') for r in ok):.2f}, {max(int(r['merged_depth']) for r in ok)} |",
              f"| leaf span pages merged (mean of doc means) | {statistics.mean(f(r, 'merged_leaf_pages_mean') for r in ok):.2f} |",
              f"| docs with empty-text pages | {sum(1 for r in ok if int(r['empty_text_pages']) > 0)} |", ""]
    errs = [r for r in rows if r.get("error")]
    if errs:
        lines += ["### Errors", ""] + [f"- `{r['doc']}`: {r['error']}" for r in errs] + [""]

    loc = json.load(open(out / "localizability.json"))
    spans = [x["span"] for x in loc if x["span"] is not None]
    lines += ["## Evidence localizability (merged tree, no LLM)", "",
              "Smallest tree node containing each gold evidence page: how many pages an agent must read once it picks the right node.", "",
              "| metric | value |", "|---|---|",
              f"| evidence pages | {len(loc)} |",
              f"| not covered by any node | {sum(1 for x in loc if x['span'] is None)} |",
              f"| span p50 / p90 / max | {pct(spans, .5)} / {pct(spans, .9)} / {max(spans) if spans else None} |",
              f"| span <= 3 pages | {sum(1 for s in spans if s <= 3) / max(len(spans), 1):.1%} |",
              f"| span <= 10 pages | {sum(1 for s in spans if s <= 10) / max(len(spans), 1):.1%} |", ""]

    worst = sorted((r for r in ok), key=lambda r: -f(r, "ms_per_page"))[:5]
    lines += ["### Slowest docs (ms/page)", ""] + [f"- `{r['doc']}`: {r['ms_per_page']} ms/p, {r['pages']} p" for r in worst] + [""]

    if args.retrieval_run:
        s = json.load(open(HERE / "results" / args.retrieval_run / "summary.json"))
        lines += ["## Retrieval (reference agent + LLM judge)", "", "| metric | value |", "|---|---|"]
        lines += [f"| {k} | {v} |" for k, v in s.items()] + [""]
    (out / "report.md").write_text("\n".join(lines))
    print("\n".join(lines))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
