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

## Open
(none yet)
