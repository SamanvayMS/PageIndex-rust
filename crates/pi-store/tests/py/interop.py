"""Python side of the pi-store interop tests (reference @619cbd8, local mode, no LLM).

    interop.py read STORE                  -> JSON of the SDK's read surface for every doc
    interop.py write STORE SAMPLE.json     -> commit SAMPLE's docs the way LocalAPI does
    interop.py save STORE DOC.json         -> DocStore.save_document with a literal record
    interop.py trylock STORE               -> "free" or "locked" (non-blocking flock)
"""
import fcntl
import json
import sys

from pageindex import PageIndexLocalClient
from pageindex.local_api import LocalAPI, _now_iso
from pageindex.local_store import DocStore
from pageindex.utils import remove_fields


def read(store):
    client = PageIndexLocalClient(storage_path=store)
    listing = client.list_documents(limit=10000)
    docs = {}
    for doc in listing["documents"]:
        doc_id = doc["id"]
        docs[doc_id] = {
            "get_document": client.get_document(doc_id),
            "get_document_structure": client.get_document_structure(doc_id),
            "get_tree": client.get_tree(doc_id, node_summary=True),
            "get_page_content": client.get_page_content(doc_id, "1-3,5"),
            "get_ocr_raw": client.get_ocr(doc_id, format="raw"),
            "get_ocr_node": client.get_ocr(doc_id, format="node"),
            "raw_tree": client._api.raw_tree(doc_id),
            "id_by_name": client.get_document_id(doc["name"]),
        }
    return {"list_documents": listing, "docs": docs}


def write(store, sample_path):
    with open(sample_path, encoding="utf-8") as f:
        sample = json.load(f)
    api = LocalAPI(store, model="none", summary_model="none")
    out = []
    for doc in sample["docs"]:
        texts = doc["pages"]
        api._check_page_bounds(doc["tree"], len(texts))
        doc_id = "pi-" + __import__("uuid").uuid4().hex
        pages = [{"page_index": i + 1, "markdown": t} for i, t in enumerate(texts)]
        with api._store.lock():
            meta = {
                "id": doc_id,
                "name": api._unique_doc_name(doc["name"]),
                "description": doc["description"],
                "status": "completed",
                "createdAt": _now_iso(),
                "pageNum": len(texts),
                "folderId": None,
                "metadata": doc["metadata"],
                "mode": "flash",
            }
            api._store.save_document(doc_id, meta, remove_fields(doc["tree"], fields=["text"]),
                                     pages)
        out.append({"doc_id": doc_id, "name": meta["name"]})
    return out


def save(store, record_path):
    with open(record_path, encoding="utf-8") as f:
        record = json.load(f)
    DocStore(store).save_document(record["meta"]["id"], record["meta"], record["tree"],
                                  record["pages"])
    return {"ok": True}


def trylock(store):
    with open(f"{store}/.lock", "w") as handle:
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return "locked"
        fcntl.flock(handle, fcntl.LOCK_UN)
        return "free"


if __name__ == "__main__":
    cmd, store, *rest = sys.argv[1:]
    result = {"read": read, "write": write, "save": save, "trylock": trylock}[cmd](store, *rest)
    sys.stdout.write(json.dumps(result, ensure_ascii=False))
