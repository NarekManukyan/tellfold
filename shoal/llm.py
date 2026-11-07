"""LLM backends. `ClaudeLocal` drives the local Claude Code CLI; `FakeLLM` is for tests."""

from __future__ import annotations

import inspect
import json as _json
import os
import random
import re
import shutil
import time
from contextlib import aclosing
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Awaitable, Callable, Protocol, TypeVar

import anyio
import claude_agent_sdk
from claude_agent_sdk import (
    AssistantMessage,
    ClaudeAgentOptions,
    ClaudeSDKError,
    CLINotFoundError,
    ResultError,
    ResultMessage,
    TextBlock,
)
from pydantic import BaseModel

T = TypeVar("T", bound=BaseModel)


class LLMError(Exception):
    """Raised when a call fails after all retries or returns unusable output."""


class LLM(Protocol):
    async def text(self, prompt: str, *, system: str, model: str, purpose: str) -> str: ...

    async def json(
        self, prompt: str, *, system: str, model: str, purpose: str, schema: type[T]
    ) -> T: ...


# responder(purpose, prompt, schema) -> str for text(), dict or JSON str for json(); may be async
Responder = Callable[[str, str, "type[BaseModel] | None"], "str | dict | Awaitable[str | dict]"]


class FakeLLM:
    """Deterministic test double. Records every call in `self.calls` as (purpose, model, prompt)."""

    def __init__(self, responder: Responder):
        self.responder = responder
        self.calls: list[tuple[str, str, str]] = []

    async def _respond(self, purpose: str, prompt: str, schema: type[BaseModel] | None):
        out = self.responder(purpose, prompt, schema)
        if inspect.isawaitable(out):
            out = await out
        return out

    async def text(self, prompt: str, *, system: str, model: str, purpose: str) -> str:
        self.calls.append((purpose, model, prompt))
        out = await self._respond(purpose, prompt, None)
        if isinstance(out, Exception):
            raise out
        return out if isinstance(out, str) else str(out)

    async def json(self, prompt: str, *, system: str, model: str, purpose: str, schema: type[T]) -> T:
        self.calls.append((purpose, model, prompt))
        out = await self._respond(purpose, prompt, schema)
        if isinstance(out, Exception):
            raise out
        try:
            return schema.model_validate_json(out) if isinstance(out, str) else schema.model_validate(out)
        except Exception as e:  # mirror ClaudeLocal: invalid output surfaces as LLMError
            raise LLMError(f"{purpose}: invalid fake output: {e}") from e


# ClaudeLocal is implemented by the llm builder below this line (see ARCHITECTURE.md).


# ---------------------------------------------------------------------------------------------
# ClaudeLocal: the real backend, driving the locally installed Claude Code CLI via claude-agent-sdk.
# ---------------------------------------------------------------------------------------------

_RETRYABLE_STATUS = {408, 409, 429, 500, 502, 503, 504, 529}
_AUTH_STATUS = {401, 403}
_AUTH_HINT = "Claude Code is not logged in: run `claude` once and use /login, then retry."
_RATE_HINTS = ("rate limit", "rate_limit", "usage limit", "overloaded", "too many requests")
_AUTH_HINTS = ("failed to authenticate", "oauth", "not logged in", "invalid api key", "/login")

# Purposes that keep extended thinking on. Thinking makes haiku emit 10-20k hidden tokens per call
# (measured: graph extraction 126s with thinking vs 9.5s without), so it is off everywhere else.
DEFAULT_THINK_PURPOSES: tuple[str, ...] = ("report.write",)
PLANNER_THINK_PURPOSES: tuple[str, ...] = ("graph.", "personas.", "report.")
_THINKING_OFF = {"type": "disabled"}

# Extra turns let the CLI re-prompt the model when its structured output fails schema validation.
_STRUCTURED_MAX_TURNS = 3
_REPAIR_SNIPPET_CHARS = 1500


def _sdk_supports_structured_output() -> bool:
    fields = getattr(ClaudeAgentOptions, "__dataclass_fields__", {})
    return "output_format" in fields and "structured_output" in getattr(ResultMessage, "__dataclass_fields__", {})


