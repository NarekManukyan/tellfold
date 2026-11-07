"""Real-tool checks for the runtimes marked tested=True. Skipped unless SHOAL_REAL_RUNTIMES=1.

Each makes one tiny call (haiku-class model, a one-word prompt):
    SHOAL_REAL_RUNTIMES=1 uv run pytest -q tests/test_runtimes_real.py
"""

from __future__ import annotations

import os

import anyio
import pytest
from pydantic import BaseModel

from shoal.models import SimConfig
from shoal.runtimes import build_llm, get_runtime, test_runtime
from shoal.settings import Settings

pytestmark = pytest.mark.skipif(os.environ.get("SHOAL_REAL_RUNTIMES") != "1",
                                reason="real runtime calls run only with SHOAL_REAL_RUNTIMES=1")


class Word(BaseModel):
    word: str


def _require(runtime_id: str) -> None:
    if not get_runtime(runtime_id, Settings()).detect().installed:
        pytest.skip(f"{runtime_id} is not installed")


def test_claude_code_text_and_json() -> None:
    _require("claude-code")
    result = anyio.run(lambda: test_runtime("claude-code", Settings()))
    assert result["ok"], result
    llm = build_llm(SimConfig(question="q", seed_files=[]), Settings(), concurrency=1, log_path=None)
    word = anyio.run(lambda: llm.json("Give the word 'ok'.", system="Be terse.", model="haiku",
                                      purpose="engine.round", schema=Word))
    assert word.word.strip()


def test_cursor_agent_text() -> None:
    _require("cursor-agent")
    result = anyio.run(lambda: test_runtime("cursor-agent", Settings()))
    assert result["ok"], result
