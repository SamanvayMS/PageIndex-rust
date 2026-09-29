# Python PageIndex baseline: `llmfree-619cbd8`

## Indexing (LLM-free flash, serial extraction, 3 docs concurrently on 4 cores)

| metric | value |
|---|---|
| docs ok / total | 84 / 84 |
| pages | 12013 |
| extraction total | 2204.7 s |
| ms/page (pooled) | 183.5 |
| ms/page per doc p50 / p95 | 125.7 / 366.7 |
| merge pass total | 1.09 s |
| toc_source | {'detected': 70, 'bookmarks': 11, 'hybrid': 3} |
| nodes raw / merged (mean) | 170.9 / 86.9 |
| depth merged (mean, max) | 3.10, 6 |
| leaf span pages merged (mean of doc means) | 3.21 |
| docs with empty-text pages | 15 |

## Evidence localizability (merged tree, no LLM)

Smallest tree node containing each gold evidence page: how many pages an agent must read once it picks the right node.

| metric | value |
|---|---|
| evidence pages | 189 |
| not covered by any node | 0 |
| span p50 / p90 / max | 5 / 131 / 176 |
| span <= 3 pages | 45.5% |
| span <= 10 pages | 60.8% |

### Slowest docs (ms/page)

- `3M_2018_10K`: 473.9 ms/p, 160 p
- `3M_2023Q2_10Q`: 462.4 ms/p, 92 p
- `3M_2022_10K`: 432.8 ms/p, 252 p
- `COCACOLA_2017_10K`: 391.0 ms/p, 197 p
- `CVSHEALTH_2022_10K`: 366.7 ms/p, 213 p

## Findings

1. **Speed.** 183 ms/page pooled, and 3M, Coca-Cola and CVS 10-Ks reach 370–470 ms/page. That is 2–5× the brief's 80–90 ms/page, measured on a 4-core container with serial extraction. Character extraction (`_page_pass1`: pdfium char walk, content-stream tokenizer, text-object lookup) takes ~75% of the time (3M 10-K cProfile). This is the Rust port's main target. The spawn process pool was *slower* than serial on this box (72 s vs 58 s on the 3M 10-K).
2. **Tree quality on 10-Ks is the retrieval risk.** 72 of 189 gold evidence pages (38%) sit only inside a node spanning more than 20 pages. Flash frequently misses the `PART`/`Item N.` skeleton of a 10-K, so the financial statements fall into a giant `Preface` (the gap before the first detected heading) or into `Documents Incorporated by Reference`. The worst cases are Boeing, PepsiCo, AMD, AmEx and 3M. The reference's LLM `expand` step (optimize="full") is meant to split such nodes, and the LLM run will measure how much it recovers. A 10-K-aware heading detector (Item/Part patterns) is a candidate *post-parity* improvement, logged as a deliberate divergence.
3. **Bookmarks.** Only 14 of 84 filings use embedded bookmarks (11 full, 3 hybrid). The other 70 rely on detected layout.
4. **Empty-text pages.** 15 filings have pages with no extractable text. Walmart 2019 has 29 of 155, and those are OCR-routing candidates (Track B triage).

## Pending
- Retrieval plus LLM judge over the 150 questions (`retrieve_python.py`), which needs `PI_LLM_*` env vars.
