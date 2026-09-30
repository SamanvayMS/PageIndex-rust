"""Generate the pi-mcp golden fixtures from the Python reference (@619cbd8).

Run with the parity venv (no LLM, no network):

    parity/.venv/bin/python crates/pi-mcp/gen/gen_fixtures.py TREES_DIR

TREES_DIR holds Python-built `<doc>.merged.json` / `<doc>.pages.json` pairs (default:
bench/results/llmfree-619cbd8/trees). Writes, under crates/pi-mcp/tests/fixtures/:

* stores.json  - store specs: literal doc.json metas, trees (literal or a compact
                 recipe), page texts (truncated, or a repeat recipe).
* cases.json   - tool calls and the exact `call_tool` output text + is_error per call,
                 each run against a fresh store built from its spec.
* surface.json - TOOL_CONTRACT, local descriptions/schemas, AGENT_INSTRUCTIONS, prompts.

The Rust test (tests/golden.rs) expands the same recipes, so the expansion rules below
must stay in sync with it.
"""
from __future__ import annotations

import hashlib
import json
import shutil
import sys
import tempfile
from pathlib import Path

from pageindex import PageIndexLocalClient
from pageindex import agent_tools
from pageindex.agent_tools import call_tool
from pageindex.local_chat import CHAT_HEADER
from pageindex.local_store import DocStore

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "tests" / "fixtures"
ROOT = HERE.parents[2]
TREES = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "bench/results/llmfree-619cbd8/trees"

FULL_OUTPUT_LIMIT = 6_000  # longer outputs are stored as sha256 + length + head


# ── recipe expansion (mirrored in tests/golden.rs) ──

def expand_tree(spec):
    if isinstance(spec, dict) and "$flat" in spec:
        p = spec["$flat"]
        return [{
            "title": f"Chapter {i}", "node_id": f"{i:04d}",
            "start_index": i + 1, "end_index": i + 1,
            "summary": "s" * p["summary_len"], "text": "T",
        } for i in range(p["count"])]
    if isinstance(spec, dict) and "$summaries" in spec:
        p = spec["$summaries"]

        def walk(nodes):
            for node in nodes:
                node["summary"] = (node["title"] + ". ") * p["repeat"]
                walk(node.get("nodes") or [])
        tree = json.loads(json.dumps(p["tree"]))
        walk(tree)
        return tree
    return spec


def expand_pages(spec):
    texts = []
    for item in spec:
        if isinstance(item, dict):
            texts.append(item["$repeat"] * item["n"])
        else:
            texts.append(item)
    return [{"page_index": i + 1, "markdown": t} for i, t in enumerate(texts)]


def build_store(path: Path, docs: list) -> None:
    store = DocStore(str(path))
    for doc in docs:
        pages = doc["pages_raw"] if "pages_raw" in doc else expand_pages(doc["pages"])
        store.save_document(doc["meta"]["id"], doc["meta"], expand_tree(doc["tree"]), pages)
        if doc.get("missing_tree"):
            (path / "docs" / doc["meta"]["id"] / "tree.json").unlink()


# ── store specs ──

def meta(doc_id, name, created_at, *, description="A test document", page_num=2,
         metadata=None, status="completed", mode="flash"):
    return {"id": doc_id, "name": name, "description": description, "status": status,
            "createdAt": created_at, "pageNum": page_num, "folderId": None,
            "metadata": metadata, "mode": mode}


def truncated_pages(doc: str, limit: int) -> list[str]:
    pages = json.loads((TREES / f"{doc}.pages.json").read_text())
    return [p[:limit] for p in pages]


def real_tree(doc: str) -> list:
    return json.loads((TREES / f"{doc}.merged.json").read_text())["structure"]


SMALL_TREE = [{
    "title": "Doc", "node_id": "0000", "start_index": 1, "end_index": 2,
    "summary": "root summary", "text": "ROOT TEXT",
    "nodes": [
        {"title": "Intro", "node_id": "0001", "start_index": 1, "end_index": 1,
         "summary": "intro summary", "text": "INTRO TEXT"},
        {"title": "Body", "node_id": "0002", "start_index": 2, "end_index": 2,
         "summary": "body summary", "text": "BODY TEXT", "key_items": ["a", "b"]},
    ],
}]
SMALL_PAGES = ["Page one text about apples", "Page two text about bananas"]


