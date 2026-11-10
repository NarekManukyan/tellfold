"""Agent runtimes: settings, detection, the shared adapter machinery, CLI and HTTP adapters, routing.

No test spawns a real agent CLI or calls a remote API: CLI adapters get a fake process runner, HTTP adapters an
`httpx.MockTransport`. Detection runs against tiny shell scripts in a temp folder.
"""

from __future__ import annotations

import asyncio
import json
import os
import signal
import stat
import sys
import time
from pathlib import Path
from typing import Any

import anyio
import httpx
import pytest
from pydantic import BaseModel

from tellfold.llm import ClaudeLocal, LLMError
from tellfold.models import SimConfig
from tellfold.runtimes import (
    RoutedLLM,
    UnknownRuntime,
    build_llm,
    detect_all,
    get_runtime,
    is_agent_purpose,
    known_ids,
    test_runtime,
)
from tellfold.runtimes import cli_tools, http_api, registry
from tellfold.runtimes.base import AdapterLLM, AdapterReply, AdapterRequest, BadOutput, Fatal, Transient
from tellfold.runtimes.cli_tools import ProcessResult, parse_json_reply
from tellfold.runtimes.detect import child_env, find_binary, parse_version
from tellfold.settings import OpenAIEndpoint, Settings, key_env_for, load_settings, save_settings


class Pick(BaseModel):
    color: str
    n: int


SECRET = "sk-test-0123456789abcdef"


