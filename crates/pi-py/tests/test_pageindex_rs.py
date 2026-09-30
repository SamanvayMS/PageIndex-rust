"""Smoke test of the pageindex_rs extension (build it first: `maturin develop` in
crates/pi-py). Skipped when the extension, PDFium (PDFIUM_LIB) or the reference example PDF
is unavailable. No model is called: summaries are off and optimize is merge-only."""
import json
import os
from pathlib import Path

import pytest

pageindex_rs = pytest.importorskip("pageindex_rs")

REF = Path(os.environ.get("PI_REF_DIR", "/home/user/PageIndex-rust/parity/.ref"))
PDF = REF / "examples" / "documents" / "earthmover.pdf"


@pytest.fixture
def pdf():
    if not PDF.is_file():
        pytest.skip(f"{PDF} not found")
    if not os.environ.get("PDFIUM_LIB"):
        pytest.skip("PDFIUM_LIB not set")
    return PDF


def test_index_then_structure(pdf, tmp_path):
    storage = tmp_path / ".pageindex"
    out = pageindex_rs.index(str(pdf), storage=str(storage), accept_flat=True)
    assert out["doc_id"].startswith("pi-")
    assert out["name"] == "earthmover.pdf"
    assert out["result"]["structure"]
    assert out["timings"]["total_s"] > 0

    envelope = json.loads(pageindex_rs.call_tool(
        str(storage), "get_document_structure", json.dumps({"doc_name": "earthmover.pdf"})))
    assert envelope["success"] is True
    assert envelope["structure"][0]["start_index"] == 1

    pages = json.loads(pageindex_rs.call_tool(
        str(storage), "get_page_content", json.dumps({"doc_name": "earthmover.pdf", "pages": "1"})))
    assert pages["content"][0]["text"]


def test_refused_flat_tree_is_not_stored(pdf, tmp_path):
    out = pageindex_rs.index(str(pdf), storage=str(tmp_path / "s"))
    assert out["doc_id"] is None
    assert "12 pages" in out["rejected"]


def test_extract_and_triage(pdf):
    spans = pageindex_rs.extract_spans(str(pdf))
    assert len(spans) == 12 and spans[0]["page"] == 1 and spans[0]["spans"]
    labels = pageindex_rs.triage(str(pdf))
    assert [p["label"] for p in labels] == ["text"] * 12


def test_bad_arguments():
    with pytest.raises(ValueError):
        pageindex_rs.index("x.pdf", optimize="fast")
    envelope = json.loads(pageindex_rs.call_tool("/nonexistent", "nope", "{}"))
    assert envelope["errorCode"] == "INVALID_INPUT"