def nested_big_tree():
    children = [{"title": f"Child {i:02d}", "node_id": f"{i + 1:04d}",
                 "start_index": 1, "end_index": 2} for i in range(50)]
    children.insert(7, {"title": "Deep", "node_id": "0900", "start_index": 1, "end_index": 2,
                        "nodes": [{"title": f"Grand {j:02d}", "node_id": f"{j + 1000:04d}",
                                   "start_index": 2, "end_index": 2} for j in range(40)]})
    return [{"title": "Root", "node_id": "0000", "start_index": 1, "end_index": 2,
             "nodes": children},
            {"title": "Tail", "node_id": "0999", "start_index": 2, "end_index": 2}]


def store_specs():
    mmm = real_tree("3M_2018_10K")
    main = [
        {"meta": meta("pi-000a", "3M_2018_10K.pdf", "2026-08-01T10:00:00.123000",
                      description="3M 2018 annual report (Form 10-K)", page_num=160,
                      metadata={"ticker": "MMM", "year": 2018, "ratio": 0.5,
                                "audited": True, "nested": {"x": 1}, "tags": ["a"],
                                "none": None}),
         "tree": mmm, "pages": truncated_pages("3M_2018_10K", 100)},
        {"meta": meta("pi-000b", "AMCOR_2022_8K.pdf", "2026-08-02T09:30:00",
                      description=None, page_num=9),
         "tree": real_tree("AMCOR_2022_8K_dated-2022-07-01"),
         "pages": truncated_pages("AMCOR_2022_8K_dated-2022-07-01", 200)},
        {"meta": meta("pi-000c", "annual-report.pdf", "2026-08-01T10:00:00.123Z"),
         "tree": SMALL_TREE, "pages": SMALL_PAGES},
        {"meta": meta("pi-000d", "same.pdf", "2026-07-01T10:00:00.000000",
                      description="old copy"),
         "tree": SMALL_TREE, "pages": SMALL_PAGES},
        {"meta": meta("pi-000e", "same.pdf", "2026-07-02T10:00:00.000000",
                      description="new copy"),
         "tree": SMALL_TREE, "pages": SMALL_PAGES},
        {"meta": meta("pi-000f", "blanks.pdf", "2026-06-01T00:00:00",
                      page_num=3, description=""),
         "tree": SMALL_TREE,
         "pages": ["", "tab\there \"quoted\" back\\slash \x01\x1f\x7f é 中文 😀  ",
                   "third"]},
        {"meta": meta("pi-0010", "huge.pdf", "2026-06-02T00:00:00", page_num=4),
         "tree": SMALL_TREE,
         "pages": [{"$repeat": '"', "n": 30_000}, {"$repeat": '"', "n": 20_000},
                   {"$repeat": "x", "n": 96_000}, "short"]},
        {"meta": meta("pi-0011", "big.pdf", "2026-06-03T00:00:00", page_num=60),
         "tree": {"$flat": {"count": 60, "summary_len": 4000}},
         "pages": ["x"]},
        {"meta": meta("pi-0012", "nested-big.pdf", "2026-06-04T00:00:00"),
         "tree": {"$summaries": {"tree": nested_big_tree(), "repeat": 300}},
         "pages": SMALL_PAGES},
        {"meta": meta("pi-0013", "JPMORGAN_2022_10K.pdf", "2026-06-05T00:00:00",
                      page_num=382),
         "tree": {"$summaries": {"tree": real_tree("JPMORGAN_2022_10K"), "repeat": 2}},
         "pages": truncated_pages("JPMORGAN_2022_10K", 40)},
        {"meta": meta("pi-0014", "failed.pdf", "2026-05-01T00:00:00", status="failed"),
         "tree": SMALL_TREE, "pages": SMALL_PAGES},
        {"meta": meta("pi-0015", "processing.pdf", "2026-05-02T00:00:00",
                      status="processing"),
         "tree": SMALL_TREE, "pages": SMALL_PAGES},
        {"meta": meta("pi-0016", "notree.pdf", "2026-05-03T00:00:00"),
         "tree": SMALL_TREE, "pages": SMALL_PAGES, "missing_tree": True},
        {"meta": meta("pi-0017", "nopages.pdf", "2026-05-04T00:00:00"),
         "tree": SMALL_TREE, "pages_raw": []},
        {"meta": meta("pi-0018", "weird.pdf", "2026-05-05T00:00:00", page_num=0),
         "tree": [{"title": "Only", "text": "t", "zeta": 1, "page_index": 3, "nodes": []}],
         "pages_raw": [{"page_index": 1, "markdown": "one"},
                       {"page_index": 4, "markdown": None},
                       {"page_index": 1, "markdown": "one again"},
                       {"page_index": 2.0, "markdown": "float"},
                       {"page_index": "3", "markdown": "string"},
                       "not a dict"]},
        {"meta": meta("pi-0019", "bools.pdf", "2026-05-06T00:00:00"),
         "tree": SMALL_TREE,
         "pages_raw": [{"page_index": True, "markdown": "bool one"}]},
    ]
    many = [{"meta": meta(f"pi-1{i:03d}", f"doc{i:02d}.pdf",
                          f"2026-01-{i % 28 + 1:02d}T00:00:00.{i:03d}000",
                          metadata={"i": i} if i % 3 == 0 else None),
             "tree": SMALL_TREE, "pages": SMALL_PAGES} for i in range(57)]
    return {"main": main, "many": many, "empty": []}


