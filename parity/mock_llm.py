"""Record / replay / stub for every LiteLLM call the reference makes (indexing lane).

Both implementations key replies the same way, so optimize/summary parity is deterministic:

    key = sha256(json.dumps(messages, ensure_ascii=False, separators=(",", ":")))

`messages` is the chat message list exactly as sent (role/content dicts). The model is not part
of the key, so a fixture recorded with one model can be replayed by any; it is stored alongside
for provenance. Fixture format: JSONL of {"key", "model", "messages", "reply"}.

Modes:
  record  call the real endpoint, append new replies to the fixture
  replay  serve from the fixture; a miss raises (never silently calls out)
  stub    deterministic canned reply, no network (for smoke tests of stage 10 before recording)

Usage (in-process):  import mock_llm; mock_llm.install("replay", "parity/mock_llm/fixtures.jsonl")
Usage (env):         PI_MOCK_LLM=replay PI_MOCK_LLM_FIXTURE=... then `import mock_llm; mock_llm.install_from_env()`

The chat/agent lane (OpenAI Agents SDK) does not go through LiteLLM and is not covered.
"""
from __future__ import annotations

import hashlib
import json
import os
import threading
from pathlib import Path
from types import SimpleNamespace

STUB_REPLY = json.dumps({"subsections": [], "summary": "stub summary", "title": "stub title",
                         "description": "stub description"})


def key_of(messages) -> str:
    canon = json.dumps(list(messages), ensure_ascii=False, separators=(",", ":"))
    return hashlib.sha256(canon.encode("utf-8")).hexdigest()


class MockMiss(RuntimeError):
    pass


class _Store:
    def __init__(self, path: Path):
        self.path = path
        self.lock = threading.Lock()
        self.replies: dict[str, str] = {}
        if path.exists():
            for line in path.read_text().splitlines():
                if line.strip():
                    rec = json.loads(line)
                    self.replies[rec["key"]] = rec["reply"]

    def add(self, key, model, messages, reply):
        with self.lock:
            if key in self.replies:
                return
            self.replies[key] = reply
            self.path.parent.mkdir(parents=True, exist_ok=True)
            with open(self.path, "a") as f:
                f.write(json.dumps({"key": key, "model": model, "messages": messages, "reply": reply},
                                   ensure_ascii=False) + "\n")


def _response(content: str):
    msg = SimpleNamespace(content=content, role="assistant", tool_calls=None)
    choice = SimpleNamespace(message=msg, finish_reason="stop", index=0)
    return SimpleNamespace(choices=[choice], usage=SimpleNamespace(prompt_tokens=0, completion_tokens=0,
                                                                   total_tokens=0))


def install(mode: str, fixture: str | os.PathLike | None = None):
    import litellm

    if mode not in ("record", "replay", "stub"):
        raise ValueError(f"unknown mock mode {mode!r}")
    store = _Store(Path(fixture)) if fixture else None
    if mode in ("record", "replay") and store is None:
        raise ValueError(f"{mode} needs a fixture path")
    real_sync, real_async = litellm.completion, litellm.acompletion

    def lookup(kw):
        messages = kw.get("messages") or []
        key = key_of(messages)
        if mode == "stub":
            return key, _response(STUB_REPLY)
        if key in store.replies:
            return key, _response(store.replies[key])
        if mode == "replay":
            raise MockMiss(f"no recorded reply for prompt {key[:12]} ({len(store.replies)} in fixture)")
        return key, None

    def completion(*a, **kw):
        key, hit = lookup(kw)
        if hit is not None:
            return hit
        resp = real_sync(*a, **kw)
        store.add(key, kw.get("model"), kw.get("messages"), resp.choices[0].message.content)
        return resp

    async def acompletion(*a, **kw):
        key, hit = lookup(kw)
        if hit is not None:
            return hit
        resp = await real_async(*a, **kw)
        store.add(key, kw.get("model"), kw.get("messages"), resp.choices[0].message.content)
        return resp

    litellm.completion, litellm.acompletion = completion, acompletion
    return store


def install_from_env():
    mode = os.environ.get("PI_MOCK_LLM")
    if mode:
        return install(mode, os.environ.get("PI_MOCK_LLM_FIXTURE"))
    return None
