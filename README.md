# PageIndex-rust

A Rust reimplementation of [VectifyAI/PageIndex](https://github.com/VectifyAI/PageIndex) ("vectorless RAG"). It turns a PDF into a tree of sections (title, page span, summary) that an LLM agent navigates to answer questions with page citations. The port targets **output parity** with the Python reference at commit `619cbd8`, and adds page triage and OCR for scanned filings.

## Status

| Stage | Crate | Parity vs Python goldens (18 docs) |
|---|---|---|
| 01 spans (PDFium chars, content streams, font Unicode repair, glyph merge) | `pi-extract` | ✅ diff-clean, 6–12× faster |
| 02 lines, 03 columns, page/doc stats | `pi-layout` | ✅ bit-exact |
| 04 blocks, 05 classification/title/captions | `pi-layout` | in progress |
| 06 heading candidates, 07 outline, 08 tree | `pi-outline` | next |
| 09 embedded bookmarks | `pi-outline` | in progress |
| 10 fallbacks + merge/expand + summaries | `pi-optimize`, `pi-llm`, `pi-summary` | ✅ merge clean; LLM passes clean under replay |
| Store (`.pageindex/`, Python-SDK compatible), MCP tools | `pi-store`, `pi-mcp` | in progress |
| Page triage + OCR (OpenAI-compatible vision endpoint) | `pi-triage`, `pi-ocr` | ✅ (tested with a mock endpoint) |

Details and every known deviation: `parity/KNOWN_DIFFS.md`. Design notes: `docs/spikes/`.

## Layout

```
crates/
  pi-core       Span / PageSpans / Rect: the producer→layout contract
  pi-pycompat   CPython-exact helpers: difflib, round, list.sort, str whitespace, Unicode 14 tables
  pi-data       reference data tables (dictionaries, glyph names, …)
  pi-extract    text-layer span producer (PDFium via pdfium-render raw bindings + lopdf)
  pi-triage     per-page labels: text | scanned | garbled | mixed | graphic
  pi-ocr        OCR producer: render → OpenAI-compatible vision model → spans, tables, title hints
  pi-layout     lines, columns, blocks, classification
  pi-outline    headings, outline assembly, tree, embedded bookmarks
  pi-optimize   tree merge / expand (LLM) / relabel
  pi-llm        OpenAI-compatible client + record/replay/stub backends
  pi-summary    bottom-up summaries and document description
  pi-store      .pageindex/ on-disk format (readable by the Python SDK)
  pi-storage    local / S3 / GCS ingest and JSON mirror
  pi-mcp        retrieval tools over MCP
  pi-config     TOML + env configuration
  pi-cli        `pageindex-rs` (index, serve-mcp, diff, …)
parity/         reference dump scripts, corpus manifest, known diffs (goldens are generated locally)
bench/          Python-vs-Rust benchmark kit (FinanceBench, EDGAR)
```

## Requirements

- Rust 1.94 (`rust-toolchain.toml`).
- **PDFium build 7999**, the same build pypdfium2 5.13 bundles: `bblanchon/pdfium-binaries` release `chromium/7999`. Point `PDFIUM_LIB` at `libpdfium.so`/`.dylib`, or put it in `./pdfium/lib/`.
- For parity work: `parity/fetch_reference.sh`, which clones the reference at `619cbd8` into a pinned venv.

## Parity workflow

```bash
parity/fetch_reference.sh                                                   # reference + venv
parity/.venv/bin/python parity/dump_reference.py --corpus parity/corpus.toml --verify   # goldens
parity/check_stage01.sh                                                     # Rust stage 01 vs goldens
cargo test -p pi-layout --test parity                                       # stages 02-03
cargo test -p pi-optimize --test golden_merge                               # stage 10
target/release/pageindex-rs diff --stage N --a parity/golden --b <rust-out> # any stage
```

## Benchmarks

See `bench/README.md`. `bench/run_all.sh` runs the Python reference and pageindex-rs side by side on FinanceBench (32 companies, 84 SEC filings, 150 questions), and optionally on EDGAR filings for your own ticker list.

## License

MIT. Ported logic and bundled data from PageIndex are under its MIT license; see `NOTICE`.
