"""Agent runtimes: detect the LLM tools installed on this Mac and drive any of them through the `LLM` protocol.

```python
detect_all(settings, check_auth=False) -> list[RuntimeInfo]
get_runtime(runtime_id, settings) -> Runtime
build_llm(config, settings, *, concurrency, log_path) -> RoutedLLM
```
`build_llm` sends `engine.*` and `chat.interview` calls to the agent runtime and everything else (graph, personas,
report, ask) to the planner runtime.
"""

from __future__ import annotations

import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path
from typing import Any, TypeVar

from pydantic import BaseModel

from shoal.llm import LLM, LLMError
from shoal.models import SimConfig
from shoal.runtimes.base import CLAUDE_ALIASES, TOOL_DEFAULT, Role, Runtime, RuntimeInfo
from shoal.runtimes.registry import (
    AnyRuntime,
    UnknownRuntime,
    all_runtimes,
    get_runtime,
    known_ids,
)
from shoal.settings import Settings

__all__ = [
    "AGENT_PURPOSES",
    "RoutedLLM",
    "Runtime",
    "RuntimeInfo",
    "UnknownRuntime",
    "build_llm",
    "detect_all",
    "get_runtime",
    "is_agent_purpose",
    "known_ids",
    "test_runtime",
]

T = TypeVar("T", bound=BaseModel)
AGENT_PURPOSES: tuple[str, ...] = ("engine.", "chat.interview")
DETECT_WORKERS = 16


def is_agent_purpose(purpose: str) -> bool:
    return purpose.startswith(AGENT_PURPOSES)


def detect_all(settings: Settings, *, check_auth: bool = False) -> list[RuntimeInfo]:
    """Every runtime with its status, probed in parallel (version calls time out after 3 s each)."""
    runtimes = all_runtimes(settings)

    def probe(runtime: AnyRuntime) -> RuntimeInfo:
        try:
            return runtime.detect(check_auth=check_auth)
        except Exception as e:  # noqa: BLE001 - one broken probe must not hide the others
            return RuntimeInfo(id=runtime.id, kind=runtime.kind, name=getattr(runtime, "name", runtime.id),
                               installed=False, notes=f"detection failed: {type(e).__name__}: {e}"[:300])

    with ThreadPoolExecutor(max_workers=min(DETECT_WORKERS, len(runtimes))) as pool:
        return list(pool.map(probe, runtimes))


@dataclass
class Route:
    llm: LLM
    runtime_id: str
    default_model: str
    accepts_claude_aliases: bool

    def model(self, requested: str) -> str:
        """The caller's model, unless it is a Claude alias (or empty) sent to a runtime that cannot use one."""
        if self.accepts_claude_aliases:
            return requested
        if not requested or requested in CLAUDE_ALIASES:
            return self.default_model or TOOL_DEFAULT
        return requested


class RoutedLLM:
    """`LLM` that sends agent purposes to one backend and planner purposes to another (or the same) one."""

    def __init__(self, agent: Route, planner: Route):
        self.agent = agent
        self.planner = planner

    def route(self, purpose: str) -> Route:
        return self.agent if is_agent_purpose(purpose) else self.planner

    async def text(self, prompt: str, *, system: str, model: str, purpose: str) -> str:
        route = self.route(purpose)
        return await route.llm.text(prompt, system=system, model=route.model(model), purpose=purpose)

    async def json(self, prompt: str, *, system: str, model: str, purpose: str, schema: type[T]) -> T:
        route = self.route(purpose)
        return await route.llm.json(prompt, system=system, model=route.model(model), purpose=purpose, schema=schema)


def _route(runtime: AnyRuntime, llm: LLM, role: Role) -> Route:
    return Route(llm=llm, runtime_id=runtime.id, default_model=runtime.static_default(role),
                 accepts_claude_aliases=runtime.accepts_claude_aliases)


def build_llm(config: SimConfig, settings: Settings, *, concurrency: int, log_path: Path | None,
              think: bool | None = None, **options: Any) -> RoutedLLM:
    """One LLM for a run: agent purposes go to `config.agent_runtime`, the rest to `config.planner_runtime`.

    Raises `UnknownRuntime` for an id that is not in the catalogue. `think` defaults to `config.think`;
    `options` (e.g. `timeout_s`, `retries`) are passed to the adapters."""
    think = config.think if think is None else think
    agent_rt = get_runtime(config.agent_runtime, settings)
    planner_rt = agent_rt if config.planner_runtime == config.agent_runtime \
        else get_runtime(config.planner_runtime, settings)
    agent_llm = agent_rt.llm(concurrency=concurrency, log_path=log_path, think=think, **options)
    planner_llm = agent_llm if planner_rt is agent_rt \
        else planner_rt.llm(concurrency=concurrency, log_path=log_path, think=think, **options)
    return RoutedLLM(_route(agent_rt, agent_llm, "agent"), _route(planner_rt, planner_llm, "planner"))


PING_PROMPT = "Reply with the single word: ok"
PING_SYSTEM = "You are a connectivity check. Answer with one word."


async def test_runtime(runtime_id: str, settings: Settings, *, model: str | None = None, role: Role = "agent",
                       timeout_s: float = 120) -> dict[str, Any]:
    """One tiny real call: `{ok, seconds, reply, error, model}`. Raises UnknownRuntime for an unknown id."""
    runtime = get_runtime(runtime_id, settings)
    chosen = model or runtime.static_default(role) or TOOL_DEFAULT
    llm = runtime.llm(concurrency=1, log_path=None, timeout_s=timeout_s, retries=0)
    started = time.monotonic()
    try:
        reply = await llm.text(PING_PROMPT, system=PING_SYSTEM, model=chosen, purpose="runtime.test")
    except LLMError as e:
        return {"ok": False, "seconds": round(time.monotonic() - started, 2), "reply": None,
                "error": str(e)[:500], "model": chosen}
    return {"ok": bool(reply.strip()), "seconds": round(time.monotonic() - started, 2), "reply": reply.strip()[:200],
            "error": None, "model": chosen}


test_runtime.__test__ = False  # not a pytest test, despite the name
