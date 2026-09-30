"""Record the CPython values `pi-llm`'s unit tests compare against.

Run with the reference venv (it has litellm + tiktoken):
    /home/user/PageIndex-rust/parity/.venv/bin/python crates/pi-llm/gen/llm_fixture.py \
        > crates/pi-llm/tests/fixtures/llm.json
"""
import json
import os
import sys
from pathlib import Path

os.environ.setdefault("LITELLM_LOCAL_MODEL_COST_MAP", "True")
ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "parity"))
import mock_llm  # noqa: E402

CASES = [
    "plain",
    'quote " and back\\slash / slash',
    "nl\ntab\tcr\r bs\x08 ff\x0c nul\x00 us\x1f del\x7f",
    "uni é 日本語     \x85 ﻿ \U0001F600",
    '</script>{"a":[1,2]}',
    "",
]

keys = []
for c in CASES:
    keys.append({"messages": [{"role": "user", "content": c}],
                 "key": mock_llm.key_of([{"role": "user", "content": c}])})
multi = [{"role": "system", "content": "sys"}, {"role": "user", "content": "ué"}]
keys.append({"messages": multi, "key": mock_llm.key_of(multi)})

record_line = json.dumps({"key": "k", "model": "m", "messages": multi, "reply": 'ré\n"x"'},
                         ensure_ascii=False)

from pageindex.utils import count_tokens  # noqa: E402

TOKEN_TEXTS = [
    "",
    "Hello wörld — this is a test 日本語 text.\n\nNew para",
    "<|endoftext|> special tokens stay ordinary text <|fim_prefix|>",
    ("The quick brown fox jumps over the lazy dog. " * 60) + "éè" * 300,
    "   \n\n\t  spaces   and　ideographic　space  ",
    "x" * 3000,
]
tokens = []
for t in TOKEN_TEXTS:
    for model in (None, "openai/mock", "gpt-5.6-luna", "gpt-4o-mini", "gpt-3.5-turbo", "some-other"):
        tokens.append({"text": t, "model": model, "count": count_tokens(t, model=model)})

print(json.dumps({"keys": keys, "record_line": record_line, "tokens": tokens},
                 ensure_ascii=False, indent=1))
