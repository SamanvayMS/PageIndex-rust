# bench/: Python PageIndex baseline on FinanceBench

The baseline the Rust port has to match or beat. It uses the pinned reference (`parity/fetch_reference.sh`, PageIndex@619cbd8) on the FinanceBench open-source subset: 32 companies, 84 SEC filings (10-K/10-Q/8-K/earnings) and 150 questions with gold answers and 0-based evidence pages. FinanceBench is CC-BY-NC 4.0, so data and results are gitignored.

```bash
parity/fetch_reference.sh                          # reference + pinned venv in parity/.venv
PY=parity/.venv/bin/python
$PY bench/fetch_financebench.py                    # -> bench/data/{pdfs,*.jsonl,manifest.json}
$PY bench/index_python.py --run llmfree-619cbd8    # LLM-free: timings, tree stats, localizability
$PY bench/report.py llmfree-619cbd8                # -> bench/results/<run>/report.md

# needs PI_LLM_BASE_URL / PI_LLM_MODEL / PI_LLM_KEY (OpenAI-compatible); optional PI_CHAT_MODEL, PI_JUDGE_MODEL
$PY bench/retrieve_python.py --run llm-619cbd8 --limit 5   # smoke test
$PY bench/retrieve_python.py --run llm-619cbd8             # full 150 (resumable)
$PY bench/report.py llmfree-619cbd8 --retrieval-run llm-619cbd8
```

Metrics:
- **Indexing:** flash extraction ms/page (serial), tree size and depth, `toc_source`, and the merge pass.
- **Evidence localizability (no LLM):** the page span of the smallest tree node containing each gold evidence page.
- **Retrieval:** evidence-page hit rate (the gold page is among the pages fetched by `get_page_content`), pages read, tool calls, and latency p50/p95.
- **Answers:** LLM-judge label (correct / incorrect / refusal) against the gold answer.

## Run everything on your laptop (Python vs Rust)

```bash
export PDFIUM_LIB=/path/to/pdfium/lib/libpdfium.dylib   # bblanchon/pdfium-binaries release chromium/7999
export PI_LLM_BASE_URL=https://.../v1 PI_LLM_MODEL=... PI_LLM_KEY=...   # OpenAI-compatible, for retrieval
export SEC_USER_AGENT="Your Name you@example.com"                       # only with --edgar
bench/run_all.sh [--edgar] [--no-retrieval]
```

Steps:
1. **LLM-free indexing with both implementations**, then `bench/compare.py`, which writes `bench/results/compare-llmfree.md` with speed, tree size, evidence localizability and per-doc agreement.
2. **Retrieval.** The same Python agent and judge answer the 150 questions twice, once over the Python-built index and once over the Rust-built `.pageindex/` (`retrieve_python.py --index-dir`). The on-disk format is shared, so only the index builder differs.
3. **`--edgar`.** Fetches the latest 10-K for each ticker in `bench/tickers_30.txt` (Dow 30 by default; edit freely), renders them to PDF with Chromium, and indexes them with both implementations (LLM-free metrics only).
