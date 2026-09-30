"""Record CPython values for pi-outline's embedded_toc tests.

Writes crates/pi-outline/tests/fixtures/embedded_toc.json:
  titles     _normalize_title / _title_template / _GENERIC_TITLE on tricky strings
  bookmarks  for every PDF found (FinanceBench bench set + reference examples/tests): page
             count, read_bookmarks count + sha256 of its JSON, the validated entries' sha256,
             and the tier -- compact, no text
  hybrid     SKELETON-tier docs from extra golden dumps (dump_reference.py output dirs given
             with --hybrid): raw entries, stage-08 tree, page texts, expected stage-09

Run with the reference venv from the repo root:
  /home/user/PageIndex-rust/parity/.venv/bin/python crates/pi-outline/gen/embedded_toc_fixture.py \
      --hybrid <scratch>/AMCOR_2023Q4_EARNINGS <scratch>/PEPSICO_2023Q1_EARNINGS
"""
import argparse
import glob
import hashlib
import json
import os
from pathlib import Path

import pypdfium2 as pdfium
from pageindex.flash import embedded_toc as E

HERE = Path(__file__).resolve().parent
PDF_GLOBS = [
    "/home/user/PageIndex-rust/bench/data/pdfs/*.pdf",
    "/home/user/PageIndex-rust/parity/.ref/examples/documents/*.pdf",
    "/home/user/PageIndex-rust/parity/.ref/tests/data/flash/*.pdf",
]
TITLES = [
    "II. The General Assembly", "2. The General Assembly", "Chapter XIV", "Mix of $x^2$ di",
    "a $b\n c$ d $e$ f", "snake_case x_1", "İstanbul Iİı", "ſlide 4", "Folİe 2", "PAGE 12",
    "document \t page 7", "5\n", "5\n\n", "page 3 ", "٣٤", "x² ½", "第三章 概述",
    "ΣΊΣΥΦΟΣ Σ", "Part IV-3a", "A1B22", "civil liv mix", "Ⅻ roman numeral", "café",
    "Page", "Slide", "", "Document Page", "document page12", "M", "mmmcmxcix", "iiii", "vx",
]


def sha(obj):
    return hashlib.sha256(json.dumps(obj, ensure_ascii=False, separators=(",", ":")).encode()).hexdigest()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--hybrid", nargs="*", default=[])
    args = ap.parse_args()
    titles = [{"title": t, "norm": E._normalize_title(t), "template": E._title_template(t),
               "generic": bool(E._GENERIC_TITLE.match(t))} for t in TITLES]
    books = []
    for pattern in PDF_GLOBS:
        for p in sorted(glob.glob(pattern)):
            try:
                n = len(pdfium.PdfDocument(p))
            except Exception:
                continue
            raw = E.read_bookmarks(p)
            val = E.validate_bookmarks(raw, n)
            books.append({"file": p, "n_pages": n, "count": len(raw), "sha": sha(raw),
                          "validated": len(val), "validated_sha": sha(val),
                          "tier": E.classify_bookmarks(val, n)})
    hybrid = []
    for d in args.hybrid:
        d = Path(d)
        load = lambda name: json.loads((d / name).read_text())
        s08, s09, texts = load("08_tree_raw.json"), load("09_tree_bookmarks.json"), load("page_texts.json")
        pdf = next(p for pat in PDF_GLOBS for p in glob.glob(pat) if Path(p).name == s09["doc"])
        hybrid.append({"doc": s09["doc"], "n_pages": len(texts["data"]), "entries": E.read_bookmarks(pdf),
                       "structure": s08["data"], "page_texts": texts["data"], "expected": s09["data"]})
    out = HERE.parent / "tests" / "fixtures" / "embedded_toc.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps({"titles": titles, "bookmarks": books, "hybrid": hybrid},
                              ensure_ascii=False, indent=0) + "\n")
    print(out, os.path.getsize(out), len(books))


if __name__ == "__main__":
    main()