# ── call matrix ──

def cases():
    c = []

    def add(store, tool, args, doc_ids=None):
        c.append({"store": store, "tool": tool, "args": args, "doc_ids": doc_ids})

    for args in [{}, {"limit": 2}, {"limit": 2, "offset": 2}, {"limit": "3"}, {"limit": 2.7},
                 {"limit": "abc"}, {"offset": -5, "limit": 1}, {"limit": 0}, {"limit": 100},
                 {"recursive": True}, {"recursive": "false"}, {"sort": "relevance"},
                 {"sort": "banana"}, {"query": "x"}, {"query": ""}, {"folder_id": "f1"},
                 {"folder_id": "root"}, {"folder_id": None}, {"bogus": 1},
                 {"_allowed_ids": ["pi-none"]}, {"offset": 999}, {"offset": "1_0"},
                 {"limit": [1]}, {"limit": True}, {"sort": 5}]:
        add("main", "browse_documents", args)
    add("main", "browse_documents", {}, ["pi-000c", "pi-0010"])
    add("main", "browse_documents", {"limit": 1}, ["pi-000c", "pi-0010"])
    add("main", "browse_documents", {}, [])
    for args in [{}, {"limit": 50}, {"limit": 10, "offset": 20}, {"limit": 10, "offset": 50},
                 {"offset": 57}]:
        add("many", "browse_documents", args)
    add("empty", "browse_documents", {})
    add("empty", "browse_documents", {"offset": 5})

    names = ["3M_2018_10K.pdf", "AMCOR_2022_8K.pdf", "annual-report.pdf", "same.pdf",
             "blanks.pdf", "huge.pdf", "big.pdf", "failed.pdf", "processing.pdf",
             "weird.pdf", "doc05.pdf"]
    for name in names + ["anual-report.pdf", "zzz-nothing.qqq", "3M_2018_10K", "big"]:
        add("main", "get_document", {"doc_name": name})
    for args in [{"doc_name": 123}, {"doc_name": ["x"]}, {"doc_name": "x", "folder_id": "f"},
                 {"doc_name": "same.pdf", "folder_id": "root"}, {},
                 {"doc_name": "same.pdf", "wait_for_completion": "false"},
                 {"doc_name": "same.pdf", "bogus": 1}, {"bogus": 1}]:
        add("main", "get_document", args)
    add("main", "get_document", {"doc_name": "huge.pdf"}, ["pi-000c"])
    add("many", "get_document", {"doc_name": "doc5.pdf"})

    for name in names + ["nested-big.pdf", "JPMORGAN_2022_10K.pdf", "notree.pdf",
                         "nopages.pdf", "bools.pdf", "missing.pdf"]:
        add("main", "get_document_structure", {"doc_name": name})
    for part in [2, 3, 4, 999, 0, -1, "2", "x", 2.9, None]:
        add("main", "get_document_structure", {"doc_name": "big.pdf", "part": part})
    for part in range(2, 8):
        add("main", "get_document_structure", {"doc_name": "nested-big.pdf", "part": part})
        add("main", "get_document_structure", {"doc_name": "JPMORGAN_2022_10K.pdf",
                                               "part": part})

    for pages in ["1-3", "1,99", "99", "5-9", "1,5-9", "1,5,9", "abc", "5-3", "1,,2", "-3",
                  "", "0", "1-3, 7", "1-100000", 5, "1_0", "160", "150-165",
                  "99999999999999999999", "1-6000,5001-10001", " 1", "1\n", ["1"]]:
        add("main", "get_page_content", {"doc_name": "3M_2018_10K.pdf", "pages": pages})
    for name, pages in [("huge.pdf", "1-2"), ("huge.pdf", "1-4"), ("huge.pdf", "1-2,99"),
                        ("huge.pdf", "3-4"), ("huge.pdf", "2,4"), ("blanks.pdf", "1-3"),
                        ("weird.pdf", "1-5"), ("weird.pdf", "2"), ("bools.pdf", "1"),
                        ("bools.pdf", "2"), ("nopages.pdf", "1"), ("failed.pdf", "1"),
                        ("processing.pdf", "1"), ("same.pdf", "1-2"),
                        ("AMCOR_2022_8K.pdf", "1-9"), ("missing.pdf", "1")]:
        add("main", "get_page_content", {"doc_name": name, "pages": pages})
    add("main", "get_page_content", {"doc_name": "3M_2018_10K.pdf"})
    add("main", "get_page_content", {"pages": "1"})
    add("main", "get_page_content", {"doc_name": "huge.pdf", "pages": "1"}, ["pi-000c"])

    for doc_names in [["annual-report.pdf", "ghost.pdf"], [], ["x", 123], ["  "],
                      [f"n{i}.pdf" for i in range(11)], ["same.pdf"],
                      ["same.pdf", "same.pdf", "big.pdf"], "notalist",
                      ["anual-report.pdf"]]:
        add("main", "remove_document", {"doc_names": doc_names})
    add("main", "remove_document", {"doc_names": ["big.pdf", "annual-report.pdf"]},
        ["pi-0011"])
    add("main", "remove_document", {"doc_names": ["big.pdf"], "folder_id": "x"})

    add("main", "nope", {})
    add("main", "get_document", [1])
    add("main", "get_document", "str")
    add("main", "browse_documents", None)
    return c


