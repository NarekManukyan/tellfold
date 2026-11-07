"""ClaudeLocal logic, with the SDK's query() replaced by a scripted fake (no subprocess)."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import anyio
import claude_agent_sdk
import pytest
from claude_agent_sdk import (
    AssistantMessage,
    CLINotFoundError,
    ProcessError,
    ResultError,
    ResultMessage,
    TextBlock,
)
from pydantic import BaseModel

from shoal.llm import ClaudeLocal, LLMError, extract_json
from shoal.models import Report


class Pick(BaseModel):
    color: str
    n: int


def result(text: str = "", *, structured: Any = None, is_error: bool = False, status: int | None = None,
           reason: str | None = None) -> ResultMessage:
    return ResultMessage(
        subtype="success", duration_ms=5, duration_api_ms=4, is_error=is_error, num_turns=1, session_id="s",
        result=text, structured_output=structured, api_error_status=status, terminal_reason=reason,
    )


def assistant(text: str) -> AssistantMessage:
    return AssistantMessage(content=[TextBlock(text=text)], model="haiku")


class ScriptedQuery:
    """Stands in for claude_agent_sdk.query. Each call consumes one script entry:
    a list of messages to yield, or an exception to raise."""

    def __init__(self, *script: list[Any] | BaseException):
        self.script = list(script)
        self.calls: list[tuple[str, Any]] = []

    def __call__(self, *, prompt: str, options: Any):
        self.calls.append((prompt, options))
        step = self.script.pop(0)

        async def stream():
            if isinstance(step, BaseException):
                raise step
            for message in step:
                yield message

        return stream()


@pytest.fixture
def scripted(monkeypatch: pytest.MonkeyPatch):
    def install(*script: list[Any] | BaseException) -> ScriptedQuery:
        fake = ScriptedQuery(*script)
        monkeypatch.setattr(claude_agent_sdk, "query", fake)
        return fake

    return install


@pytest.fixture
def llm(tmp_path: Path) -> ClaudeLocal:
    client = ClaudeLocal(concurrency=2, log_path=tmp_path / "calls.jsonl", cli_path="/bin/claude-test", retries=2)
    client.backoff_base_s = 0
    client.rate_limit_backoff_s = 0
    return client


# ---------- text() ----------

async def test_text_uses_isolated_options(llm, scripted):
    fake = scripted([assistant("draft"), result("hello")])
    out = await llm.text("Say hi", system="Be terse.", model="haiku", purpose="t.basic")
    assert out == "hello"
    prompt, opts = fake.calls[0]
    assert prompt == "Say hi"
    assert opts.system_prompt == "Be terse."
    assert opts.model == "haiku"
    assert opts.max_turns == 1
    assert opts.tools == [] and opts.allowed_tools == [] and opts.setting_sources == []
    assert opts.cli_path == "/bin/claude-test"
    assert opts.output_format is None


async def test_thinking_is_off_except_for_think_purposes(tmp_path, scripted):
    client = ClaudeLocal(cli_path="/bin/claude-test", think_purposes=("report.",))
    fake = scripted([result("a")], [result("b")], [result(structured={"color": "red", "n": 1})])
    await client.text("x", system="s", model="haiku", purpose="engine.round")
    await client.text("x", system="s", model="sonnet", purpose="report.write")
    await client.json("x", system="s", model="haiku", purpose="graph.extract", schema=Pick)
    assert fake.calls[0][1].thinking == {"type": "disabled"}
    assert fake.calls[1][1].thinking is None
    assert fake.calls[2][1].thinking == {"type": "disabled"}


async def test_call_log_records_turns_and_tokens(llm, scripted, tmp_path):
    msg = result(structured={"color": "red", "n": 1})
    msg.num_turns, msg.duration_api_ms, msg.usage = 2, 1500, {"output_tokens": 42}
    scripted([msg])
    await llm.json("x", system="s", model="haiku", purpose="t.log", schema=Pick)
    entry = json.loads((tmp_path / "calls.jsonl").read_text().splitlines()[-1])
    assert (entry["turns"], entry["api_seconds"], entry["output_tokens"]) == (2, 1.5, 42)


async def test_text_falls_back_to_assistant_blocks(llm, scripted):
    scripted([assistant("part one"), assistant("part two"), result("")])
    assert await llm.text("x", system="s", model="sonnet", purpose="t.blocks") == "part one\npart two"


async def test_text_empty_reply_is_llm_error(llm, scripted):
    scripted([result("   ")])
    with pytest.raises(LLMError, match="empty reply"):
        await llm.text("x", system="s", model="haiku", purpose="t.empty")


# ---------- json(): structured output ----------

async def test_json_structured_output(llm, scripted):
    fake = scripted([result("", structured={"color": "teal", "n": 4})])
    pick = await llm.json("Pick", system="Sys", model="haiku", purpose="j.struct", schema=Pick)
    assert pick == Pick(color="teal", n=4)
    opts = fake.calls[0][1]
    assert opts.output_format == {"type": "json_schema", "schema": Pick.model_json_schema()}
    assert opts.max_turns > 1
    assert opts.system_prompt.startswith("Sys")


async def test_json_structured_output_as_string(llm, scripted):
    scripted([result("", structured='{"color": "red", "n": 1}')])
    assert await llm.json("p", system="s", model="haiku", purpose="j.str", schema=Pick) == Pick(color="red", n=1)


async def test_json_nested_schema_round_trips(llm, scripted):
    report = {"title": "T", "summary": "S", "prediction": "P", "confidence": "medium",
              "sections": [{"title": "a", "body_markdown": "b"}]}
    scripted([result("", structured=report)])
    out = await llm.json("p", system="s", model="sonnet", purpose="j.report", schema=Report)
    assert out.sections[0].title == "a"


# ---------- json(): text extraction mode ----------

async def test_json_text_mode_extracts_from_prose_and_fences(llm, scripted):
    llm.structured_output = False
    reply = 'Sure! Here it is:\n```json\n{"color": "blue", "n": 7}\n```\nAnything else?'
    fake = scripted([result(reply)])
    assert await llm.json("p", system="s", model="haiku", purpose="j.text", schema=Pick) == Pick(color="blue", n=7)
    opts = fake.calls[0][1]
    assert opts.output_format is None
    assert '"color"' in opts.system_prompt  # schema is spelled out when the SDK cannot enforce it


async def test_json_repair_retry_after_invalid_output(llm, scripted):
    fake = scripted(
        [result("", structured={"color": "green", "n": "many"})],
        [result('{"color": "green", "n": 3}')],
    )
    assert await llm.json("Pick one", system="s", model="haiku", purpose="j.repair", schema=Pick) == Pick(
        color="green", n=3
    )
    repair_prompt, repair_opts = fake.calls[1]
    assert repair_prompt.startswith("Pick one")
    assert "could not be used" in repair_prompt and "many" in repair_prompt
    assert repair_opts.output_format is None


async def test_json_gives_up_after_one_repair(llm, scripted):
    fake = scripted([result("no json here")], [result('{"color": 5}')])
    with pytest.raises(LLMError, match="after repair"):
        await llm.json("p", system="s", model="haiku", purpose="j.bad", schema=Pick)
    assert len(fake.calls) == 2


async def test_json_non_api_error_result_goes_to_repair(llm, scripted):
    scripted([result("structured output retries exhausted", is_error=True)], [result('{"color": "x", "n": 0}')])
    assert await llm.json("p", system="s", model="haiku", purpose="j.maxturns", schema=Pick) == Pick(color="x", n=0)


# ---------- retries, errors, timeout ----------

async def test_retries_process_errors_then_succeeds(llm, scripted):
    fake = scripted(ProcessError("boom", exit_code=1), ProcessError("boom", exit_code=1), [result("ok")])
    assert await llm.text("x", system="s", model="haiku", purpose="r.ok") == "ok"
    assert len(fake.calls) == 3


async def test_retries_exhausted_raise_llm_error(llm, scripted):
    scripted(*[ProcessError("boom", exit_code=1)] * 3)
    with pytest.raises(LLMError, match="failed after 3 attempts"):
        await llm.text("x", system="s", model="haiku", purpose="r.fail")


async def test_plain_sdk_exception_is_retried(llm, scripted):
    fake = scripted(Exception("Control request timeout: initialize"), [result("ok")])
    assert await llm.text("x", system="s", model="haiku", purpose="r.plain") == "ok"
    assert len(fake.calls) == 2


async def test_plain_sdk_exception_exhausted_becomes_llm_error(llm, scripted):
    scripted(*[RuntimeError("reader died")] * 3)
    with pytest.raises(LLMError, match="RuntimeError: reader died"):
        await llm.text("x", system="s", model="haiku", purpose="r.plain")


async def test_rate_limit_uses_rate_limit_backoff(llm, scripted, monkeypatch):
    seen: list[bool] = []
    monkeypatch.setattr(llm, "_backoff", lambda attempt, rate_limited: seen.append(rate_limited) or 0)
    scripted([result("API Error: 429", is_error=True, status=429, reason="api_error")], [result("ok")])
    assert await llm.text("x", system="s", model="haiku", purpose="r.429") == "ok"
    assert seen == [True]


async def test_overloaded_sdk_result_error_is_retried(llm, scripted):
    err = ResultError("api failed", data={"result": "Overloaded", "api_error_status": 529,
                                          "terminal_reason": "api_error"}, exit_code=1)
    fake = scripted(err, [result("fine")])
    assert await llm.text("x", system="s", model="haiku", purpose="r.529") == "fine"
    assert len(fake.calls) == 2


async def test_auth_failure_is_not_retried(llm, scripted):
    fake = scripted([result("Failed to authenticate: OAuth session expired", is_error=True, reason="api_error")])
    with pytest.raises(LLMError, match="/login"):
        await llm.text("x", system="s", model="haiku", purpose="r.auth")
    assert len(fake.calls) == 1


async def test_missing_cli_is_not_retried(llm, scripted):
    fake = scripted(CLINotFoundError("Claude Code not found"))
    with pytest.raises(LLMError, match="not found"):
        await llm.text("x", system="s", model="haiku", purpose="r.cli")
    assert len(fake.calls) == 1


async def test_timeout_per_call(tmp_path, monkeypatch):
    def hanging(*, prompt, options):
        async def stream():
            await anyio.sleep(10)
            yield result("late")

        return stream()

    monkeypatch.setattr(claude_agent_sdk, "query", hanging)
    client = ClaudeLocal(timeout_s=0.05, retries=0)
    with pytest.raises(LLMError, match="timed out"):
        await client.text("x", system="s", model="haiku", purpose="r.timeout")


async def test_concurrency_is_bounded(monkeypatch):
    active, peak = 0, 0

    def slow(*, prompt, options):
        async def stream():
            nonlocal active, peak
            active += 1
            peak = max(peak, active)
            await anyio.sleep(0.02)
            active -= 1
            yield result(prompt)

        return stream()

    monkeypatch.setattr(claude_agent_sdk, "query", slow)
    client = ClaudeLocal(concurrency=2)
    async with anyio.create_task_group() as tg:
        for i in range(6):
            tg.start_soon(lambda i=i: client.text(f"p{i}", system="s", model="haiku", purpose="c"))
    assert peak == 2


# ---------- call log ----------

async def test_call_log_records_lengths_not_prompts(llm, scripted, tmp_path):
    scripted([result("answer")], ProcessError("x", exit_code=1), ProcessError("x", exit_code=1),
             ProcessError("x", exit_code=1))
    await llm.text("SECRET-PROMPT-TEXT", system="SECRET-SYSTEM", model="haiku", purpose="log.ok")
    with pytest.raises(LLMError):
        await llm.text("SECRET-PROMPT-TEXT", system="s", model="sonnet", purpose="log.fail")

    raw = (tmp_path / "calls.jsonl").read_text()
    assert "SECRET" not in raw
    ok, failed = [json.loads(line) for line in raw.splitlines()]
    assert {"ts", "purpose", "model", "seconds", "ok", "error", "prompt_chars", "output_chars"} <= ok.keys()
    assert ok["ok"] is True and ok["error"] is None
    assert ok["prompt_chars"] == len("SECRET-PROMPT-TEXT") + len("SECRET-SYSTEM")
    assert ok["output_chars"] == len("answer")
    assert failed["ok"] is False and failed["attempts"] == 3 and failed["model"] == "sonnet"


# ---------- extract_json ----------

@pytest.mark.parametrize(
    "text, expected",
    [
        ('{"a": 1}', {"a": 1}),
        ('Here you go: {"a": {"b": [1, 2]}} done', {"a": {"b": [1, 2]}}),
        ('```json\n{"a": 2}\n```', {"a": 2}),
        ('noise {not json} then {"a": 3}', {"a": 3}),
        ('[1, 2] and {"a": 4}', {"a": 4}),
    ],
)
def test_extract_json(text, expected):
    assert extract_json(text) == expected


def test_extract_json_prefers_arrays_when_asked():
    assert extract_json('{"a": 1} [1, 2]', prefer="[") == [1, 2]


def test_extract_json_without_json_raises():
    with pytest.raises(ValueError, match="no JSON"):
        extract_json("just words")


# ---------- shared conftest fixture ----------

async def test_fake_llm_factory_routes_by_purpose(fake_llm):
    llm = fake_llm(
        {"engine": [{"color": "a", "n": 1}, {"color": "b", "n": 2}], "chat.ask": lambda prompt, schema: prompt.upper()},
        default="fallback",
    )
    assert (await llm.json("p", system="s", model="m", purpose="engine.round", schema=Pick)).color == "a"
    assert (await llm.json("p", system="s", model="m", purpose="engine.round", schema=Pick)).color == "b"
    assert (await llm.json("p", system="s", model="m", purpose="engine.round", schema=Pick)).color == "b"
    assert await llm.text("hi", system="s", model="m", purpose="chat.ask") == "HI"
    assert await llm.text("hi", system="s", model="m", purpose="report.write") == "fallback"
    assert [c[0] for c in llm.calls][-2:] == ["chat.ask", "report.write"]
