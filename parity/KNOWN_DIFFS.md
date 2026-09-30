# Known differences vs the Python reference (@619cbd8)

Each entry lists the stage, the scope, the reason and who reviewed it. An entry is required whenever a ported stage is not diff-clean.

## Reference defects to port as-is behind a flag
- `outline_assembly/style_context.py::compare_heading_depth`: the docstring's sign convention is inverted relative to the code. Port the code (`+1` = first argument is shallower).
- `page_index_classic.py::calculate_page_offset` returns `None` when no titles match, and the caller then raises `TypeError`.
- The classic indexer's chunk size is hardcoded at 20,000 tokens.
- `local_api.py` builds the tree from pdfium text but stores PyPDF2 text in `pages.json`. The Rust version stores the pipeline's own text (intentional divergence at the store level only).

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

## Open
(none yet)
