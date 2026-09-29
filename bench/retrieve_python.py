"""Retrieval benchmark of the Python PageIndex reference on FinanceBench.

1. Index each needed filing with the reference local client (flash + LLM summaries/merge/expand).
2. Ask each question through the reference agent, scoped to its filing, logging every tool call.
3. Score: evidence-page hit (gold page among fetched pages), pages read, turns, latency,
   and answer correctness via an LLM judge against the gold answer.

LLM connection comes from the environment (OpenAI-compatible):
  PI_LLM_BASE_URL, PI_LLM_MODEL, PI_LLM_KEY   [+ optional PI_CHAT_MODEL, PI_JUDGE_MODEL]
"""
from __future__ import annotations

import argparse
import json
import os
import statistics
import sys
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
DATA = HERE / "data"


def env(name: str, default: str | None = None) -> str:
    v = os.environ.get(name, default)
    if not v:
        sys.exit(f"missing environment variable {name}")
    return v


def litellm_name(model: str) -> str:
    # Route through LiteLLM's OpenAI-compatible provider unless a provider prefix is given.
    return model if "/" in model else f"openai/{model}"


_TOOL_LOG = threading.local()


def install_tool_logger():
    from pageindex import agent_tools

    for name, impl in list(agent_tools._IMPLEMENTATIONS.items()):
        def wrapped(*a, __impl=impl, __name=name, **kw):
            t0 = time.perf_counter()
            payload, is_error = __impl(*a, **kw)
            log = getattr(_TOOL_LOG, "calls", None)
            if log is not None:
                args = {k: v for k, v in kw.items() if not k.startswith("_")}
                log.append({"tool": __name, "args": args, "error": bool(is_error),
                            "ms": round(1000 * (time.perf_counter() - t0), 1),
                            "chars": len(json.dumps(payload, ensure_ascii=False))})
            return payload, is_error
        agent_tools._IMPLEMENTATIONS[name] = wrapped


def pages_fetched(calls) -> set[int]:
    from pageindex.agent_tools import _expand_pages

    got: set[int] = set()
    for c in calls:
        if c["tool"] == "get_page_content" and not c["error"]:
            try:
                got.update(_expand_pages(c["args"].get("pages", "")))
            except Exception:  # noqa: BLE001 - malformed spec: counted as no pages
                pass
    return got


JUDGE_PROMPT = """You are grading an answer to a financial question about an SEC filing.
Question: {question}
Gold answer: {gold}
Candidate answer: {answer}

Is the candidate answer correct? Allow rounding and formatting differences and equivalent units;
the core figure or claim must match the gold answer. If the candidate declines or says the
information is unavailable, label it "refusal".
Reply with JSON only: {{"label": "correct" | "incorrect" | "refusal", "reason": "<one sentence>"}}"""


