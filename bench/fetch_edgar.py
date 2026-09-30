"""Fetch recent SEC filings for a ticker list from EDGAR and render them to PDF.

Run this on your own machine; it is not run in the sandbox. EDGAR requires a descriptive
User-Agent with contact details, and limits clients to 10 requests/second.

  export SEC_USER_AGENT="Your Name your@email.com"
  python bench/fetch_edgar.py --tickers bench/tickers_30.txt --forms 10-K --per-form 1
  python bench/fetch_edgar.py --forms 10-K 10-Q --per-form 2 --no-render     # HTML only

Output: bench/data/edgar/<TICKER>_<FORM>_<period>.{htm,pdf} and bench/data/edgar/manifest.json.
PDF rendering uses Playwright + Chromium (`pip install playwright && playwright install chromium`).
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import threading
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE / "data" / "edgar"
TICKERS_URL = "https://www.sec.gov/files/company_tickers.json"
SUBMISSIONS_URL = "https://data.sec.gov/submissions/CIK{cik:010d}.json"
ARCHIVE_URL = "https://www.sec.gov/Archives/edgar/data/{cik}/{acc}/{doc}"


class Client:
    """Minimal EDGAR client: required User-Agent, <= `rate` requests/second, retries."""

    def __init__(self, user_agent: str, rate: float = 8.0):
        self.ua = user_agent
        self.min_gap = 1.0 / rate
        self.last = 0.0
        self.lock = threading.Lock()

    def get(self, url: str, retries: int = 4) -> bytes:
        for attempt in range(retries):
            with self.lock:
                wait = self.last + self.min_gap - time.monotonic()
                if wait > 0:
                    time.sleep(wait)
                self.last = time.monotonic()
            req = urllib.request.Request(url, headers={"User-Agent": self.ua, "Accept-Encoding": "identity"})
            try:
                with urllib.request.urlopen(req, timeout=60) as r:
                    return r.read()
            except Exception as e:  # noqa: BLE001 - retry any transport / 429 error
                if attempt == retries - 1:
                    raise RuntimeError(f"GET {url}: {e}") from e
                time.sleep(2 ** (attempt + 1))
        raise AssertionError("unreachable")


def read_tickers(path: Path) -> list[str]:
    return [l.strip().upper() for l in path.read_text().splitlines() if l.strip() and not l.startswith("#")]


def latest_filings(client: Client, cik: int, forms: list[str], per_form: int) -> list[dict]:
    sub = json.loads(client.get(SUBMISSIONS_URL.format(cik=cik)))
    recent = sub["filings"]["recent"]
    picked: dict[str, list[dict]] = {f: [] for f in forms}
    for i, form in enumerate(recent["form"]):
        if form in picked and len(picked[form]) < per_form and recent["primaryDocument"][i]:
            picked[form].append({
                "form": form,
                "accession": recent["accessionNumber"][i],
                "filed": recent["filingDate"][i],
                "period": recent["reportDate"][i] or recent["filingDate"][i],
                "primary_doc": recent["primaryDocument"][i],
            })
    return [f for fs in picked.values() for f in fs]


def render_pdfs(jobs: list[tuple[Path, Path]]) -> None:
    from playwright.sync_api import sync_playwright

    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        for html, pdf in jobs:
            page.goto(html.resolve().as_uri(), wait_until="load", timeout=180_000)
            page.pdf(path=str(pdf), format="Letter", print_background=False,
                     margin={"top": "0.5in", "bottom": "0.5in", "left": "0.5in", "right": "0.5in"})
            print(f"rendered {pdf.name}", flush=True)
        browser.close()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--tickers", type=Path, default=HERE / "tickers_30.txt")
    ap.add_argument("--forms", nargs="+", default=["10-K"])
    ap.add_argument("--per-form", type=int, default=1, help="most recent N filings per form type")
    ap.add_argument("--no-render", action="store_true", help="keep HTML only")
    ap.add_argument("--out", type=Path, default=OUT)
    args = ap.parse_args()

    ua = os.environ.get("SEC_USER_AGENT")
    if not ua or "@" not in ua:
        sys.exit('set SEC_USER_AGENT="Your Name you@example.com" (EDGAR requires contact details)')
    client = Client(ua)
    args.out.mkdir(parents=True, exist_ok=True)

    cik_by_ticker = {v["ticker"].upper(): int(v["cik_str"])
                     for v in json.loads(client.get(TICKERS_URL)).values()}
    manifest_path = args.out / "manifest.json"
    manifest = json.loads(manifest_path.read_text()) if manifest_path.exists() else {}
    render_jobs = []
    for ticker in read_tickers(args.tickers):
        cik = cik_by_ticker.get(ticker)
        if cik is None:
            print(f"{ticker}: not found in company_tickers.json", file=sys.stderr)
            continue
        for f in latest_filings(client, cik, args.forms, args.per_form):
            stem = f"{ticker}_{f['form'].replace('/', '')}_{f['period']}"
            html = args.out / f"{stem}.htm"
            if not html.exists():
                url = ARCHIVE_URL.format(cik=cik, acc=f["accession"].replace("-", ""), doc=f["primary_doc"])
                html.write_bytes(client.get(url))
                print(f"fetched {html.name}", flush=True)
            pdf = args.out / f"{stem}.pdf"
            if not args.no_render and not pdf.exists():
                render_jobs.append((html, pdf))
            manifest[stem] = {"ticker": ticker, "cik": cik, **f,
                              "html": html.name, "pdf": None if args.no_render else pdf.name}
    if render_jobs:
        render_pdfs(render_jobs)
    manifest_path.write_text(json.dumps(manifest, indent=1, sort_keys=True))
    print(f"{len(manifest)} filings in {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
