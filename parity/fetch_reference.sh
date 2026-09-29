#!/usr/bin/env bash
# Clone the pinned PageIndex reference into parity/.ref and create a venv with pinned deps.
set -euo pipefail
REF_SHA="${REF_SHA:-619cbd8}"
HERE="$(cd "$(dirname "$0")" && pwd)"
REF="$HERE/.ref"
if [ ! -d "$REF/.git" ]; then
  git clone -q https://github.com/VectifyAI/PageIndex.git "$REF"
fi
git -C "$REF" fetch -q origin
git -C "$REF" checkout -q "$REF_SHA"
if [ ! -x "$HERE/.venv/bin/python" ]; then
  python3 -m venv "$HERE/.venv"
fi
"$HERE/.venv/bin/pip" install -q -r "$HERE/requirements.txt"
"$HERE/.venv/bin/pip" install -q -e "$REF"
echo "reference at $(git -C "$REF" rev-parse --short HEAD); venv at $HERE/.venv"