@pytest.fixture(autouse=True)
def isolated_home(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """Settings, HOME and PATH point into tmp_path, so detection never sees the real tools."""
    home = tmp_path / "home"
    home.mkdir()
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    monkeypatch.setenv("TELLFOLD_HOME", str(tmp_path / "tellfold-home"))
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("PATH", str(bin_dir))
    for name in ("ANTHROPIC_API_KEY", "OPENAI_API_KEY", "CURSOR_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY",
                 "GROK_API_KEY", "FACTORY_API_KEY", "GH_TOKEN", "GITHUB_TOKEN", "COPILOT_GITHUB_TOKEN",
                 "NPM_CONFIG_PREFIX", "npm_config_prefix", "TELLFOLD_KEY_LOCAL"):
        monkeypatch.delenv(name, raising=False)
    # the real machine's Homebrew / /usr/local folders must not leak in
    monkeypatch.setattr("tellfold.runtimes.detect.extra_bin_dirs", lambda: [home / ".local" / "bin"])
    return tmp_path


def make_script(folder: Path, name: str, body: str) -> Path:
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / name
    path.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return path


def run(coro_fn, *args, **kwargs):
    return anyio.run(lambda: coro_fn(*args, **kwargs))


# ---------- settings ----------


def test_settings_defaults_roundtrip_and_location(isolated_home: Path) -> None:
    assert load_settings() == Settings()  # missing file -> defaults
    settings = Settings(agent_runtime="ollama", agent_model="llama3.2:3b", openai_endpoints=[
        OpenAIEndpoint(id="LMStudio", name="LM Studio", base_url="http://127.0.0.1:1234/v1/", models=["qwen/qwen3-8b"])])
    path = save_settings(settings)
    assert path == isolated_home / "tellfold-home" / "settings.json"
    loaded = load_settings()
    assert loaded == settings
    assert loaded.openai_endpoints[0].id == "lmstudio"
    assert loaded.openai_endpoints[0].base_url == "http://127.0.0.1:1234/v1"
    assert loaded.openai_endpoints[0].runtime_id == "openai-compatible:lmstudio"
    assert loaded.openai_endpoints[0].api_key_env == "TELLFOLD_KEY_LMSTUDIO"
    path.write_text("{not json", encoding="utf-8")
    assert load_settings() == Settings()  # corrupt file -> defaults, never raises


def test_settings_reject_secrets_and_bad_urls() -> None:
    with pytest.raises(ValueError):
        OpenAIEndpoint(id="x", name="X", base_url="http://h/v1", key_env=SECRET + "!")
    with pytest.raises(ValueError):
        OpenAIEndpoint(id="x", name="X", base_url="ftp://h/v1")
    with pytest.raises(ValueError):
        OpenAIEndpoint(id="x", name="X", base_url="http://user:pw@h/v1")
    with pytest.raises(ValueError):
        Settings(openai_endpoints=[OpenAIEndpoint(id="a", name="A", base_url="http://h"),
                                   OpenAIEndpoint(id="a", name="B", base_url="http://h")])
    assert key_env_for("open-router.ai") == "TELLFOLD_KEY_OPEN_ROUTER_AI"


def test_simconfig_runtime_fields_default_for_old_configs() -> None:
    old = SimConfig.model_validate({"question": "q", "seed_files": []})
    assert old.agent_runtime == old.planner_runtime == "claude-code"


# ---------- detection ----------


def test_find_binary_searches_extra_dirs_and_child_env_puts_them_on_path(isolated_home: Path,
                                                                        monkeypatch: pytest.MonkeyPatch) -> None:
    local_bin = isolated_home / "home" / ".local" / "bin"
    tool = make_script(local_bin, "codex", 'echo "codex-cli 0.9.1"')
    monkeypatch.setenv("PATH", "/nonexistent")
    assert find_binary("codex") == str(tool)
    assert child_env(str(tool))["PATH"].split(os.pathsep)[0] == str(local_bin)
    assert parse_version("codex-cli 0.9.1\n") == "0.9.1"
    assert parse_version("2026.01.23-916f423") == "2026.01.23-916f423"


def test_detect_cli_runtime_version_and_auth(isolated_home: Path) -> None:
    bin_dir = isolated_home / "bin"
    make_script(bin_dir, "cursor-agent", 'if [ "$1" = "--version" ]; then echo 2026.01.23-abc; '
                                         'elif [ "$1" = "status" ]; then echo " Not logged in"; fi')
    make_script(bin_dir, "claude", 'if [ "$1" = "--version" ]; then echo "2.1.283 (Claude Code)"; '
                                   'else echo \'{"loggedIn": true, "email": "x@y"}\'; fi')
    infos = {i.id: i for i in detect_all(Settings(), check_auth=True)}
    cursor = infos["cursor-agent"]
    assert cursor.installed and cursor.version == "2026.01.23-abc" and cursor.auth == "missing"
    assert "cursor-agent login" in (cursor.auth_hint or "")
    assert not cursor.tested
    claude = infos["claude-code"]
    assert claude.installed and claude.version == "2.1.283" and claude.auth == "ok" and claude.tested
    assert claude.default_models == {"agent": "haiku", "planner": "sonnet"}
    assert not infos["codex"].installed and infos["codex"].path is None
    # without --check-auth no auth command runs
    quick = {i.id: i for i in detect_all(Settings())}
    assert quick["cursor-agent"].auth == "unknown" and quick["claude-code"].auth == "unknown"


def test_detect_all_covers_every_runtime_and_survives_unreachable_servers(monkeypatch: pytest.MonkeyPatch) -> None:
    def refuse(request: httpx.Request) -> httpx.Response:
        raise httpx.ConnectError("refused", request=request)

    monkeypatch.setattr(registry.HTTPRuntime, "sync_transport", httpx.MockTransport(refuse))
    settings = Settings(openai_endpoints=[OpenAIEndpoint(id="lmstudio", name="LM Studio",
                                                         base_url="http://127.0.0.1:1234/v1", models=["m1"])])
    infos = detect_all(settings)
    ids = [i.id for i in infos]
    assert ids == ["claude-code", "cursor-agent", "codex", "gemini-cli", "opencode", "qwen-code", "copilot", "grok",
                   "droid", "ollama", "openai-compatible:lmstudio", "anthropic-api", "openai-api"]
    assert ids == known_ids(settings)
    lm = infos[ids.index("openai-compatible:lmstudio")]
    assert not lm.installed and lm.models == ["m1"] and lm.default_models["agent"] == "m1"
    assert all(not i.installed for i in infos)
    assert infos[ids.index("anthropic-api")].auth == "missing"


def test_detect_ollama_and_openai_compatible_with_mock_servers(monkeypatch: pytest.MonkeyPatch) -> None:
    seen: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        seen.append(request)
        if request.url.path == "/api/tags":
            return httpx.Response(200, json={"models": [{"name": "llama3.2:3b"}, {"name": "qwen3:8b"}]})
        if request.url.path == "/api/version":
            return httpx.Response(200, json={"version": "0.12.0"})
        if request.url.path == "/v1/models":
            if request.headers.get("authorization") != f"Bearer {SECRET}":
                return httpx.Response(401, json={"error": "no key"})
            return httpx.Response(200, json={"data": [{"id": "deepseek-chat"}]})
        return httpx.Response(404)

    monkeypatch.setattr(registry.HTTPRuntime, "sync_transport", httpx.MockTransport(handler))
    settings = Settings(planner_runtime="ollama", planner_model="qwen3:8b", openai_endpoints=[
        OpenAIEndpoint(id="deepseek", name="DeepSeek", base_url="https://api.deepseek.example/v1")])
    infos = {i.id: i for i in detect_all(settings)}
    ollama = infos["ollama"]
    assert ollama.installed and ollama.version == "0.12.0" and ollama.models == ["llama3.2:3b", "qwen3:8b"]
    assert ollama.default_models == {"agent": "llama3.2:3b", "planner": "qwen3:8b"}
    assert infos["openai-compatible:deepseek"].auth == "missing"
    monkeypatch.setenv("TELLFOLD_KEY_DEEPSEEK", SECRET)
    ds = get_runtime("openai-compatible:deepseek", settings).detect()
    assert ds.installed and ds.auth == "ok" and ds.models == ["deepseek-chat"]
    assert SECRET not in ds.model_dump_json()


def test_keyed_api_runtimes_check_auth_only_when_asked(monkeypatch: pytest.MonkeyPatch) -> None:
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        return httpx.Response(200, json={"data": [{"id": "claude-haiku-5-5"}]})

    monkeypatch.setattr(registry.HTTPRuntime, "sync_transport", httpx.MockTransport(handler))
    monkeypatch.setenv("ANTHROPIC_API_KEY", SECRET)
    rt = get_runtime("anthropic-api", Settings())
    assert rt.detect().auth == "unknown" and not calls
    info = rt.detect(check_auth=True)
    assert info.auth == "ok" and info.installed
    assert calls[0].headers["x-api-key"] == SECRET and calls[0].headers["anthropic-version"]


def test_get_runtime_unknown_ids() -> None:
    with pytest.raises(UnknownRuntime):
        get_runtime("nope", Settings())
    with pytest.raises(UnknownRuntime):
        get_runtime("openai-compatible:missing", Settings())


# ---------- the shared adapter machinery ----------


class Scripted(AdapterLLM):
    runtime_id = "scripted"
    backoff_base_s = 0.0
    rate_limit_backoff_s = 0.0

    def __init__(self, *script: Any, **kw: Any):
        super().__init__(**kw)
        self.script = list(script)
        self.requests: list[AdapterRequest] = []

    async def _send(self, request: AdapterRequest) -> AdapterReply:
        self.requests.append(request)
        step = self.script.pop(0)
        if isinstance(step, BaseException):
            raise step
        if callable(step):
            return await step()
        return step if isinstance(step, AdapterReply) else AdapterReply(text=step)


def test_text_retries_transient_then_succeeds_and_logs_no_prompt(tmp_path: Path) -> None:
    log = tmp_path / "calls.jsonl"
    llm = Scripted(Transient("boom"), Transient("slow down", rate_limited=True), "  hi  ", log_path=log)
    assert run(llm.text, "SECRET PROMPT TEXT", system="sys", model="m", purpose="chat.ask") == "hi"
    entry = json.loads(log.read_text().strip())
    assert entry["ok"] and entry["attempts"] == 3 and entry["runtime"] == "scripted"
    assert entry["prompt_chars"] == len("SECRET PROMPT TEXT") + 3
    assert "SECRET PROMPT" not in log.read_text()


def test_fatal_is_not_retried_and_only_llmerror_escapes(tmp_path: Path) -> None:
    llm = Scripted(Fatal("not logged in"), "never", log_path=tmp_path / "c.jsonl")
    with pytest.raises(LLMError, match="not logged in"):
        run(llm.text, "p", system="s", model="m", purpose="x")
    assert len(llm.requests) == 1
    llm = Scripted(KeyError("adapter bug"))
    with pytest.raises(LLMError, match="KeyError"):
        run(llm.text, "p", system="s", model="m", purpose="x")
    llm = Scripted(Transient("a"), Transient("b"), Transient("c"))
    with pytest.raises(LLMError, match="after 3 attempts"):
        run(llm.text, "p", system="s", model="m", purpose="x")
    with pytest.raises(LLMError, match="unusable"):
        run(Scripted("   ").text, "p", system="s", model="m", purpose="x")


def test_json_validates_repairs_once_then_fails() -> None:
    llm = Scripted('Sure! ```json\n{"color": "teal", "n": 3}\n```')
    assert run(llm.json, "p", system="s", model="m", purpose="x", schema=Pick) == Pick(color="teal", n=3)
    assert "JSON Schema" in llm.requests[0].system and llm.requests[0].json_schema is not None

    llm = Scripted('{"color": "teal"}', '{"color": "teal", "n": 1}')
    assert run(llm.json, "p", system="s", model="m", purpose="x", schema=Pick).n == 1
    assert "could not be used" in llm.requests[1].prompt and '"color": "teal"' in llm.requests[1].prompt

    llm = Scripted("no json", "still none", "unused")
    with pytest.raises(LLMError, match="after repair"):
        run(llm.json, "p", system="s", model="m", purpose="x", schema=Pick)
    assert len(llm.requests) == 2

    structured = Scripted(AdapterReply(structured={"color": "red", "n": 2}))
    assert run(structured.json, "p", system="s", model="m", purpose="x", schema=Pick).color == "red"


def test_timeout_and_concurrency_semaphore() -> None:
    async def slow() -> AdapterReply:
        await anyio.sleep(5)
        return AdapterReply(text="late")

    llm = Scripted(slow, timeout_s=0.05, retries=0)
    with pytest.raises(LLMError, match="timed out"):
        run(llm.text, "p", system="s", model="m", purpose="x")

    active = peak = 0

    async def tracked() -> AdapterReply:
        nonlocal active, peak
        active += 1
        peak = max(peak, active)
        await anyio.sleep(0.02)
        active -= 1
        return AdapterReply(text="ok")

    llm = Scripted(*[tracked] * 6, concurrency=2)

    async def many() -> None:
        async with anyio.create_task_group() as tg:
            for _ in range(6):
                tg.start_soon(lambda: llm.text("p", system="s", model="m", purpose="x"))

    anyio.run(many)
    assert peak == 2


# ---------- CLI adapters (fake process runner) ----------


class FakeRunner:
    def __init__(self, *results: ProcessResult | Any):
        self.results = list(results)
        self.calls: list[dict[str, Any]] = []

    async def __call__(self, argv: list[str], *, stdin: str | None, cwd: str, env: dict[str, str]) -> ProcessResult:
        self.calls.append({"argv": argv, "stdin": stdin, "cwd": cwd, "env": env,
                           "cwd_was_empty": not any(Path(cwd).iterdir())})
        step = self.results.pop(0)
        return step(argv, cwd) if callable(step) else step


@pytest.fixture
def fake_bin(tmp_path: Path) -> str:
    path = tmp_path / "tool"
    path.write_text("", encoding="utf-8")
    return str(path)


def test_cursor_agent_argv_isolation_and_json_parsing(fake_bin: str) -> None:
    out = json.dumps({"type": "result", "subtype": "success", "is_error": False, "result": '{"color":"x","n":1}',
                      "session_id": "s"})
    runner = FakeRunner(ProcessResult(0, out, ""))
    llm = cli_tools.CursorAgentLLM(fake_bin, runner=runner)
    pick = run(llm.json, "the prompt", system="be brief", model="gpt-5", purpose="engine.round", schema=Pick)
    assert pick == Pick(color="x", n=1)
    call = runner.calls[0]
    argv = call["argv"]
    assert argv[:3] == [fake_bin, "-p", "--output-format"]
    assert argv[argv.index("--mode") + 1] == "ask" and argv[argv.index("--sandbox") + 1] == "enabled"
    assert argv[argv.index("--workspace") + 1] == call["cwd"] and call["cwd_was_empty"]
    assert "--trust" in argv  # headless calls fail on the workspace-trust prompt without it
    assert argv[argv.index("--model") + 1] == "gpt-5"
    assert "--force" not in argv and "--approve-mcps" not in argv and "-f" not in argv
    assert "be brief" in argv[-1] and "the prompt" in argv[-1] and not argv[-1].startswith("-")
    assert not Path(call["cwd"]).exists()  # the scratch folder is removed afterwards


def test_cursor_agent_tool_default_model_and_auth_failure(fake_bin: str) -> None:
    runner = FakeRunner(ProcessResult(1, "", "Error: Authentication required. Please run 'agent login' first."))
    llm = cli_tools.CursorAgentLLM(fake_bin, runner=runner)
    with pytest.raises(LLMError, match="cursor-agent login"):
        run(llm.text, "p", system="s", model="default", purpose="x")
    assert len(runner.calls) == 1 and "--model" not in runner.calls[0]["argv"]


def test_cli_rate_limit_is_retried_and_crash_is_transient(fake_bin: str) -> None:
    ok = json.dumps({"type": "result", "is_error": False, "result": "fine"})
    runner = FakeRunner(ProcessResult(1, "", "429 Too Many Requests"), ProcessResult(137, "", ""),
                        ProcessResult(0, ok, ""))
    llm = cli_tools.CursorAgentLLM(fake_bin, runner=runner)
    llm.backoff_base_s = llm.rate_limit_backoff_s = 0
    assert run(llm.text, "p", system="s", model="", purpose="x") == "fine"
    assert len(runner.calls) == 3


def test_cli_missing_binary_is_fatal(tmp_path: Path) -> None:
    llm = cli_tools.CursorAgentLLM(str(tmp_path / "nope"), runner=FakeRunner())
    with pytest.raises(LLMError, match="not installed"):
        run(llm.text, "p", system="s", model="", purpose="x")


def test_codex_prompt_on_stdin_and_last_message_file(fake_bin: str) -> None:
    def respond(argv: list[str], cwd: str) -> ProcessResult:
        Path(argv[argv.index("--output-last-message") + 1]).write_text("final answer\n", encoding="utf-8")
        return ProcessResult(0, "lots of progress chatter", "")

    runner = FakeRunner(respond)
    llm = cli_tools.CodexLLM(fake_bin, runner=runner)
    assert run(llm.text, "question", system="sys", model="", purpose="x") == "final answer"
    call = runner.calls[0]
    argv = call["argv"]
    assert argv[1] == "exec" and argv[-1] == "-" and "question" in call["stdin"]
    assert argv[argv.index("--sandbox") + 1] == "read-only" and "--skip-git-repo-check" in argv
    assert argv[argv.index("-C") + 1] == call["cwd"] and "-m" not in argv


@pytest.mark.parametrize(("cls", "stdout", "expected", "flags"), [
    (cli_tools.GeminiLLM, json.dumps({"response": "gem", "stats": {}}), "gem", ["--approval-mode", "default"]),
    (cli_tools.QwenLLM, json.dumps([{"type": "system"}, {"type": "result", "result": "qw"}]), "qw",
     ["--prompt", "--approval-mode"]),
    (cli_tools.OpenCodeLLM, '{"type":"step_start"}\n{"type":"text","part":{"type":"text","text":"op"}}\n'
                            '{"type":"text","part":{"type":"text","text":"en"}}\n', "open", ["run", "--format"]),
    (cli_tools.CopilotLLM, "plain copilot answer\n", "plain copilot answer", ["--deny-tool", "--no-ask-user"]),
    (cli_tools.GrokLLM, "\x1b[1mgrok says\x1b[0m\n", "grok says", ["--directory", "--prompt"]),
    (cli_tools.DroidLLM, json.dumps({"type": "result", "is_error": False, "result": "dr"}), "dr", ["exec", "--cwd"]),
])
def test_other_cli_adapters(fake_bin: str, cls: type, stdout: str, expected: str, flags: list[str],
                            monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv(cli_tools.UNSAFE_OPT_IN_ENV, "grok")  # grok is off unless opted in
    runner = FakeRunner(ProcessResult(0, stdout, ""))
    llm = cls(fake_bin, runner=runner)
    assert run(llm.text, "p", system="s", model="m1", purpose="x") == expected
    argv = runner.calls[0]["argv"]
    for flag in flags:
        assert flag in argv
    assert "m1" in argv
    for dangerous in ("--yolo", "--auto", "--allow-all-tools", "--force", "--skip-permissions-unsafe"):
        assert dangerous not in argv


ENGINE_SECRETS = {"TELLFOLD_SERVE_TOKEN": "engine-token-0123456789", "TELLFOLD_KEY_DEEPSEEK": SECRET,
                  "ANTHROPIC_API_KEY": SECRET, "OPENAI_API_KEY": SECRET}


@pytest.mark.parametrize("cls", [cli_tools.CursorAgentLLM, cli_tools.CodexLLM, cli_tools.GeminiLLM,
                                 cli_tools.QwenLLM, cli_tools.OpenCodeLLM, cli_tools.CopilotLLM, cli_tools.GrokLLM,
                                 cli_tools.DroidLLM])
def test_cli_children_never_see_engine_secrets(fake_bin: str, cls: type, monkeypatch: pytest.MonkeyPatch) -> None:
    for name, value in ENGINE_SECRETS.items():
        monkeypatch.setenv(name, value)
    monkeypatch.setenv("SOME_UNRELATED_VAR", "x")
    monkeypatch.setenv("GEMINI_API_KEY", "gem-key")
    monkeypatch.setenv("LC_ALL", "en_US.UTF-8")
    monkeypatch.setenv(cli_tools.UNSAFE_OPT_IN_ENV, "grok")
    runner = FakeRunner(ProcessResult(0, json.dumps({"result": "ok", "response": "ok"}), ""))
    run(cls(fake_bin, runner=runner).text, "p", system="s", model="", purpose="x")
    env = runner.calls[0]["env"]
    assert not set(ENGINE_SECRETS) & set(env)
    assert SECRET not in env.values() and "engine-token-0123456789" not in env.values()
    assert "SOME_UNRELATED_VAR" not in env and cli_tools.UNSAFE_OPT_IN_ENV not in env
    assert env["HOME"] == os.environ["HOME"] and env["LC_ALL"] == "en_US.UTF-8" and env["PATH"]
    # each tool keeps only its own variables
    assert ("GEMINI_API_KEY" in env) == (cls is cli_tools.GeminiLLM)


def test_opencode_child_denies_bash_edit_and_webfetch(fake_bin: str, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("OPENCODE_CONFIG_CONTENT", '{"permission": {"bash": "allow"}}')  # must not leak through
    runner = FakeRunner(ProcessResult(0, '{"type":"step_start"}\n{"type":"text","part":{"type":"text","text":"hi"}}\n',
                                      ""))
    run(cli_tools.OpenCodeLLM(fake_bin, runner=runner).text, "p", system="s", model="", purpose="x")
    config = json.loads(runner.calls[0]["env"]["OPENCODE_CONFIG_CONTENT"])
    assert config["permission"] == {"edit": "deny", "bash": "deny", "webfetch": "deny"}
    assert config["tools"]["bash"] is False and config["tools"]["edit"] is False


def test_gemini_trusts_only_the_empty_scratch_folder(fake_bin: str) -> None:
    runner = FakeRunner(ProcessResult(0, json.dumps({"response": "ok"}), ""))
    run(cli_tools.GeminiLLM(fake_bin, runner=runner).text, "p", system="s", model="", purpose="x")
    call = runner.calls[0]
    # folder trust is on by default and headless runs exit in an untrusted folder
    assert call["env"]["GEMINI_CLI_TRUST_WORKSPACE"] == "true" and call["cwd_was_empty"]
    assert "--yolo" not in call["argv"] and call["argv"][call["argv"].index("--approval-mode") + 1] == "default"


@pytest.mark.parametrize(("version", "standalone"), [("1.19.5", False), ("opencode 2.2.6", True), ("", False)])
def test_opencode_v2_runs_its_own_server(tmp_path: Path, version: str, standalone: bool) -> None:
    binary = make_script(tmp_path / "bin", "opencode", f'echo "{version}"')
    cli_tools.opencode_major_version.cache_clear()
    runner = FakeRunner(ProcessResult(0, '{"type":"step_start"}\n{"type":"text","part":{"type":"text","text":"hi"}}\n', ""))
    run(cli_tools.OpenCodeLLM(str(binary), runner=runner).text, "p", system="s", model="", purpose="x")
    argv = runner.calls[0]["argv"]
    assert ("--standalone" in argv) == standalone
    assert "--dir" not in argv  # v2 removed it; the scratch folder is the working directory instead


def test_ollama_context_fits_long_prompts() -> None:
    assert http_api.ollama_context_length("short") == 8192
    assert http_api.ollama_context_length("x" * 60_000) == 32768
    assert http_api.ollama_context_length("x" * 10_000_000) == 65536


def test_grok_is_disabled_unless_opted_in(fake_bin: str, isolated_home: Path,
                                          monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv(cli_tools.UNSAFE_OPT_IN_ENV, raising=False)
    runner = FakeRunner()
    with pytest.raises(LLMError, match=cli_tools.UNSAFE_OPT_IN_ENV):
        run(cli_tools.GrokLLM(fake_bin, runner=runner).text, "p", system="s", model="", purpose="x")
    assert runner.calls == []  # never spawned, never retried
    make_script(isolated_home / "bin", "grok", "echo 1.0.0")
    monkeypatch.setenv("GROK_API_KEY", "k")
    info = registry.get_runtime("grok", Settings()).detect()
    assert info.installed and info.auth == "missing" and cli_tools.UNSAFE_OPT_IN_ENV in (info.auth_hint or "")
    monkeypatch.setenv(cli_tools.UNSAFE_OPT_IN_ENV, "opencode, grok")
    assert registry.get_runtime("grok", Settings()).detect().auth == "ok"


def test_claude_code_options_blank_engine_secrets(monkeypatch: pytest.MonkeyPatch) -> None:
    for name, value in ENGINE_SECRETS.items():
        monkeypatch.setenv(name, value)
    options = registry.IsolatedClaudeLocal(cli_path="/bin/true")._options("sys", "haiku", None, "x")
    for name in ENGINE_SECRETS:
        assert options.env[name] == ""
    monkeypatch.delenv("ANTHROPIC_API_KEY")
    # blanked even when unset, so a key from the user's shell profile never switches Claude Code to API billing
    assert registry.IsolatedClaudeLocal(cli_path="/bin/true")._options("s", "haiku", None, "x").env[
        "ANTHROPIC_API_KEY"] == ""


def test_live_process_groups_are_tracked_and_killed(tmp_path: Path) -> None:
    script = tmp_path / "slow"
    script.write_text(f"#!{sys.executable}\nimport time\ntime.sleep(30)\n", encoding="utf-8")
    script.chmod(0o755)

    async def scenario() -> tuple[set[int], cli_tools.ProcessResult]:
        task = asyncio.ensure_future(cli_tools.run_process([str(script)], stdin=None, cwd=str(tmp_path),
                                                           env=dict(os.environ)))
        for _ in range(200):
            if cli_tools.live_process_groups():
                break
            await asyncio.sleep(0.01)
        live = cli_tools.live_process_groups()
        cli_tools.kill_live_process_groups()  # what the force-exit timer does before os._exit
        return live, await asyncio.wait_for(task, timeout=10)

    live, result = asyncio.run(scenario())
    assert len(live) == 1 and result.returncode == -9
    assert cli_tools.live_process_groups() == set()


def test_processes_a_tool_leaves_behind_are_ended(tmp_path: Path) -> None:
    pid_file = tmp_path / "worker.pid"
    script = make_script(tmp_path / "bin", "tool", f"sleep 30 >/dev/null 2>&1 &\necho $! > {pid_file}\necho done")
    result = asyncio.run(cli_tools.run_process([str(script)], stdin=None, cwd=str(tmp_path), env=dict(os.environ)))
    assert result.returncode == 0 and result.stdout.strip() == "done"
    worker = int(pid_file.read_text().strip())
    for _ in range(100):  # SIGTERM lands asynchronously
        try:
            os.kill(worker, 0)
        except ProcessLookupError:
            break
        time.sleep(0.02)
    else:
        os.kill(worker, signal.SIGKILL)
        raise AssertionError("the background worker outlived the call")


def test_parse_json_reply_errors() -> None:
    with pytest.raises(Fatal, match="log in"):
        parse_json_reply(json.dumps({"is_error": True, "result": "Please log in"}), auth_hint="log in!")
    with pytest.raises(Transient):
        parse_json_reply(json.dumps({"error": {"message": "quota exceeded"}}))
    with pytest.raises(BadOutput):
        parse_json_reply("   ")
    assert parse_json_reply("not json at all").text == "not json at all"


def test_real_process_runner_kills_on_timeout(tmp_path: Path) -> None:
    script = tmp_path / "slow"
    script.write_text(f"#!{sys.executable}\nimport time\ntime.sleep(30)\n", encoding="utf-8")
    script.chmod(0o755)

    class Slow(cli_tools.CLIAdapterLLM):
        runtime_id = "slow"

        def build(self, request: AdapterRequest, workdir: str) -> tuple[list[str], str | None]:
            return [self.binary, request.combined()], None

    llm = Slow(str(script), timeout_s=0.5, retries=0)
    with pytest.raises(LLMError, match="timed out"):
        run(llm.text, "p", system="s", model="", purpose="x")


def test_real_process_runner_passes_prompt_without_shell(tmp_path: Path) -> None:
    script = tmp_path / "echo"
    script.write_text(f"#!{sys.executable}\nimport sys, os\nprint(repr(sys.argv[1:]) + '|' + os.getcwd() + '|' "
                      f"+ sys.stdin.read())\n", encoding="utf-8")
    script.chmod(0o755)

    class Echo(cli_tools.CLIAdapterLLM):
        runtime_id = "echo"
        json_output = False

        def build(self, request: AdapterRequest, workdir: str) -> tuple[list[str], str | None]:
            return [self.binary, request.prompt], "from stdin"

    hostile = "$(touch pwned); `id`; rm -rf ~ && echo"
    out = run(Echo(str(script)).text, hostile, system="s", model="", purpose="x")
    args, cwd, stdin = out.split("|")
    assert hostile in args and "tellfold-rt-" in cwd and stdin == "from stdin"
    assert not (tmp_path / "pwned").exists()


# ---------- HTTP adapters (MockTransport) ----------


def test_ollama_chat_with_schema_and_auto_model() -> None:
    bodies: list[dict] = []

    def handler(request: httpx.Request) -> httpx.Response:
        if request.url.path == "/api/tags":
            return httpx.Response(200, json={"models": [{"name": "llama3.2:3b"}]})
        body = json.loads(request.content)
        bodies.append(body)
        return httpx.Response(200, json={"model": body["model"], "eval_count": 7,
                                         "message": {"role": "assistant", "content": '{"color": "o", "n": 4}'}})

    llm = http_api.OllamaLLM(base_url="http://127.0.0.1:11434", transport=httpx.MockTransport(handler))
    assert run(llm.json, "p", system="s", model="", purpose="engine.round", schema=Pick) == Pick(color="o", n=4)
    assert bodies[0]["model"] == "llama3.2:3b" and bodies[0]["stream"] is False
    assert bodies[0]["format"]["properties"]["n"]["type"] == "integer"
    assert [m["role"] for m in bodies[0]["messages"]] == ["system", "user"]


def test_ollama_missing_model_and_unreachable() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(404, json={"error": "model 'zzz' not found, try pulling it first"})

    llm = http_api.OllamaLLM(base_url="http://h:1", transport=httpx.MockTransport(handler))
    with pytest.raises(LLMError, match="ollama pull zzz"):
        run(llm.text, "p", system="s", model="zzz", purpose="x")

    def refuse(request: httpx.Request) -> httpx.Response:
        raise httpx.ConnectError("refused", request=request)

    llm = http_api.OllamaLLM(base_url="http://h:1", transport=httpx.MockTransport(refuse), retries=1)
    llm.backoff_base_s = 0
    with pytest.raises(LLMError, match="cannot reach"):
        run(llm.text, "p", system="s", model="m", purpose="x")


def test_openai_compatible_falls_back_json_schema_then_object_then_prompt(monkeypatch: pytest.MonkeyPatch,
                                                                         tmp_path: Path) -> None:
    monkeypatch.setenv("TELLFOLD_KEY_LOCAL", SECRET)
    seen: list[dict] = []

    def handler(request: httpx.Request) -> httpx.Response:
        assert request.headers["authorization"] == f"Bearer {SECRET}"
        body = json.loads(request.content)
        seen.append(body)
        if "response_format" in body:
            return httpx.Response(400, json={"error": {"message": "response_format not supported"}})
        return httpx.Response(200, json={"model": "m", "usage": {"completion_tokens": 3},
                                         "choices": [{"message": {"content": '{"color": "c", "n": 5}'}}]})

    log = tmp_path / "calls.jsonl"
    llm = http_api.OpenAICompatLLM(base_url="http://127.0.0.1:1234/v1", runtime_id="openai-compatible:local",
                                   api_key_env="TELLFOLD_KEY_LOCAL", transport=httpx.MockTransport(handler),
                                   log_path=log)
    assert run(llm.json, "p", system="s", model="qwen/qwen3-8b", purpose="x", schema=Pick).n == 5
    assert [b.get("response_format", {}).get("type") for b in seen] == ["json_schema", "json_object", None]
    assert seen[0]["response_format"]["json_schema"]["name"] == "Pick"
    assert run(llm.json, "p", system="s", model="qwen/qwen3-8b", purpose="x", schema=Pick).n == 5
    assert len(seen) == 4  # the fallback sticks
    assert SECRET not in log.read_text()


def test_openai_compatible_auth_failure_has_no_secret(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("TELLFOLD_KEY_LOCAL", SECRET)

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(401, json={"error": {"message": "invalid key"}})

    llm = http_api.OpenAICompatLLM(base_url="http://h/v1", api_key_env="TELLFOLD_KEY_LOCAL",
                                   transport=httpx.MockTransport(handler))
    with pytest.raises(LLMError) as err:
        run(llm.text, "p", system="s", model="m", purpose="x")
    assert "401" in str(err.value) and SECRET not in str(err.value)


def test_anthropic_api_headers_alias_mapping_and_refusal(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("ANTHROPIC_API_KEY", SECRET)
    seen: list[httpx.Request] = []
    replies = [
        {"content": [{"type": "text", "text": "hello"}], "stop_reason": "end_turn", "usage": {"output_tokens": 2}},
        {"content": [], "stop_reason": "refusal"},
    ]

    def handler(request: httpx.Request) -> httpx.Response:
        seen.append(request)
        return httpx.Response(200, json=replies.pop(0))

    llm = http_api.AnthropicAPILLM(transport=httpx.MockTransport(handler), retries=0)
    assert run(llm.text, "p", system="sys", model="haiku", purpose="x") == "hello"
    body = json.loads(seen[0].content)
    assert seen[0].url.path == "/v1/messages" and seen[0].headers["x-api-key"] == SECRET
    assert seen[0].headers["anthropic-version"] == http_api.ANTHROPIC_VERSION
    assert body["model"] == "claude-haiku-5-5" and body["system"] == "sys"
    assert "temperature" not in body and "thinking" not in body
    with pytest.raises(LLMError, match="declined"):
        run(llm.text, "p", system="sys", model="sonnet", purpose="x")


def test_keyed_apis_without_key_fail_fast(monkeypatch: pytest.MonkeyPatch) -> None:
    def never(request: httpx.Request) -> httpx.Response:  # pragma: no cover - must not be called
        raise AssertionError("no request without a key")

    for llm in (http_api.AnthropicAPILLM(transport=httpx.MockTransport(never)),
                http_api.OpenAIAPILLM(transport=httpx.MockTransport(never))):
        with pytest.raises(LLMError, match="not set"):
            run(llm.text, "p", system="s", model="m", purpose="x")


# ---------- routing and build_llm ----------


class Recorder:
    def __init__(self, name: str):
        self.name = name
        self.calls: list[tuple[str, str]] = []

    async def text(self, prompt: str, *, system: str, model: str, purpose: str) -> str:
        self.calls.append((purpose, model))
        return self.name

    async def json(self, prompt: str, *, system: str, model: str, purpose: str, schema: type) -> Any:
        self.calls.append((purpose, model))
        return schema(color=self.name, n=0)


def test_routed_llm_sends_agent_purposes_to_agent_runtime_and_maps_aliases() -> None:
    from tellfold.runtimes import Route

    agent, planner = Recorder("agent"), Recorder("planner")
    routed = RoutedLLM(Route(agent, "ollama", "llama3.2", False), Route(planner, "claude-code", "sonnet", True))
    for purpose in ("engine.round", "engine.sentiment", "engine.seed", "chat.interview"):
        assert is_agent_purpose(purpose)
        assert run(routed.text, "p", system="s", model="haiku", purpose=purpose) == "agent"
    for purpose in ("graph.extract", "personas.batch", "personas.cast", "report.write", "chat.ask", "doctor.ping"):
        assert not is_agent_purpose(purpose)
        assert run(routed.json, "p", system="s", model="sonnet", purpose=purpose, schema=Pick).color == "planner"
    assert {m for _, m in agent.calls} == {"llama3.2"}  # "haiku" means nothing to Ollama
    assert {m for _, m in planner.calls} == {"sonnet"}
    run(routed.text, "p", system="s", model="qwen3:8b", purpose="engine.round")
    assert agent.calls[-1] == ("engine.round", "qwen3:8b")  # an explicit model passes through


def test_build_llm_defaults_to_claude_code_and_shares_one_backend(tmp_path: Path) -> None:
    config = SimConfig(question="q", seed_files=[], think=True)
    llm = build_llm(config, Settings(), concurrency=3, log_path=tmp_path / "calls.jsonl")
    assert isinstance(llm, RoutedLLM)
    assert llm.agent.llm is llm.planner.llm and isinstance(llm.agent.llm, ClaudeLocal)
    assert llm.agent.llm._think_purposes[0] == "graph."
    options = llm.agent.llm._options("sys", "haiku", None, "engine.round")
    assert options.cwd and "tellfold-runtime-cwd" in str(options.cwd) and options.setting_sources == []


def test_build_llm_mixes_runtimes_and_uses_settings_models() -> None:
    settings = Settings(agent_runtime="openai-compatible:lm", agent_model="qwen/qwen3-8b", openai_endpoints=[
        OpenAIEndpoint(id="lm", name="LM", base_url="http://127.0.0.1:1234/v1", models=["first"])])
    config = SimConfig(question="q", seed_files=[], agent_runtime="openai-compatible:lm",
                       planner_runtime="anthropic-api")
    llm = build_llm(config, settings, concurrency=2, log_path=None)
    assert isinstance(llm.agent.llm, http_api.OpenAICompatLLM) and llm.agent.default_model == "qwen/qwen3-8b"
    assert isinstance(llm.planner.llm, http_api.AnthropicAPILLM) and llm.planner.accepts_claude_aliases
    with pytest.raises(UnknownRuntime):
        build_llm(config.model_copy(update={"planner_runtime": "mystery"}), settings, concurrency=1, log_path=None)


def test_test_runtime_reports_failure_without_raising(monkeypatch: pytest.MonkeyPatch) -> None:
    result = anyio.run(lambda: test_runtime("anthropic-api", Settings()))
    assert result["ok"] is False and "ANTHROPIC_API_KEY" in result["error"] and result["model"] == "claude-haiku-5-5"
    with pytest.raises(UnknownRuntime):
        anyio.run(lambda: test_runtime("nope", Settings()))


def test_make_llm_uses_the_run_config(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from tellfold import cli
    from tellfold.runner import write_json

    monkeypatch.delenv("TELLFOLD_FAKE_LLM", raising=False)
    run_dir = tmp_path / "run"
    run_dir.mkdir()
    write_json(run_dir / "config.json", SimConfig(question="q", seed_files=[], agent_runtime="ollama"))
    llm = cli.make_llm(run_dir, 2)
    assert isinstance(llm, RoutedLLM) and llm.agent.runtime_id == "ollama" and llm.planner.runtime_id == "claude-code"
    write_json(run_dir / "config.json", SimConfig(question="q", seed_files=[], agent_runtime="gone"))
    with pytest.raises(LLMError, match="tellfold runtimes"):
        cli.make_llm(run_dir, 2)
    assert cli.make_llm(None, 1).agent.runtime_id == "claude-code"