def _blanked_secret_env() -> dict[str, str]:
    """The SDK hands the CLI our whole environment and only lets options override it, so blank every engine
    secret: the bearer token, the Keychain keys (SHOAL_KEY_*) and ANTHROPIC_API_KEY / OPENAI_API_KEY. With
    ANTHROPIC_API_KEY set, Claude Code in -p mode would bill the API key instead of using the subscription login."""
    from shoal.runtimes.detect import is_secret_env  # lazy: shoal.runtimes imports this module

    names = {name for name in os.environ if is_secret_env(name)} | {"ANTHROPIC_API_KEY", "SHOAL_SERVE_TOKEN"}
    return dict.fromkeys(sorted(names), "")


class _Transient(Exception):
    """A failure worth retrying after a backoff (process crash, timeout, rate limit, 5xx)."""

    def __init__(self, message: str, *, rate_limited: bool = False):
        super().__init__(message)
        self.rate_limited = rate_limited


class _BadOutput(Exception):
    """The model answered, but not with something we can use; a repair prompt may fix it."""

    def __init__(self, message: str, *, reply_text: str = ""):
        super().__init__(message)
        self.reply_text = reply_text


@dataclass
class _Reply:
    text: str
    structured: Any = None
    turns: int = 0
    api_seconds: float = 0.0
    startup_seconds: float = 0.0  # spawn to first streamed message
    output_tokens: int = 0


@dataclass
class _CallRecord:
    purpose: str
    model: str
    prompt_chars: int
    mode: str
    started: float = field(default_factory=time.monotonic)
    attempts: int = 0
    output_chars: int = 0
    turns: int = 0
    api_seconds: float = 0.0
    startup_seconds: float = 0.0
    output_tokens: int = 0

    def add(self, reply: "_Reply") -> None:
        self.turns += reply.turns
        self.api_seconds += reply.api_seconds
        self.startup_seconds += reply.startup_seconds
        self.output_tokens += reply.output_tokens


def _classify_api_failure(message: str, status: int | None, terminal_reason: str | None) -> Exception:
    lowered = message.lower()
    if status in _AUTH_STATUS or (status is None and any(h in lowered for h in _AUTH_HINTS)):
        return LLMError(f"{_AUTH_HINT} ({message.strip()[:200]})")
    rate_limited = status == 429 or any(h in lowered for h in _RATE_HINTS)
    if rate_limited or status in _RETRYABLE_STATUS or terminal_reason == "api_error":
        return _Transient(message.strip()[:300], rate_limited=rate_limited)
    return _BadOutput(message.strip()[:300] or "the CLI reported an error result")


def _classify_sdk_error(err: BaseException) -> Exception:
    if isinstance(err, CLINotFoundError):
        return LLMError(f"Claude Code CLI not found ({err}). Install it and make sure `claude` is on PATH.")
    if isinstance(err, ResultError):
        text = " ".join([err.result or "", *err.errors]) or str(err)
        return _classify_api_failure(text, err.api_error_status, err.terminal_reason)
    return _Transient(f"{type(err).__name__}: {str(err)[:300]}")


_FENCE = re.compile(r"```(?:json|JSON)?\s*(.*?)```", re.DOTALL)


def extract_json(text: str, *, prefer: str = "{") -> Any:
    """Return the first JSON value embedded in `text` (fenced or bare), preferring `prefer` ('{' or '[')."""
    candidates = [m.group(1) for m in _FENCE.finditer(text)] + [text]
    decoder = _json.JSONDecoder()
    for opener in (prefer, "[" if prefer == "{" else "{"):
        for chunk in candidates:
            for match in re.finditer(re.escape(opener), chunk):
                try:
                    value, _ = decoder.raw_decode(chunk, match.start())
                except _json.JSONDecodeError:
                    continue
                return value
    raise ValueError("no JSON value found in the reply")


def _root_opener(json_schema: dict[str, Any]) -> str:
    return "[" if json_schema.get("type") == "array" else "{"


def _json_system(system: str, json_schema: dict[str, Any], *, structured: bool) -> str:
    if structured:
        contract = (
            "Deliver your final answer through the structured output tool. Every required field must be "
            "present, types must match the schema exactly, and no field may be invented."
        )
    else:
        contract = (
            "Your reply is parsed by a program. Respond with exactly one JSON value that validates against "
            "the JSON Schema below: no prose before or after it, no markdown code fences, no comments, "
            "no trailing commas. Every required field must be present and every type must match.\n\n"
            f"JSON Schema:\n{_json.dumps(json_schema, ensure_ascii=False)}"
        )
    return f"{system.rstrip()}\n\n# Output format\n{contract}"


