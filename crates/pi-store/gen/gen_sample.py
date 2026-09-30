"""Build crates/pi-store/tests/fixtures/sample.json from Python-built index trees.

    python crates/pi-store/gen/gen_sample.py TREES_DIR

TREES_DIR holds `<doc>.merged.json` / `<doc>.pages.json` (e.g.
bench/results/llmfree-619cbd8/trees). Page texts are truncated to keep the fixture small.
A few nodes get summaries, key items and `text` so every branch of `_format_tree_node`
and `remove_fields` is exercised.
"""
import json
import sys
from pathlib import Path

TREES = Path(sys.argv[1])
OUT = Path(__file__).resolve().parents[1] / "tests" / "fixtures" / "sample.json"


def load(doc, limit):
    tree = json.loads((TREES / f"{doc}.merged.json").read_text())["structure"]
    pages = [p[:limit] for p in json.loads((TREES / f"{doc}.pages.json").read_text())]
    return tree, pages


def decorate(nodes, depth=0):
    for i, node in enumerate(nodes):
        if i % 2 == 0:
            node["summary"] = f"Summary of {node['title']} (depth {depth})"
        if i % 3 == 0:
            node["text"] = "node text that must not be stored"
        decorate(node.get("nodes") or [], depth + 1)


mmm_tree, mmm_pages = load("3M_2018_10K", 80)
decorate(mmm_tree)
amcor_tree, amcor_pages = load("AMCOR_2022_8K_dated-2022-07-01", 150)
sample = {"docs": [
    {"name": "3M_2018_10K.pdf", "description": "3M 2018 annual report — “Form 10-K”",
     "metadata": {"ticker": "MMM", "year": 2018, "score": 0.25, "note": "é "},
     "tree": mmm_tree, "pages": mmm_pages},
    {"name": "AMCOR_2022_8K.pdf", "description": None, "metadata": None,
     "tree": amcor_tree, "pages": amcor_pages},
    {"name": "3M_2018_10K.pdf", "description": "duplicate upload", "metadata": {},
     "tree": amcor_tree, "pages": amcor_pages},
]}
OUT.parent.mkdir(parents=True, exist_ok=True)
OUT.write_text(json.dumps(sample, ensure_ascii=False, separators=(",", ":")) + "\n",
               encoding="utf-8")
print(OUT)
