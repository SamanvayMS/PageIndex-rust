# Known differences vs the Python reference (@619cbd8)

Each entry lists the stage, the scope, the reason and who reviewed it. An entry is required whenever a ported stage is not diff-clean.

## Reference defects to port as-is behind a flag
- `outline_assembly/style_context.py::compare_heading_depth`: the docstring's sign convention is inverted relative to the code. Port the code (`+1` = first argument is shallower).
- `page_index_classic.py::calculate_page_offset` returns `None` when no titles match, and the caller then raises `TypeError`.
- The classic indexer's chunk size is hardcoded at 20,000 tokens.
- `local_api.py` builds the tree from pdfium text but stores PyPDF2 text in `pages.json`. The Rust version stores the pipeline's own text (intentional divergence at the store level only).

## Stage 10: optimize / LLM / summaries (`pi-optimize`, `pi-llm`, `pi-summary`)
Parity status: `10_tree_optimized` matches for 18/18 golden docs (`pi-optimize/tests/golden_merge.rs`). With `StubLlm`, `10b_tree_full_llm` from `dump_reference.py --llm stub` matches for earthmover, four-lectures and q1-fy25-earnings. With a replay of the prompt-dependent fake (`pi-summary/gen/fake_llm_golden.py`), output matches for those three plus fb_adobe_2017_10k, including expand. Every fixture key is hit, no prompt misses, and the result is the same at concurrency 1, 3 and the default.

Intentional divergences. None of them affects a golden:
- **Transport.** `pi-llm` speaks the OpenAI `/chat/completions` wire format directly instead of going through LiteLLM. It strips the `litellm/` and `openai/` prefixes as LiteLLM does and sends any other `provider/` prefix unchanged, so non-OpenAI providers need an OpenAI-compatible gateway. It keeps the retry ladder: 10 attempts, 1 s apart, with no retry on 400/401/403/404. A `null` content becomes `""`, which every reference consumer treats like `None`. The per-role concurrency semaphore is an extra cap that the reference does not have.
- **Token counting.** `utils.count_tokens` goes through `litellm.token_counter`. The port uses tiktoken `cl100k_base` or `o200k_base`, chunked by 1024 code points, with extrapolation above 4M chars. It picks the encoding from a snapshot of litellm's OpenAI chat-model list (`pi-llm/src/tokens.rs`). litellm's HuggingFace tokenizers for Claude, Llama and Cohere model names are not ported, and those names count with `cl100k_base`. This only moves the 200-token raw-text threshold for leaves when such a model is the summary model.
- **Reply JSON.** Model replies are parsed with serde_json, not Python's `json.loads`. `NaN`/`Infinity` literals and lone-surrogate `\ud800` escapes are rejected: Python would accept them. Integers beyond i64/u64 lose precision. This affects only malformed replies. A rejected reply takes the same path as any other unparsable reply: an expand error/retry, or the raw-reply fallback in `parse_summary`.
- **Expand page type.** Expand accepts `"page": true` as page 1, since `isinstance(True, int)` holds in Python, but it stores the child's `start_index` as the integer `1` where Python would store `True`.
- **`repr()` in `generate_doc_description`.** `repr()` of the structure uses the `unicode-general-category` table, which is newer than CPython 3.11's Unicode 14. Code points assigned after 14.0 print raw where CPython would escape them.
- **Not ported.** Some of `tree_optimize.py` is not ported, because the flash pipeline never reaches it:
  - the CLI (`main`, `load_pages`, `report_costs`, `print_metrics`)
  - the cached per-page heading source (`children_from_cache`, `load_headings_cache`; the flash pipeline passes `cache=None`)
  - `validate`/`new_issues`, which are not part of the `optimize` report
  - most per-event log fields, which are reduced to what the report counts
- **`_same_page` in stage 10.** `pi_optimize::postprocess_merge` leaves `_same_page` on nodes, as `dump_reference.py::optimized` (and so the `10_tree_optimized` golden) does. `page_index_flash` strips the key. `pi_summary::page_index_flash_post` reproduces `page_index_flash` and strips it.