def record(text: str):
    if len(text) <= FULL_OUTPUT_LIMIT:
        return {"text": text}
    return {"sha256": hashlib.sha256(text.encode("utf-8")).hexdigest(),
            "len": len(text), "head": text[:1500]}


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    stores = store_specs()
    (OUT / "stores.json").write_text(json.dumps(stores, ensure_ascii=False, separators=(",", ":")) + "\n",
                                     encoding="utf-8")
    out_cases = []
    with tempfile.TemporaryDirectory() as tmp:
        for i, case in enumerate(cases()):
            path = Path(tmp) / f"s{i}"
            build_store(path, stores[case["store"]])
            client = PageIndexLocalClient(storage_path=str(path))
            text, is_error = call_tool(client, case["tool"], case["args"],
                                       doc_ids=case["doc_ids"])
            result = {**case, "is_error": is_error, **record(text)}
            if case["tool"] == "remove_document":
                listing = client.list_documents(limit=10000)
                result["remaining"] = [d["name"] for d in listing["documents"]]
            out_cases.append(result)
            shutil.rmtree(path, ignore_errors=True)
    (OUT / "cases.json").write_text(json.dumps(out_cases, ensure_ascii=False, indent=1) + "\n",
                                    encoding="utf-8")

    surface = {
        "tool_contract": agent_tools.TOOL_CONTRACT,
        "tool_names": list(agent_tools.tool_names(False)),
        "tool_names_management": list(agent_tools.tool_names(True)),
        "local_descriptions": {n: agent_tools._local_description(n)
                               for n in agent_tools.tool_names(True)},
        "local_schemas": {n: agent_tools._local_schema(n) for n in agent_tools.tool_names(True)},
        "agent_instructions": agent_tools.AGENT_INSTRUCTIONS,
        "chat_header": CHAT_HEADER,
        "citation_prompts": agent_tools.LOCAL_CITATION_PROMPTS,
        "constants": {
            "TOOL_RESPONSE_CHAR_LIMIT": agent_tools.TOOL_RESPONSE_CHAR_LIMIT,
            "_CHAR_BUDGET": agent_tools._CHAR_BUDGET,
            "_MAX_REQUESTED_PAGES": agent_tools._MAX_REQUESTED_PAGES,
            "STRUCTURE_FIRST_PAGE_THRESHOLD": agent_tools.STRUCTURE_FIRST_PAGE_THRESHOLD,
        },
    }
    (OUT / "surface.json").write_text(json.dumps(surface, ensure_ascii=False, indent=1) + "\n",
                                      encoding="utf-8")
    print(f"{len(out_cases)} cases written to {OUT}")


if __name__ == "__main__":
    main()
