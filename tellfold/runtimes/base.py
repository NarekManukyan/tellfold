"""Shared runtime types and the `AdapterLLM` base class every non-Claude-Code adapter builds on.

`AdapterLLM` gives each adapter the same guarantees as `tellfold.llm.ClaudeLocal`: only `LLMError` escapes,
retries with exponential backoff (longer after a rate limit), a per-attempt timeout, a concurrency semaphore,
one `calls.jsonl` line per call (lengths and timings, never prompt text), and `json()` that validates against the
pydantic schema with one repair retry. An adapter implements one method, `_send(request) -> AdapterReply`.
"""

from __future__ import annotations

import json as _json
import random
import time
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Literal, Protocol, TypeVar, runtime_checkable

import anyio
from pydantic import BaseModel, Field

from tellfold.llm import LLM, LLMError, extract_json

T = TypeVar("T", bound=BaseModel)

Role = Literal["agent", "planner"]
ROLES: tuple[Role, ...] = ("agent", "planner")
# Claude Code model aliases. Other runtimes swap these for their own default model (see RoutedLLM).
CLAUDE_ALIASES = frozenset({"haiku", "sonnet", "opus"})
# A model value meaning "whatever the tool itself defaults to": adapters omit their --model flag.
TOOL_DEFAULT = "default"


class RuntimeInfo(BaseModel):
    id: str
    kind: Literal["cli", "http"]
    name: str
    installed: bool
    version: str | None = None
    path: str | None = None
    auth: Literal["ok", "missing", "unknown"] = "unknown"
    auth_hint: str | None = None
    models: list[str] = Field(default_factory=list)
    default_models: dict[str, str] = Field(default_factory=dict)
    tested: bool = False
    notes: str | None = None


@runtime_checkable
class Runtime(Protocol):
    @property
    def info(self) -> RuntimeInfo: ...

    def llm(self, *, concurrency: int, log_path: Path | None) -> LLM: ...


# ---------- failures inside an adapter ----------


class Transient(Exception):
    """Worth retrying after a backoff: process crash, timeout, connection error, 5xx, rate limit."""

    def __init__(self, message: str, *, rate_limited: bool = False):
        super().__init__(message)
        self.rate_limited = rate_limited


class Fatal(Exception):
    """Retrying will not help: not installed, not logged in, unknown model, bad request."""


class BadOutput(Exception):
    """The model answered, but not usefully; a repair prompt may fix it."""

    def __init__(self, message: str, *, reply_text: str = ""):
        super().__init__(message)
        self.reply_text = reply_text


@dataclass
class AdapterRequest:
    prompt: str
    system: str
    model: str  # "" or TOOL_DEFAULT means: use the tool's / server's default
    purpose: str
    json_schema: dict[str, Any] | None = None  # set for json(); adapters may enforce it natively
    schema_name: str = "result"

    @property
    def wants_json(self) -> bool:
        return self.json_schema is not None

    @property
    def model_or_none(self) -> str | None:
        return None if not self.model or self.model == TOOL_DEFAULT else self.model

    def combined(self) -> str:
        """System + prompt as one text, for tools that have no separate system prompt input."""
        return f"<instructions>\n{self.system.strip()}\n</instructions>\n\n{self.prompt}"


@dataclass
class AdapterReply:
    text: str = ""
    structured: Any = None  # an already-parsed JSON value, when the backend returned one
    model: str | None = None  # the model that actually answered, if the backend says
    output_tokens: int = 0


@dataclass
class CallRecord:
    purpose: str
    model: str
    prompt_chars: int
    mode: str
    started: float = field(default_factory=time.monotonic)
    attempts: int = 0
    output_chars: int = 0
    output_tokens: int = 0


# ---------- prompts shared by every adapter ----------

_REPAIR_SNIPPET_CHARS = 1500


