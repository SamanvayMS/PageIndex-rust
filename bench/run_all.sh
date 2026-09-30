#!/usr/bin/env bash
# Laptop benchmark: Python reference vs pageindex-rs on FinanceBench (and optionally EDGAR).
#
#   export PDFIUM_LIB=/path/to/pdfium-7999/lib/libpdfium.{so,dylib}   # bblanchon chromium/7999
#   export PI_LLM_BASE_URL=... PI_LLM_MODEL=... PI_LLM_KEY=...        # OpenAI-compatible (retrieval)
#   export SEC_USER_AGENT="Name you@example.com"                      # optional, EDGAR
#   bench/run_all.sh [--edgar] [--no-retrieval]
set -euo pipefail
cd "$(dirname "$0")/.."
EDGAR=0; RETRIEVAL=1
for a in "$@"; do
  case "$a" in --edgar) EDGAR=1 ;; --no-retrieval) RETRIEVAL=0 ;; esac
done
PY=parity/.venv/bin/python
[ -x "$PY" ] || parity/fetch_reference.sh
cargo build --release -p pi-cli
$PY bench/fetch_financebench.py

# 1. LLM-free indexing, both implementations
$PY bench/index_python.py --run py-llmfree
$PY bench/index_rust.py --run rust-llmfree
$PY bench/compare.py --python py-llmfree --rust rust-llmfree --out compare-llmfree

# 2. Retrieval: same Python agent + judge, over the Python-built and the Rust-built index
if [ "$RETRIEVAL" = 1 ] && [ -n "${PI_LLM_KEY:-}" ]; then
  $PY bench/retrieve_python.py --run py-retrieval
  $PY bench/index_rust.py --run rust-llm --extra --summary
  $PY bench/retrieve_python.py --run rust-retrieval --index-dir bench/results/rust-llm/.pageindex
  $PY bench/compare.py --python py-llmfree --rust rust-llmfree \
      --python-retrieval py-retrieval --rust-retrieval rust-retrieval --out compare-retrieval
fi

# 3. Optional: EDGAR filings for your ticker list (LLM-free metrics only)
if [ "$EDGAR" = 1 ]; then
  $PY -m pip install -q playwright && $PY -m playwright install chromium
  $PY bench/fetch_edgar.py --tickers bench/tickers_30.txt --forms 10-K --per-form 1
  $PY bench/index_python.py --run py-edgar --pdf-dir bench/data/edgar
  $PY bench/index_rust.py --run rust-edgar --pdf-dir bench/data/edgar
  $PY bench/compare.py --python py-edgar --rust rust-edgar --out compare-edgar
fi
ls -1 bench/results/*.md
