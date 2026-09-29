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