def _repair_prompt(prompt: str, problem: str, previous: str) -> str:
    snippet = previous.strip()[:_REPAIR_SNIPPET_CHARS] or "(empty)"
    return (
        f"{prompt}\n\n---\nYour previous answer to this request could not be used.\n"
        f"Problem: {problem[:800]}\n"
        f"Previous answer (truncated):\n{snippet}\n\n"
        "Answer the original request again, fixing the problem. Output only the corrected JSON."
    )


class ClaudeLocal:
    """`LLM` backed by the local Claude Code CLI on the user's logged-in subscription (no API key)."""

    structured_output: bool = _sdk_supports_structured_output()
    backoff_base_s: float = 2.0
    rate_limit_backoff_s: float = 20.0

    def __init__(
        self,
        *,
        concurrency: int = 4,
        log_path: Path | None = None,
        cli_path: str | None = None,
        timeout_s: float = 240,
        retries: int = 2,
        think_purposes: tuple[str, ...] = DEFAULT_THINK_PURPOSES,
    ):
        self._semaphore = anyio.Semaphore(max(1, concurrency))
        self._log_path = Path(log_path) if log_path else None
        self._cli_path = cli_path or shutil.which("claude")
        self._timeout_s = timeout_s
        self._retries = max(0, retries)
        self._think_purposes = think_purposes

    async def text(self, prompt: str, *, system: str, model: str, purpose: str) -> str:
        record = _CallRecord(purpose, model, len(prompt) + len(system), mode="text")
        try:
            reply = await self._call_with_retries(prompt, self._options(system, model, None, purpose), record)
            if not reply.text.strip():
                raise _BadOutput("empty reply")
        except _BadOutput as err:
            self._log(record, err)
            raise LLMError(f"{purpose}: unusable reply from Claude: {err}") from err
        except BaseException as err:
            self._log(record, err)
            raise
        record.output_chars = len(reply.text)
        self._log(record, None)
        return reply.text.strip()

    async def json(self, prompt: str, *, system: str, model: str, purpose: str, schema: type[T]) -> T:
        json_schema = schema.model_json_schema()
        structured = self.structured_output
        record = _CallRecord(purpose, model, len(prompt) + len(system), mode="structured" if structured else "json")
        try:
            result = await self._json_with_repair(prompt, system, model, schema, json_schema, structured, record)
        except BaseException as err:
            self._log(record, err)
            raise
        self._log(record, None)
        return result

    async def _json_with_repair(
        self,
        prompt: str,
        system: str,
        model: str,
        schema: type[T],
        json_schema: dict[str, Any],
        structured: bool,
        record: _CallRecord,
    ) -> T:
        try:
            options = self._options(_json_system(system, json_schema, structured=structured), model,
                                    json_schema if structured else None, record.purpose)
            reply = await self._call_with_retries(prompt, options, record)
            return self._parse(reply, schema, json_schema, record)
        except _BadOutput as first:
            problem, previous = str(first), first.reply_text
        # One repair attempt in plain-JSON mode: it also covers a CLI that rejects the structured schema.
        try:
            options = self._options(_json_system(system, json_schema, structured=False), model, None, record.purpose)
            reply = await self._call_with_retries(_repair_prompt(prompt, problem, previous), options, record)
            return self._parse(reply, schema, json_schema, record)
        except _BadOutput as second:
            raise LLMError(f"{record.purpose}: invalid JSON after repair: {second}") from second

    @staticmethod
    def _parse(reply: _Reply, schema: type[T], json_schema: dict[str, Any], record: _CallRecord) -> T:
        structured_chars = len(_json.dumps(reply.structured, default=str)) if reply.structured is not None else 0
        record.output_chars = structured_chars or len(reply.text)
        try:
            value = reply.structured
            if isinstance(value, str):
                value = extract_json(value, prefer=_root_opener(json_schema))
            if value is None:
                value = extract_json(reply.text, prefer=_root_opener(json_schema))
            return schema.model_validate(value)
        except ValueError as err:  # includes pydantic.ValidationError
            raise _BadOutput(str(err), reply_text=reply.text or _json.dumps(reply.structured, default=str)) from err

    def _options(
        self, system: str, model: str, json_schema: dict[str, Any] | None, purpose: str
    ) -> ClaudeAgentOptions:
        extra: dict[str, Any] = {}
        if json_schema is not None:
            extra["output_format"] = {"type": "json_schema", "schema": json_schema}
        if not purpose.startswith(self._think_purposes):
            extra["thinking"] = _THINKING_OFF
        return ClaudeAgentOptions(
            system_prompt=system,
            model=model,
            max_turns=_STRUCTURED_MAX_TURNS if json_schema is not None else 1,
            tools=[],
            allowed_tools=[],
            setting_sources=[],
            strict_mcp_config=True,  # never start the user's own MCP servers for simulation calls
            verbatim_prompts=True,  # seed text may contain "@path" or "/command"; never expand it
            cli_path=self._cli_path,
            env=_blanked_secret_env(),
            **extra,
        )

    async def _call_with_retries(self, prompt: str, options: ClaudeAgentOptions, record: _CallRecord) -> _Reply:
        for attempt in range(self._retries + 1):
            record.attempts += 1
            try:
                async with self._semaphore:
                    with anyio.fail_after(self._timeout_s):
                        reply = await self._run_once(prompt, options)
                record.add(reply)
                return reply
            except TimeoutError:
                failure: _Transient = _Transient(f"timed out after {self._timeout_s:.0f}s")
            except _Transient as err:
                failure = err
            if attempt == self._retries:
                raise LLMError(f"{record.purpose}: failed after {attempt + 1} attempts: {failure}") from failure
            await anyio.sleep(self._backoff(attempt, failure.rate_limited))
        raise AssertionError("unreachable")

    def _backoff(self, attempt: int, rate_limited: bool) -> float:
        base = self.rate_limit_backoff_s if rate_limited else self.backoff_base_s
        return base * (2**attempt) * random.uniform(0.8, 1.2)

    async def _run_once(self, prompt: str, options: ClaudeAgentOptions) -> _Reply:
        texts: list[str] = []
        final: ResultMessage | None = None
        started, first_message = time.monotonic(), None
        try:
            async with aclosing(claude_agent_sdk.query(prompt=prompt, options=options)) as stream:
                async for message in stream:
                    first_message = first_message or time.monotonic()
                    if isinstance(message, AssistantMessage) and message.parent_tool_use_id is None:
                        texts.extend(b.text for b in message.content if isinstance(b, TextBlock))
                    elif isinstance(message, ResultMessage):
                        final = message
                        break
        except ClaudeSDKError as err:
            raise _classify_sdk_error(err) from err
        except Exception as err:  # the SDK also raises bare Exceptions (e.g. a slow CLI start-up handshake)
            raise _Transient(f"{type(err).__name__}: {str(err)[:300]}") from err
        if final is None:
            raise _Transient("the CLI stream ended without a result")
        reply = self._from_result(final, texts)
        reply.turns = final.num_turns or 0
        reply.api_seconds = (final.duration_api_ms or 0) / 1000
        reply.startup_seconds = (first_message or started) - started
        reply.output_tokens = int((final.usage or {}).get("output_tokens") or 0)
        return reply

    @staticmethod
    def _from_result(result: ResultMessage, texts: list[str]) -> _Reply:
        text = result.result if isinstance(result.result, str) and result.result.strip() else "\n".join(texts)
        if result.is_error:
            detail = " ".join([text, *(result.errors or [])]).strip() or result.subtype
            raise _classify_api_failure(detail, result.api_error_status, result.terminal_reason)
        return _Reply(text=text, structured=getattr(result, "structured_output", None))

    def _log(self, record: _CallRecord, error: BaseException | None) -> None:
        if self._log_path is None:
            return
        entry = {
            "ts": datetime.now(timezone.utc).isoformat(timespec="seconds"),
            "purpose": record.purpose,
            "model": record.model,
            "mode": record.mode,
            "seconds": round(time.monotonic() - record.started, 3),
            "attempts": record.attempts,
            "ok": error is None,
            "error": None if error is None else f"{type(error).__name__}: {str(error)[:300]}",
            "prompt_chars": record.prompt_chars,
            "output_chars": record.output_chars,
            "turns": record.turns,
            "api_seconds": round(record.api_seconds, 2),
            "startup_seconds": round(record.startup_seconds, 2),
            "output_tokens": record.output_tokens,
        }
        self._log_path.parent.mkdir(parents=True, exist_ok=True)
        with self._log_path.open("a", encoding="utf-8") as fh:
            fh.write(_json.dumps(entry, ensure_ascii=False) + "\n")
