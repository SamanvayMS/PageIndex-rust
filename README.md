# PageIndex-rust

A Rust reimplementation of [VectifyAI/PageIndex](https://github.com/VectifyAI/PageIndex) ("vectorless RAG"). It turns a PDF into a tree of sections (title, page span, summary) that an LLM agent navigates to answer questions with page citations. The port targets **output parity** with the Python reference at commit `619cbd8`, and adds page triage and OCR for scanned filings.

## Status

The whole Flash pipeline is ported. **End to end, from PDF to `page_index_flash(optimize="merge")` result, the output is identical to the Python reference on all 18 corpus docs**, at about 12 ms/page single-process. That covers PRML, the 2023 annual report, the Regulation Best Interest releases, CJK/Arabic/Hindi fixtures, and FinanceBench 10-Ks for 3M, AMD, Adobe, Walmart and PepsiCo.

| Stage | Crate | Parity vs Python goldens (18 docs) |
|---|---|---|
| 01 spans (PDFium chars, content streams, font Unicode repair, glyph merge) | `pi-extract` | ✅ identical; 6–12× faster |
| 02 lines, 03 columns, 04 blocks, 05 classification/title/captions | `pi-layout` | ✅ bit-exact |
| 06 heading candidates, 07 outline, 08 tree, 09 embedded bookmarks | `pi-outline` | ✅ bit-exact (+3 hybrid-bookmark docs; 97/97 bookmark reads) |
| 10 fallbacks + merge/expand + summaries | `pi-optimize`, `pi-llm`, `pi-summary` | ✅ merge identical; LLM passes identical under record/replay |
| Store (`.pageindex/`, Python-SDK compatible), MCP tools | `pi-store`, `pi-mcp` | ✅ 156/156 tool calls byte-identical; the Python SDK reads Rust-built stores |
| Orchestrator, CLI, Python bindings | `pi-index`, `pi-cli`, `pi-py` | ✅ |
| Page triage + OCR (OpenAI-compatible endpoint: generic vision model or PaddleOCR-VL) | `pi-triage`, `pi-ocr` | ✅ (tested with mock endpoints; real-model calibration is `ocr_calibrate`) |

Details and every known deviation: `parity/KNOWN_DIFFS.md`. Design notes: `docs/spikes/`.

## Quick start

```bash
export PDFIUM_LIB=/path/to/pdfium-7999/lib/libpdfium.so
cargo build --release -p pi-cli
target/release/pageindex-rs index --no-summary --optimize merge --storage .pageindex report.pdf
target/release/pageindex-rs serve-mcp --storage .pageindex          # MCP tools over stdio
# with summaries / expand (OpenAI-compatible endpoint):
PI_LLM_BASE_URL=... PI_LLM_MODEL=... PI_LLM_KEY=... target/release/pageindex-rs index --summary report.pdf
# scanned documents: add an [ocr] endpoint in pageindex.toml (or PI_OCR_*) and pass --ocr auto
```

### OCR with PaddleOCR-VL
Text-layer pages are read directly. Triage (in Rust, no pypdf) sends scanned, garbled and image-heavy pages to OCR. To use PaddleOCR-VL served by vLLM (or PaddleOCR's genai server) as the OCR engine:
```toml
# pageindex.toml
[ocr]
profile = "paddleocr-vl"            # default "spans-json" = generic vision LLM with a JSON prompt
base_url = "http://dgx:8118/v1"
model = "PaddleOCR-VL-1.6-0.9B"
tables = true                       # second pass: "Table Recognition:" on detected table regions -> markdown
```
The engine sends `skip_special_tokens: false` so vLLM keeps the `<|LOC_n|>` box tokens. Check your server once with
`cargo run --release -p pi-ocr --example ocr_calibrate -- --profile paddleocr-vl --dump-raw some.pdf`.
Details: `docs/spikes/E-ocr-endpoint.md`.

Python: `cd crates/pi-py && maturin develop --release`, then `import pageindex_rs; pageindex_rs.index("report.pdf")`.

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
cargo test --release -p pi-outline --test parity                            # stages 06-08 (+ 05 openers)
cargo test -p pi-optimize --test golden_merge                               # stage 10
target/release/pageindex-rs diff --stage N --a parity/golden --b <rust-out> # any stage
```

## Benchmarks

See `bench/README.md`. `bench/run_all.sh` runs the Python reference and pageindex-rs side by side on FinanceBench (32 companies, 84 SEC filings, 150 questions), and optionally on EDGAR filings for your own ticker list.

## License

MIT. Ported logic and bundled data from PageIndex are under its MIT license; see `NOTICE`.