## Store and agent tools (`pi-store`, `pi-mcp`)
All intentional. None changes what the golden matrix (`crates/pi-mcp/tests/fixtures`, 156 calls) or the interop tests (`crates/pi-store/tests/python_interop.rs`) check. Reviewed by: nobody yet (written by the porting agent).
- `local_store.py::list_metas` iterates a `set` of directory names, so the key order of a manifest it rewrites after drift depends on hash order. Rust iterates them sorted. Reads are unaffected, and `save_document` keeps the reference's insertion order, so after the same commits the manifest bytes match.
- Reading JSON: serde_json rejects `NaN`/`Infinity` literals and lone-surrogate escapes that Python's `json.load` accepts. Such a file reads as absent. Integers beyond the u64 range become floats. Rust strings cannot carry lone surrogates, so the reference's `errors="replace"` write path has no counterpart.
- An `OSError` that escapes into a tool envelope (`remove_document` `"failed"` entries, `INTERNAL_ERROR` `"<tool> failed: ..."`) carries Rust's `io::Error` text instead of Python's `[Errno N] ...`.
- A non-string `markdown` in `pages.json` reads as `""` in `get_tree(include_text)` and `get_ocr("node"/"raw")`. Python raises `TypeError` there. `get_page_content` matches the reference.
- `naming.sanitize_filename`'s NFKC comes from the `unicode-normalization` crate (a newer Unicode than Python 3.11's 14.0). It differs only for code points changed after 14.0.
- `_normalize_created_at` ports CPython 3.11's pure-Python `datetime.fromisoformat` algorithm. The C implementation also accepts a few extra spellings (e.g. whitespace before the UTC offset); Rust returns those strings raw. A UTC conversion that leaves the year range 1..9999 also returns the string raw (Python: `OverflowError` → `INTERNAL_ERROR`).
- Page numbers in page specs are `u128` (saturating), and `page_index` keys are 64-bit, where Python ints are unbounded. They differ only past 38 digits or 2^64.
- `str()`/`repr()` of a non-string `doc_name` (quoted back in NOT_FOUND envelopes) approximates `str.isprintable` for non-Latin-1 characters.
- MCP server: `remove_document` is gated at registration, as in the SDK's in-process server. Calling a tool that is not registered returns the unknown-tool envelope listing the registered tools; the SDK's server rejects it inside the Claude SDK instead. `initialize` carries `AGENT_INSTRUCTIONS` as the server `instructions`. The SDK delivers them only through the system prompt.

## Stage 09: embedded bookmarks (`pi-outline::embedded_toc`)
Parity status: `09_tree_bookmarks` is exact (key order included) for 18/18 golden docs. Six of them take the FULL path ("bookmarks") and 12 the IGNORE path ("detected").
- The SKELETON ("hybrid") path is covered by three FinanceBench docs dumped with `dump_reference.py`:
  - AMCOR_2023Q4_EARNINGS and PEPSICO_2023Q1_EARNINGS are committed as derived fixtures.
  - NIKE_2023_10K runs through env `PI_EXTRA_GOLDEN`.
- `read_bookmarks`/`validate_bookmarks`/`classify_bookmarks` match pypdfium2 5.13 on all 97 local PDFs (count + sha256 of entries, tier): 78 IGNORE, 3 SKELETON, 16 FULL.

Notes:
- **PDF loading and API scope.** A path is read into memory and opened with `FPDF_LoadMemDocument64`; pypdfium2 uses `FPDF_LoadDocument` on the path. The document is parsed the same way either way. The pypdfium2 v4 `item.page_index` fallback is not ported, since the reference venv pins 5.13.
- **PDFium lock.** PDFium is not thread-safe, so `read_bookmarks` serializes its own PDFium use with a process-wide mutex. Concurrent `pi-extract` use from other threads is not covered by that lock. A single shared PDFium lock belongs in `pi_extract::pdfium`.

## End-to-end indexing (`pi-index`, `pi-cli`, `pi-py`)
Temporary until stages 04-08 land in the `pi_index::detect_structure` seam. Reviewed by: nobody yet.
- `detect_structure` returns an empty structure, so every document without trustworthy bookmarks gets the `_page_nodes` fallback (`toc_source = "pages"`).
- `page_texts` (and so `pages.json` and the text seen by expand and summaries) are the layout lines joined with `\n`. The reference joins stage-04 block texts.
- `pageindex-rs index` still writes a refused result (flat tree over 10 pages, unreadable, empty) to `--tree-out`. In the report it is marked `rejected` (not `error`, so `bench/index_rust.py` keeps the tree) and it is not committed. The SDK raises instead. `--accept-flat` (Python: `accept_flat=True`) commits flat page trees anyway.
- `--ocr auto` (triage + OCR through `[ocr]`) is Rust-only. The reference has no OCR. With OCR on, doc.json `metadata` carries `{"page_labels": {...}, "ocr_pages": n}`; with it off, `metadata` is null as in the SDK.
- `pageindex-rs index` exits non-zero only when every input failed; per-document failures go to the report as `error`.