def json_system(system: str, json_schema: dict[str, Any]) -> str:
    contract = (
        "Your reply is parsed by a program. Respond with exactly one JSON value that validates against "
        "the JSON Schema below: no prose before or after it, no markdown code fences, no comments, "
        "no trailing commas. Every required field must be present and every type must match. Do not use "
        "any tools; answer from the text you were given.\n\n"
        f"JSON Schema:\n{_json.dumps(json_schema, ensure_ascii=False)}"
    )
    return f"{system.rstrip()}\n\n# Output format\n{contract}"


def repair_prompt(prompt: str, problem: str, previous: str) -> str:
    snippet = previous.strip()[:_REPAIR_SNIPPET_CHARS] or "(empty)"
    return (
        f"{prompt}\n\n---\nYour previous answer to this request could not be used.\n"
        f"Problem: {problem[:800]}\n"
        f"Previous answer (truncated):\n{snippet}\n\n"
        "Answer the original request again, fixing the problem. Output only the corrected JSON."
    )


def root_opener(json_schema: dict[str, Any]) -> str:
    return "[" if json_schema.get("type") == "array" else "{"


# ---------- the base class ----------


class AdapterLLM:
    """`LLM` implementation shared by all adapters. Subclasses implement `_send`."""

    runtime_id: str = "adapter"
    backoff_base_s: float = 2.0
    rate_limit_backoff_s: float = 20.0

    def __init__(self, *, concurrency: int = 4, log_path: Path | None = None, timeout_s: float = 240,
                 retries: int = 2):
        self._semaphore = anyio.Semaphore(max(1, concurrency))
        self._log_path = Path(log_path) if log_path else None
        self._timeout_s = timeout_s
        self._retries = max(0, retries)

    # ----- the LLM protocol -----

    async def text(self, prompt: str, *, system: str, model: str, purpose: str) -> str:
        record = CallRecord(purpose, model, len(prompt) + len(system), mode="text")
        try:
            try:
                reply = await self._with_retries(AdapterRequest(prompt, system, model, purpose), record)
                text = (reply.text or "").strip()
                if not text and reply.structured is not None:
                    text = _json.dumps(reply.structured, ensure_ascii=False)
                if not text:
                    raise BadOutput("empty reply")
            except BadOutput as err:
                raise LLMError(f"{purpose}: unusable reply from {self.runtime_id}: {err}") from err
        except BaseException as err:
            self._log(record, err)
            raise
        record.output_chars = len(text)
        self._log(record, None)
        return text

    async def json(self, prompt: str, *, system: str, model: str, purpose: str, schema: type[T]) -> T:
        json_schema = schema.model_json_schema()
        record = CallRecord(purpose, model, len(prompt) + len(system), mode="json")
        name = str(json_schema.get("title") or schema.__name__ or "result")
        try:
            sys_text = json_system(system, json_schema)
            try:
                request = AdapterRequest(prompt, sys_text, model, purpose, json_schema, name)
                reply = await self._with_retries(request, record)
                result = self._parse(reply, schema, json_schema, record)
            except BadOutput as first:
                problem, previous = str(first), first.reply_text
                request = AdapterRequest(repair_prompt(prompt, problem, previous), sys_text, model, purpose,
                                         json_schema, name)
                try:
                    reply = await self._with_retries(request, record)
                    result = self._parse(reply, schema, json_schema, record)
                except BadOutput as second:
                    raise LLMError(f"{purpose}: invalid JSON after repair: {second}") from second
        except BaseException as err:
            self._log(record, err)
            raise
        self._log(record, None)
        return result

    # ----- machinery -----

    @staticmethod
    def _parse(reply: AdapterReply, schema: type[T], json_schema: dict[str, Any], record: CallRecord) -> T:
        structured_chars = len(_json.dumps(reply.structured, default=str)) if reply.structured is not None else 0
        record.output_chars = structured_chars or len(reply.text or "")
        try:
            value = reply.structured
            if isinstance(value, str):
                value = extract_json(value, prefer=root_opener(json_schema))
            if value is None:
                value = extract_json(reply.text or "", prefer=root_opener(json_schema))
            return schema.model_validate(value)
        except ValueError as err:  # includes pydantic.ValidationError
            raise BadOutput(str(err), reply_text=reply.text or _json.dumps(reply.structured, default=str)) from err

    async def _with_retries(self, request: AdapterRequest, record: CallRecord) -> AdapterReply:
        for attempt in range(self._retries + 1):
            record.attempts += 1
            try:
                async with self._semaphore:
                    with anyio.fail_after(self._timeout_s):
                        reply = await self._send(request)
                record.output_tokens += reply.output_tokens
                return reply
            except TimeoutError:
                failure = Transient(f"timed out after {self._timeout_s:.0f}s")
            except Transient as err:
                failure = err
            except Fatal as err:
                raise LLMError(f"{request.purpose}: {self.runtime_id}: {err}") from err
            except (BadOutput, LLMError):
                raise
            except Exception as err:  # noqa: BLE001 - an adapter bug must still surface as LLMError
                raise LLMError(f"{request.purpose}: {self.runtime_id}: {type(err).__name__}: "
                               f"{str(err)[:300]}") from err
            if attempt == self._retries:
                raise LLMError(f"{request.purpose}: {self.runtime_id} failed after {attempt + 1} attempts: "
                               f"{failure}") from failure
            await anyio.sleep(self._backoff(attempt, failure.rate_limited))
        raise AssertionError("unreachable")

    def _backoff(self, attempt: int, rate_limited: bool) -> float:
        base = self.rate_limit_backoff_s if rate_limited else self.backoff_base_s
        return base * (2**attempt) * random.uniform(0.8, 1.2)

    async def _send(self, request: AdapterRequest) -> AdapterReply:  # pragma: no cover - abstract
        raise NotImplementedError

    def _log(self, record: CallRecord, error: BaseException | None) -> None:
        if self._log_path is None:
            return
        entry = {
            "ts": datetime.now(timezone.utc).isoformat(timespec="seconds"),
            "purpose": record.purpose,
            "model": record.model,
            "runtime": self.runtime_id,
            "mode": record.mode,
            "seconds": round(time.monotonic() - record.started, 3),
            "attempts": record.attempts,
            "ok": error is None,
            "error": None if error is None else f"{type(error).__name__}: {str(error)[:300]}",
            "prompt_chars": record.prompt_chars,
            "output_chars": record.output_chars,
            "output_tokens": record.output_tokens,
        }
        try:
            self._log_path.parent.mkdir(parents=True, exist_ok=True)
            with self._log_path.open("a", encoding="utf-8") as fh:
                fh.write(_json.dumps(entry, ensure_ascii=False) + "\n")
        except OSError:
            pass


# ---------- error text classification (CLI stderr, HTTP bodies) ----------

AUTH_HINTS = ("not logged in", "login", "log in", "authenticat", "unauthorized", "api key", "api_key",
              "credentials", "forbidden", "401", "403")
RATE_HINTS = ("rate limit", "rate_limit", "ratelimit", "usage limit", "quota", "too many requests", "overloaded",
              "429", "resource_exhausted")
MODEL_HINTS = ("unknown model", "invalid model", "model not found", "model_not_found", "no such model",
               "not a valid model", "unsupported model", "does not exist")


def classify_failure(message: str, *, auth_hint: str | None = None) -> Exception:
    """Map a tool's error text onto Fatal (auth / model) or Transient (rate limit / anything else)."""
    text = " ".join(message.split())[:400]
    lowered = text.lower()
    if any(h in lowered for h in RATE_HINTS):
        return Transient(text or "rate limited", rate_limited=True)
    if any(h in lowered for h in MODEL_HINTS):
        return Fatal(text)
    if any(h in lowered for h in AUTH_HINTS):
        return Fatal(f"{auth_hint} ({text})" if auth_hint else text)
    return Transient(text or "the tool failed without an error message")
