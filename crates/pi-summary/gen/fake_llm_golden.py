"""Record a prompt-dependent fake-LLM golden for pi-summary's replay test.

The stub reply of parity/mock_llm.py is the same for every prompt, so a stub run cannot catch a
prompt that differs by one byte, or a summary routed to the wrong node. This helper installs a
deterministic fake whose reply depends on the prompt (expand gets real headings from its pages,
summaries quote their input), records every call in mock_llm's fixture format through
mock_llm's own record mode, and runs `parity/dump_reference.py::full_llm` (page_index_flash
with summary=True, optimize="full") on the stage-09 tree + page texts of each golden doc.

Outputs, per doc, under --out/<doc>/:
  10b_fake.json      the expected result ("data" of what dump_reference would write)
  fake_llm.jsonl     the fixture, with `messages` dropped to keep it small (key + reply suffice)

Run with the reference venv from the repo root:
  /home/user/PageIndex-rust/parity/.venv/bin/python crates/pi-summary/gen/fake_llm_golden.py \
      --golden /home/user/PageIndex-rust/parity/golden --out crates/pi-summary/tests/fixtures \
      earthmover four_lectures q1_fy25_earnings fb_adobe_2017_10k
"""
from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "parity"))

import litellm  # noqa: E402

import dump_reference  # noqa: E402
import mock_llm  # noqa: E402


def looks_like_heading(line: str) -> bool:
    return 3 <= len(line) <= 60 and len(line.split()) <= 8 and any(c.isalpha() for c in line)


def fake_reply(prompt: str) -> str:
    if prompt.startswith("You are splitting"):
        pages = re.findall(r"<page_(\d+)>\n(.*?)\n</page_\1>", prompt, re.S)
        # lines printed on several pages are running headers/footers, not headings
        seen = {}
        for _, text in pages:
            for line in set(l.strip() for l in text.splitlines()):
                seen[line] = seen.get(line, 0) + 1
        subs = []
        for n, text in pages[1:]:
            for line in text.splitlines():
                if seen[line.strip()] == 1 and looks_like_heading(line.strip()):
                    subs.append({"title": line.strip(), "page": int(n)})
                    break
            if len(subs) == 3:
                break
        if not subs:
            return ""  # exercises the empty-reply retry
        # every other node gets a fenced reply
        body = json.dumps({"subsections": subs})
        return f"```json\n{body}\n```" if len(prompt) % 2 else body
    if "Given Text:" in prompt:
        text = prompt.split("Given Text:", 1)[1].rsplit("Reply strictly in the following JSON format:", 1)[0]
        words = text.split()[:8]
        reply = {"summary": "Leaf: " + " ".join(words)}
        if "Also return a short title" in prompt:
            reply = {"title": "Page on " + " ".join(words[:3]), **reply}
        return json.dumps(reply, ensure_ascii=False)
    if "Subsection Titles and Summaries:" in prompt:
        title = prompt.split("Section Title:", 1)[1].split("\n", 1)[0].strip()
        listing = json.loads(prompt.split("Subsection Titles and Summaries:", 1)[1].split("\n", 1)[0])
        first = listing[0]["summary"].split()[:4] if listing else []
        return json.dumps({"summary": f"Section {title[:40]} with {len(listing)} parts; " + " ".join(first)},
                          ensure_ascii=False)
    return mock_llm.STUB_REPLY


async def fake_acompletion(*a, **kw):
    return mock_llm._response(fake_reply(kw["messages"][-1]["content"]))


def fake_completion(*a, **kw):
    return mock_llm._response(fake_reply(kw["messages"][-1]["content"]))


def load(d: Path, name: str):
    return json.loads((d / name).read_text())


def extract_toc_result(d: Path) -> dict:
    s09 = load(d, "09_tree_bookmarks.json")
    return {"doc_name": s09["doc"], "doc_title": load(d, "05_classified.json")["data"]["doc_title"],
            "structure": s09["data"]["structure"],
            "has_abstract_or_references_section": load(d, "07_outline.json")["data"]["has_abstract_or_references"],
            "page_texts": load(d, "page_texts.json")["data"], "toc_source": s09["data"]["toc_source"]}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--golden", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("docs", nargs="+")
    args = ap.parse_args()
    for doc in args.docs:
        with tempfile.TemporaryDirectory() as tmp:
            fixture = Path(tmp) / "fixture.jsonl"
            litellm.acompletion, litellm.completion = fake_acompletion, fake_completion
            mock_llm.install("record", fixture)
            data = dump_reference.full_llm(extract_toc_result(args.golden / doc))
            out = args.out / doc
            out.mkdir(parents=True, exist_ok=True)
            (out / "10b_fake.json").write_text(json.dumps(data, ensure_ascii=False, separators=(",", ":")) + "\n")
            lines = []
            for line in fixture.read_text().splitlines():
                rec = json.loads(line)
                lines.append(json.dumps({"key": rec["key"], "model": rec["model"], "reply": rec["reply"]},
                                        ensure_ascii=False))
            (out / "fake_llm.jsonl").write_text("".join(l + "\n" for l in lines))
            print(json.dumps({"doc": doc, "calls": len(lines), "optimize": data.get("optimize", {}).get("expands")}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
