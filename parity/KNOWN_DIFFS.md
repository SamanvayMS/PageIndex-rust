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
- Unicode categories (`Mn`/`Cf` checks, `_char_category`) and bidi classes currently come from the Rust Unicode crates (Unicode 15+) instead of Python 3.11's Unicode 14 tables. This only matters for code points assigned after Unicode 14 and will be switched to the generated tables (Spike D).
- PyPDF2 load failures (the reference then runs without content-stream repair) and lopdf load failures do not necessarily coincide on malformed PDFs.