## Stages 06-08: heading candidates, outline assembly, dict tree (`pi-outline`)
Parity status: `06_candidates`, `07_outline` (assembled, gate, final, `has_abstract_or_references`) and `08_tree_raw` are bit-exact (`PI_PARITY_TOL=0`) on all 18 golden docs, running the Rust pipeline from `01_spans` (`cargo test -p pi-outline --test parity`). The test also checks the stage-05 section openers against `05_classified`. `PI_OUTLINE_INJECT=1` runs stages 06-08 on stage-05 state taken from the goldens instead. That mode needs the `05a_pre_openers.json` stage file that `dump_reference.py` now writes (the block state `find_section_openers` sees), and is also clean on all 18 docs. Without that file, the pre-opener state is guessed, and fb_3m_2018_10k diverges (a block the openers pass retyped 12 cannot be recovered).

Notes:
- **`compare_heading_depth` sign.** Ported from the code: `+1` means the first candidate is shallower (see "Reference defects").
- **`SortedKeyList`.** `StyleCluster` and the per-font trees are sorted `Vec`s bisected with Python tuple `<`. The reference inserts only keys that are not already present, so this reproduces its order for any strict weak order. A NaN in a key (a NaN font size) could order differently from `sortedcontainers`' two-level bisect. The corpus has no NaN keys.
- **`NUMBERED_PREFIX_RE` `\p{Lu}`.** `\p{Number}` uses the `regex`-module table in `pi-pycompat`. `\p{Lu}` uses the Unicode 14 general category, where the installed `regex` module uses its own, newer Unicode version. Uppercase letters added after Unicode 14 can therefore differ in `is_heading_continuation`.
- **Unguarded reference dereferences.** In `scan_page_headings` the reference reads `above.line_count()` / `predecessor.line_count()` without a `None` check (sibling-continuation tests), and in `detect_heading_with_body` it tokenizes a missing above-neighbor. Python raises there, and the port panics with a message instead. Neither case occurs in the corpus.
- **Duplicated opener code.** `find_section_openers` and the detectors it needs exist twice: a subset in `pi_layout::heading_detection` (what `classify_document` runs) and the full stage-06 set in `pi_outline::heading_detection`. `pi_outline::pipeline::openers_from_layout` converts the former's nodes. Both produce identical openers on the corpus. The pi-layout copy should eventually be replaced by the pi-outline one, which needs the stage-05 orchestration split around `find_section_openers`, or pi-layout reusing pi-outline's detectors.

## Open
(none yet)

## Stage 01 (pi-extract): diff-clean on all 18 corpus docs
- PDF objects are read with `lopdf` instead of PyPDF2. lopdf stores reals as `f32`, so `pi-extract` renders them from the f32's shortest round-trip decimal. That is exact for tokens with <= 7 significant digits, which covers every box value in the corpus. A token with more digits (e.g. `595.27559055`) would differ from Python's `float(token)` by < 1e-4 pt.
- Unicode categories, bidi classes and normalization use the Unicode 14 tables generated from Python 3.11 (`pi_pycompat::unicode`).
- PyPDF2 load failures (the reference then runs without content-stream repair) and lopdf load failures do not necessarily coincide on malformed PDFs.

## Stage status
- **02_lines / 03_columns** (`pi-layout::process_page`) and **04 `doc_stats`** (`compute_doc_stats`): bit-exact on all 18 golden docs (`cargo test -p pi-layout --test parity`, also with `PI_PARITY_TOL=0`).
- **04_blocks** (`phases::build_document`: blocks, reading order, doc stats) and **05_classified** (`phases::classify_document`: header/footer, watermarks, TOC/boilerplate, body paragraphs, title + title echoes, captions, section openers, caption regions): bit-exact on all 18 golden docs (same test).
- Version pins that parity depends on: general category / case tables are Unicode 14 (Python 3.11); `\p{Number}` in `model/numbering.py` follows the `regex` module installed in the parity venv (2026.9.29), captured in `pi-pycompat/src/unicode_tables.rs`. Regenerate with `crates/pi-pycompat/gen/unicode_tables.py` if either changes.
