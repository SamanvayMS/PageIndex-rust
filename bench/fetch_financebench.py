"""Download the FinanceBench open-source subset (questions + referenced SEC filing PDFs).

Data is CC-BY-NC 4.0 (patronus-ai/financebench). It is fetched into bench/data/ and never committed.
"""
from __future__ import annotations

import argparse
import concurrent.futures as cf
import hashlib
import json
import sys
import urllib.request
from pathlib import Path

RAW = "https://raw.githubusercontent.com/patronus-ai/financebench/main"
HERE = Path(__file__).resolve().parent
DATA = HERE / "data"


def _get(url: str, retries: int = 4) -> bytes:
    last: Exception | None = None
    for attempt in range(retries):
        try:
            with urllib.request.urlopen(url, timeout=120) as r:
                return r.read()
        except Exception as e:  # noqa: BLE001 - retry any transport error
            last = e
            import time

            time.sleep(2 ** (attempt + 1))
    raise RuntimeError(f"GET {url} failed: {last}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--companies", nargs="*", help="subset by FinanceBench company name")
    ap.add_argument("--workers", type=int, default=8)
    args = ap.parse_args()

    (DATA / "pdfs").mkdir(parents=True, exist_ok=True)
    for name in ("financebench_open_source.jsonl", "financebench_document_information.jsonl"):
        (DATA / name).write_bytes(_get(f"{RAW}/data/{name}"))

    questions = [json.loads(l) for l in (DATA / "financebench_open_source.jsonl").read_text().splitlines() if l.strip()]
    if args.companies:
        questions = [q for q in questions if q["company"] in set(args.companies)]
    docs = sorted({q["doc_name"] for q in questions})

    def fetch(doc: str) -> tuple[str, int, str]:
        dest = DATA / "pdfs" / f"{doc}.pdf"
        if not dest.exists():
            body = _get(f"{RAW}/pdfs/{doc}.pdf")
            if not body.startswith(b"%PDF"):
                raise RuntimeError(f"{doc}: not a PDF ({body[:16]!r})")
            dest.write_bytes(body)
        body = dest.read_bytes()
        return doc, len(body), hashlib.sha256(body).hexdigest()

    manifest = {}
    with cf.ThreadPoolExecutor(args.workers) as ex:
        for doc, size, sha in ex.map(fetch, docs):
            manifest[doc] = {"bytes": size, "sha256": sha}
    (DATA / "manifest.json").write_text(json.dumps(manifest, indent=1, sort_keys=True))
    companies = sorted({q["company"] for q in questions})
    print(f"{len(questions)} questions, {len(docs)} docs, {len(companies)} companies -> {DATA}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
