# Known differences vs the Python reference (@619cbd8)

Each entry lists the stage, the scope, the reason and who reviewed it. An entry is required whenever a ported stage is not diff-clean.

## Reference defects to port as-is behind a flag
- `outline_assembly/style_context.py::compare_heading_depth`: the docstring's sign convention is inverted relative to the code. Port the code (`+1` = first argument is shallower).
- `page_index_classic.py::calculate_page_offset` returns `None` when no titles match, and the caller then raises `TypeError`.
- The classic indexer's chunk size is hardcoded at 20,000 tokens.
- `local_api.py` builds the tree from pdfium text but stores PyPDF2 text in `pages.json`. The Rust version stores the pipeline's own text (intentional divergence at the store level only).

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
