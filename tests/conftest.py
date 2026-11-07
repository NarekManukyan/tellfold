"""Shared fixtures. Tests never spawn the real Claude CLI: everything goes through FakeLLM."""

from __future__ import annotations

from collections.abc import Callable, Mapping
from pathlib import Path
from typing import Any

import pytest
from pydantic import BaseModel

from shoal.llm import FakeLLM

# A route answers one purpose. It can be a fixed value (str / dict / Exception), a list of values
# consumed one per call (the last one repeats), or a callable(prompt, schema) -> value.
Route = Any
FakeLLMFactory = Callable[..., FakeLLM]


def _lookup(routes: Mapping[str, Route], purpose: str) -> Route | None:
    """Exact purpose first, then the longest dotted prefix ("engine" answers "engine.round")."""
    if purpose in routes:
        return routes[purpose]
    parts = purpose.split(".")
    for cut in range(len(parts) - 1, 0, -1):
        prefix = ".".join(parts[:cut])
        if prefix in routes:
            return routes[prefix]
    return None


def make_responder(routes: Mapping[str, Route], default: Route | None = None):
    used: dict[int, int] = {}

    def responder(purpose: str, prompt: str, schema: type[BaseModel] | None):
        route = _lookup(routes, purpose)
        if route is None:
            route = default
        if route is None:
            raise AssertionError(f"FakeLLM has no route for purpose {purpose!r}; known: {sorted(routes)}")
        if isinstance(route, list):
            index = used.get(id(route), 0)
            used[id(route)] = index + 1
            return route[min(index, len(route) - 1)]
        if callable(route) and not isinstance(route, (BaseException, type)):
            return route(prompt, schema)
        return route

    return responder


@pytest.fixture
def fake_llm() -> FakeLLMFactory:
    """Factory: `fake_llm({"graph.extract": {...}, "engine": handler}, default="ok") -> FakeLLM`."""

    def make(routes: Mapping[str, Route] | None = None, default: Route | None = None) -> FakeLLM:
        return FakeLLM(make_responder(routes or {}, default))

    return make


@pytest.fixture
def run_dir(tmp_path: Path) -> Path:
    """An empty run directory shaped like `runs/<run_id>/`."""
    path = tmp_path / "runs" / "20260101-000000-test"
    path.mkdir(parents=True)
    return path