def judge(question, gold, answer, model, base, key) -> dict:
    import litellm

    for attempt in range(3):
        try:
            r = litellm.completion(
                model=litellm_name(model), api_base=base, api_key=key, temperature=0,
                messages=[{"role": "user", "content": JUDGE_PROMPT.format(
                    question=question, gold=gold, answer=answer)}])
            text = r.choices[0].message.content.strip()
            text = text[text.find("{"): text.rfind("}") + 1]
            return json.loads(text)
        except Exception as e:  # noqa: BLE001
            err = f"{type(e).__name__}: {e}"
            time.sleep(2 ** attempt)
    return {"label": "judge_error", "reason": err[:300]}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", default=time.strftime("%Y%m%d-%H%M%S"))
    ap.add_argument("--limit", type=int, help="first N questions")
    ap.add_argument("--ids", nargs="*", help="financebench_id subset")
    ap.add_argument("--max-turns", type=int, default=None)
    args = ap.parse_args()

    base, model, key = env("PI_LLM_BASE_URL"), env("PI_LLM_MODEL"), env("PI_LLM_KEY")
    chat_model = os.environ.get("PI_CHAT_MODEL", model)
    judge_model = os.environ.get("PI_JUDGE_MODEL", chat_model)
    os.environ.setdefault("OPENAI_API_KEY", key)
    os.environ.setdefault("OPENAI_BASE_URL", base)
    os.environ.setdefault("OPENAI_API_BASE", base)

    from pageindex import PageIndexClient

    install_tool_logger()
    out = HERE / "results" / args.run
    out.mkdir(parents=True, exist_ok=True)
    questions = [json.loads(l) for l in (DATA / "financebench_open_source.jsonl").read_text().splitlines() if l.strip()]
    if args.ids:
        questions = [q for q in questions if q["financebench_id"] in set(args.ids)]
    if args.limit:
        questions = questions[: args.limit]

    client = PageIndexClient(
        index_model=litellm_name(model), chat_model=chat_model,
        storage_path=str(out / ".pageindex"),
        index_backend={"api_key": key, "api_base": base},
        chat_backend={"api_key": key, "base_url": base},
    )

    # Index (resumable: doc ids cached per run).
    ids_path = out / "doc_ids.json"
    doc_ids = json.loads(ids_path.read_text()) if ids_path.exists() else {}
    index_rows = []
    for doc in sorted({q["doc_name"] for q in questions}):
        if doc in doc_ids:
            continue
        t0 = time.perf_counter()
        try:
            res = client.submit_document(str(DATA / "pdfs" / f"{doc}.pdf"))
            doc_ids[doc] = res["doc_id"]
            row = {"doc": doc, "index_s": round(time.perf_counter() - t0, 2), "ok": True}
        except Exception as e:  # noqa: BLE001
            row = {"doc": doc, "index_s": round(time.perf_counter() - t0, 2), "ok": False,
                   "error": f"{type(e).__name__}: {e}"[:300]}
        index_rows.append(row)
        ids_path.write_text(json.dumps(doc_ids, indent=1))
        print(f"index {doc}: {row}", flush=True)
    with open(out / "index_llm.jsonl", "a") as f:
        for r in index_rows:
            f.write(json.dumps(r) + "\n")

    # Retrieve + judge (resumable per question).
    res_path = out / "retrieval.jsonl"
    done = set()
    if res_path.exists():
        done = {json.loads(l)["id"] for l in res_path.read_text().splitlines() if l.strip()}
    for i, q in enumerate(questions, 1):
        if q["financebench_id"] in done or q["doc_name"] not in doc_ids:
            continue
        _TOOL_LOG.calls = []
        t0 = time.perf_counter()
        try:
            kw = {"doc_id": doc_ids[q["doc_name"]]}
            if args.max_turns:
                kw["max_turns"] = args.max_turns
            answer = client.chat(q["question"], **kw)
            err = None
        except Exception as e:  # noqa: BLE001
            answer, err = "", f"{type(e).__name__}: {e}"[:300]
        latency = time.perf_counter() - t0
        calls = _TOOL_LOG.calls
        _TOOL_LOG.calls = None
        gold_pages = {int(ev["evidence_page_num"]) + 1 for ev in q["evidence"]}
        got = pages_fetched(calls)
        verdict = judge(q["question"], q["answer"], answer, judge_model, base, key) if answer else \
            {"label": "error", "reason": err}
        rec = {
            "id": q["financebench_id"], "doc": q["doc_name"], "question_type": q["question_type"],
            "gold_pages": sorted(gold_pages), "pages_read": sorted(got),
            "evidence_hit": bool(gold_pages & got), "n_pages_read": len(got),
            "tool_calls": calls, "n_tool_calls": len(calls), "latency_s": round(latency, 2),
            "answer": answer, "gold": q["answer"], "judge": verdict, "error": err,
        }
        with open(res_path, "a") as f:
            f.write(json.dumps(rec, ensure_ascii=False) + "\n")
        print(f"[{i}/{len(questions)}] {rec['id']} hit={rec['evidence_hit']} "
              f"pages={rec['n_pages_read']} calls={rec['n_tool_calls']} {rec['latency_s']}s "
              f"judge={verdict.get('label')}", flush=True)

    recs = [json.loads(l) for l in res_path.read_text().splitlines() if l.strip()]
    if recs:
        lat = sorted(r["latency_s"] for r in recs)
        summary = {
            "n": len(recs),
            "evidence_hit_rate": round(sum(r["evidence_hit"] for r in recs) / len(recs), 4),
            "accuracy": round(sum(r["judge"].get("label") == "correct" for r in recs) / len(recs), 4),
            "refusal_rate": round(sum(r["judge"].get("label") == "refusal" for r in recs) / len(recs), 4),
            "pages_read_mean": round(statistics.mean(r["n_pages_read"] for r in recs), 2),
            "tool_calls_mean": round(statistics.mean(r["n_tool_calls"] for r in recs), 2),
            "latency_p50_s": lat[len(lat) // 2],
            "latency_p95_s": lat[min(len(lat) - 1, int(0.95 * len(lat)))],
            "models": {"index": model, "chat": chat_model, "judge": judge_model},
        }
        (out / "summary.json").write_text(json.dumps(summary, indent=1))
        print(json.dumps(summary, indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
