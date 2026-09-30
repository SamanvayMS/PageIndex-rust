#!/usr/bin/env bash
# Rust stage-01 parity over the corpus: dump spans with pi-extract, diff against parity/golden.
# Needs PDFIUM_LIB pointing at PDFium build 7999 (see docs/spikes/A-pdfium.md).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$HERE/.."
OUT="${OUT:-$ROOT/target/rust-golden}"
cargo build -q --release -p pi-extract --example dump_spans -p pi-cli
python3 -c "
import tomllib
for d in tomllib.load(open('$HERE/corpus.toml','rb'))['doc']: print(d['id'] + '\t' + d['path'])
" | while IFS=$'\t' read -r id path; do
  printf '%-45s ' "$id"
  "$ROOT/target/release/examples/dump_spans" "$HERE/$path" "$OUT/$id" 2>&1 | tail -1
done
"$ROOT/target/release/pageindex-rs" diff --stage 1 --a "$HERE/golden" --b "$OUT" "$@"
