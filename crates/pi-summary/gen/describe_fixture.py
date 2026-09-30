"""Record the prompt `generate_doc_description` sends for a sample tree (CPython repr of the
cleaned structure inside the f-string). Output: crates/pi-summary/tests/fixtures/describe.json

  /home/user/PageIndex-rust/parity/.venv/bin/python crates/pi-summary/gen/describe_fixture.py \
      > crates/pi-summary/tests/fixtures/describe.json
"""
import json

import pageindex.utils as U

structure = [
    {"title": "Preface", "node_id": "0000", "start_index": 1, "end_index": 1, "summary": "It's \"quoted\"\n"},
    {"title": "Ché 1 — 日本", "node_id": "0001", "start_index": 2, "end_index": 9,
     "key_items": ["x"], "nodes": [
         {"title": "1.1\ttab", "node_id": "0002", "start_index": 2, "end_index": 3, "summary": "a\\b ​"},
         {"title": "1.2", "node_id": "0003", "start_index": 4, "end_index": 9, "summary": "", "nodes": []},
     ], "summary": "parent"},
]
captured = {}


def fake(model, prompt, chat_history=None, return_finish_reason=False):
    captured["prompt"] = prompt
    return "desc"


U.llm_completion = fake
clean = U.create_clean_structure_for_description(structure)
U.generate_doc_description(clean, model="m")
print(json.dumps({"structure": structure, "clean": clean, "prompt": captured["prompt"]}, ensure_ascii=False, indent=1))
