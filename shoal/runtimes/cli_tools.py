"""Adapters for agent CLIs driven headless as subprocesses.

Every call runs in a fresh, empty temporary directory (never the user's repos), through
`asyncio.create_subprocess_exec` (no shell), with the prompt passed as one argv element or on stdin. No adapter
grants tool permissions; where a CLI has a read-only / ask / plan mode, a sandbox switch or a permission config,
it is used to turn tools off. A CLI with no way to turn its tools off is marked `unsafe` and refuses to run unless
the user opts in with SHOAL_ALLOW_UNSAFE_RUNTIMES. Children get an allowlisted environment (see
`detect.child_env`): never the engine's token or the Keychain keys. The process group is killed on timeout or
cancellation, and `kill_live_process_groups` kills every group still running when the engine force-exits.

Flags were taken from each tool's own `--help` (installed tools) or its official docs; see the `notes` on each
runtime. Only adapters verified against the real tool on a real Mac carry `tested=True`.
"""

from __future__ import annotations

import asyncio
import contextlib
import json as _json
import os
import signal
import tempfile
import threading
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import anyio

from shoal.runtimes.base import (
    AdapterLLM,
    AdapterReply,
    AdapterRequest,
    BadOutput,
    Fatal,
    Transient,
    classify_failure,
)
from shoal.runtimes.detect import child_env, strip_ansi

MAX_OUTPUT_BYTES = 8 * 1024 * 1024
UNSAFE_OPT_IN_ENV = "SHOAL_ALLOW_UNSAFE_RUNTIMES"

# Process groups of agent CLIs that are running right now (each child leads its own group).
_live_groups: set[int] = set()
_live_lock = threading.Lock()


def live_process_groups() -> set[int]:
    with _live_lock:
        return set(_live_groups)


def kill_live_process_groups() -> None:
    """SIGKILL every agent CLI process group still running. Safe to call from any thread, e.g. right before
    os._exit, when the event loop that would cancel them is stuck."""
    for pgid in live_process_groups():
        with contextlib.suppress(ProcessLookupError, PermissionError, OSError):
            os.killpg(pgid, signal.SIGKILL)


def unsafe_runtime_allowed(runtime_id: str) -> bool:
    raw = os.environ.get(UNSAFE_OPT_IN_ENV, "")
    allowed = {part.strip().lower() for part in raw.replace(";", ",").split(",") if part.strip()}
    return runtime_id in allowed


@dataclass
class ProcessResult:
    returncode: int
    stdout: str
    stderr: str


ProcessRunner = Callable[..., Awaitable[ProcessResult]]


async def run_process(argv: list[str], *, stdin: str | None, cwd: str, env: dict[str, str]) -> ProcessResult:
    """Spawn without a shell in its own process group; kill the whole group if we are cancelled."""
    proc = await asyncio.create_subprocess_exec(
        *argv,
        stdin=asyncio.subprocess.PIPE if stdin is not None else asyncio.subprocess.DEVNULL,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        cwd=cwd,
        env=env,
        start_new_session=True,
        limit=MAX_OUTPUT_BYTES,
    )
    with _live_lock:
        _live_groups.add(proc.pid)
    try:
        out, err = await proc.communicate(stdin.encode("utf-8") if stdin is not None else None)
    except BaseException:
        _kill_group(proc)
        with anyio.CancelScope(shield=True), contextlib.suppress(Exception):
            await asyncio.wait_for(proc.wait(), timeout=5)
        raise
    finally:
        with _live_lock:
            _live_groups.discard(proc.pid)
    return ProcessResult(proc.returncode if proc.returncode is not None else -1,
                         out.decode("utf-8", "replace"), err.decode("utf-8", "replace"))


def _kill_group(proc: asyncio.subprocess.Process) -> None:
    if proc.returncode is not None:
        return
    with contextlib.suppress(ProcessLookupError, PermissionError, OSError):
        os.killpg(proc.pid, signal.SIGKILL)
    with contextlib.suppress(ProcessLookupError, OSError):
        proc.kill()


# ---------- output parsing ----------

_TEXT_KEYS = ("result", "response", "text", "content", "output", "message", "answer")


def _text_of(value: Any) -> str | None:
    if isinstance(value, str):
        return value
    if isinstance(value, dict):
        for key in ("text", "content", "result", "response"):
            inner = value.get(key)
            if isinstance(inner, str):
                return inner
    if isinstance(value, list):
        parts = [t for t in (_text_of(v) for v in value) if t]
        return "\n".join(parts) if parts else None
    return None


def _error_of(obj: dict[str, Any]) -> str | None:
    if obj.get("is_error") is True or obj.get("subtype") == "error" or obj.get("type") == "error":
        return _text_of(obj.get("error")) or _text_of(obj.get("result")) or _text_of(obj.get("message")) \
            or "the tool reported an error"
    error = obj.get("error")
    if error and not any(isinstance(obj.get(k), str) and obj.get(k) for k in ("result", "response")):
        return _text_of(error) or str(error)
    return None


def parse_json_reply(stdout: str, *, auth_hint: str | None = None) -> AdapterReply:
    """One JSON object (`{result}` / `{response}` style), a JSON list of messages, or JSONL events; else text."""
    text = strip_ansi(stdout).strip()
    if not text:
        raise BadOutput("the tool printed nothing")
    try:
        value = _json.loads(text)
    except ValueError:
        events = []
        for line in text.splitlines():
            line = line.strip()
            if line.startswith(("{", "[")):
                with contextlib.suppress(ValueError):
                    events.append(_json.loads(line))
        if not events:
            return AdapterReply(text=text)
        value = events if len(events) > 1 else events[0]
    if isinstance(value, dict):
        error = _error_of(value)
        if error:
            raise classify_failure(error, auth_hint=auth_hint)
        for key in _TEXT_KEYS:
            found = _text_of(value.get(key))
            if found and found.strip():
                return AdapterReply(text=found, model=value.get("model") if isinstance(value.get("model"), str) else None)
        raise BadOutput("the tool's JSON output had no response text", reply_text=text[:2000])
    if isinstance(value, list):
        return AdapterReply(text=collect_event_text(value, auth_hint=auth_hint))
    return AdapterReply(text=str(value))


def collect_event_text(events: list[Any], *, auth_hint: str | None = None) -> str:
    """Final text from a list of stream events: the last `result` event, else the concatenated text parts."""
    for ev in reversed(events):
        if isinstance(ev, dict) and ev.get("type") == "result":
            error = _error_of(ev)
            if error:
                raise classify_failure(error, auth_hint=auth_hint)
            found = _text_of(ev.get("result")) or _text_of(ev.get("response"))
            if found:
                return found
    parts: list[str] = []
    for ev in events:
        if not isinstance(ev, dict):
            continue
        if ev.get("type") == "error":
            raise classify_failure(_text_of(ev.get("error")) or _text_of(ev.get("message")) or "error event",
                                   auth_hint=auth_hint)
        part = ev.get("part") if isinstance(ev.get("part"), dict) else ev
        if part.get("type") == "text" and isinstance(part.get("text"), str):
            parts.append(part["text"])
        elif ev.get("type") in ("message", "assistant") and ev.get("role", "assistant") == "assistant":
            found = _text_of(ev.get("content")) or _text_of(ev.get("message"))
            if found:
                parts.append(found)
    joined = "".join(parts).strip()
    if not joined:
        raise BadOutput("no assistant text in the tool's event stream")
    return joined


# ---------- the base CLI adapter ----------


class CLIAdapterLLM(AdapterLLM):
    """Runs one CLI invocation per request. Subclasses define `build` and may override `parse`."""

    runtime_id = "cli"
    auth_hint: str | None = None
    json_output = True  # parse stdout with parse_json_reply
    # The tool's own variables (API key, config dir) that it may inherit; engine secrets are dropped regardless.
    passthrough_env: tuple[str, ...] = ()
    # True when the CLI has no way to turn its shell / file tools off in headless mode. Seed text is untrusted, so
    # such a runtime only runs when the user lists its id in SHOAL_ALLOW_UNSAFE_RUNTIMES.
    unsafe = False

    def __init__(self, binary: str, *, concurrency: int = 4, log_path: Path | None = None,
                 timeout_s: float = 240, retries: int = 2, runner: ProcessRunner | None = None):
        super().__init__(concurrency=concurrency, log_path=log_path, timeout_s=timeout_s, retries=retries)
        self.binary = binary
        self._runner = runner or run_process

    def build(self, request: AdapterRequest, workdir: str) -> tuple[list[str], str | None]:
        """(argv, stdin text or None)."""
        raise NotImplementedError

    def parse(self, result: ProcessResult, workdir: str) -> AdapterReply:
        if self.json_output:
            return parse_json_reply(result.stdout, auth_hint=self.auth_hint)
        text = strip_ansi(result.stdout).strip()
        if not text:
            raise BadOutput("the tool printed nothing")
        return AdapterReply(text=text)

    def extra_env(self) -> dict[str, str]:
        """Variables the adapter sets for the child (they win over inherited ones)."""
        return {}

    def env(self) -> dict[str, str]:
        return child_env(self.binary, extra=self.extra_env(), passthrough=self.passthrough_env)

    async def _send(self, request: AdapterRequest) -> AdapterReply:
        if self.unsafe and not unsafe_runtime_allowed(self.runtime_id):
            raise Fatal(f"{self.runtime_id} is disabled: it cannot turn off its shell and file tools, so text in a "
                        f"seed document could make it run commands. Set {UNSAFE_OPT_IN_ENV}={self.runtime_id} to "
                        "use it anyway.")
        if not self.binary or not Path(self.binary).exists():
            raise Fatal(f"{self.runtime_id} is not installed (binary {self.binary or 'not found'})")
        with tempfile.TemporaryDirectory(prefix="shoal-rt-") as workdir:
            argv, stdin = self.build(request, workdir)
            try:
                result = await self._runner(argv, stdin=stdin, cwd=workdir, env=self.env())
            except FileNotFoundError as err:
                raise Fatal(f"{self.runtime_id}: cannot start {self.binary}: {err}") from err
            except OSError as err:
                raise Transient(f"cannot start {self.binary}: {err}") from err
            if result.returncode != 0:
                message = strip_ansi(result.stderr).strip() or strip_ansi(result.stdout).strip()
                try:  # a JSON error object on stdout says more than the exit code
                    parse_json_reply(result.stdout, auth_hint=self.auth_hint)
                except (Fatal, Transient):
                    raise
                except BadOutput:
                    pass
                raise classify_failure(f"exit {result.returncode}: {message[-1500:]}", auth_hint=self.auth_hint)
            return self.parse(result, workdir)


# ---------- adapters ----------


class CursorAgentLLM(CLIAdapterLLM):
    """`cursor-agent -p --output-format json --mode ask --sandbox enabled --workspace <tmp> [--model M] PROMPT`.

    Flags verified with `cursor-agent --help` (2026.01.23). `--mode ask` is read-only Q&A; `--force` and
    `--approve-mcps` are never passed, so no command runs and no MCP server is approved. JSON output is one
    `{type: "result", is_error, result, ...}` object; errors exit non-zero with text on stderr.
    """

    runtime_id = "cursor-agent"
    auth_hint = "Cursor Agent is not logged in: run `cursor-agent login` (or set CURSOR_API_KEY)."
    passthrough_env = ("CURSOR_API_KEY", "CURSOR_CONFIG_DIR")

    def build(self, request: AdapterRequest, workdir: str) -> tuple[list[str], str | None]:
        argv = [self.binary, "-p", "--output-format", "json", "--mode", "ask", "--sandbox", "enabled",
                "--workspace", workdir]
        if request.model_or_none:
            argv += ["--model", request.model_or_none]
        return [*argv, request.combined()], None


class CodexLLM(CLIAdapterLLM):
    """`codex exec --skip-git-repo-check --sandbox read-only --ephemeral -C <tmp> -o <tmp>/last.txt [-m M] -`
    with the prompt on stdin (official CLI reference). The final message is read from the `-o` file."""

    runtime_id = "codex"
    auth_hint = "Codex is not logged in: run `codex login`."
    json_output = False
    passthrough_env = ("CODEX_HOME",)

    def build(self, request: AdapterRequest, workdir: str) -> tuple[list[str], str | None]:
        argv = [self.binary, "exec", "--skip-git-repo-check", "--sandbox", "read-only", "--ephemeral",
                "-C", workdir, "--output-last-message", str(Path(workdir) / "last-message.txt")]
        if request.model_or_none:
            argv += ["-m", request.model_or_none]
        return [*argv, "-"], request.combined()

    def parse(self, result: ProcessResult, workdir: str) -> AdapterReply:
        path = Path(workdir) / "last-message.txt"
        with contextlib.suppress(OSError):
            text = path.read_text(encoding="utf-8").strip()
            if text:
                return AdapterReply(text=text)
        return super().parse(result, workdir)


class GeminiLLM(CLIAdapterLLM):
    """`gemini -p PROMPT --output-format json --approval-mode default [-m M]` (official CLI reference).
    JSON output is `{response, stats, error?}`. `default` approval never auto-approves a tool."""

    runtime_id = "gemini-cli"
    auth_hint = "Gemini CLI is not signed in: run `gemini` once to log in, or set GEMINI_API_KEY."
    passthrough_env = ("GEMINI_API_KEY", "GOOGLE_API_KEY", "GOOGLE_CLOUD_PROJECT", "GOOGLE_CLOUD_LOCATION",
                       "GOOGLE_GENAI_USE_VERTEXAI", "GOOGLE_APPLICATION_CREDENTIALS", "GEMINI_CLI_HOME")

    def build(self, request: AdapterRequest, workdir: str) -> tuple[list[str], str | None]:
        argv = [self.binary, "--output-format", "json", "--approval-mode", "default"]
        if request.model_or_none:
            argv += ["--model", request.model_or_none]
        return [*argv, "--prompt", request.combined()], None


class QwenLLM(GeminiLLM):
    """`qwen -p PROMPT --output-format json [-m M]` (Qwen Code headless docs; a Gemini CLI fork). The JSON shape
    varies across versions (`{response}` or a list of messages ending in a `result`); both are handled."""

    runtime_id = "qwen-code"
    auth_hint = "Qwen Code is not signed in: run `qwen` once to log in, or set DASHSCOPE_API_KEY."
    passthrough_env = ("DASHSCOPE_API_KEY",)

    def build(self, request: AdapterRequest, workdir: str) -> tuple[list[str], str | None]:
        argv = [self.binary, "--output-format", "json"]
        if request.model_or_none:
            argv += ["--model", request.model_or_none]
        return [*argv, "--prompt", request.combined()], None


# OpenCode allows its bash, edit and webfetch tools without asking by default, and `run` never prompts. This inline
# config (OPENCODE_CONFIG_CONTENT is merged over the user's own config) denies all three and switches the tools off.
OPENCODE_LOCKDOWN_CONFIG = _json.dumps({
    "permission": {"edit": "deny", "bash": "deny", "webfetch": "deny"},
    "tools": {"bash": False, "edit": False, "write": False, "patch": False, "webfetch": False},
}, separators=(",", ":"))


class OpenCodeLLM(CLIAdapterLLM):
    """`opencode run --format json --dir <tmp> [-m provider/model] PROMPT` (opencode.ai/docs/cli). JSON format is
    a stream of events; text parts are concatenated. Never passes `--auto`, and OPENCODE_CONFIG_CONTENT denies the
    bash, edit and webfetch permissions (OpenCode allows them by default and `run` never asks)."""

    runtime_id = "opencode"
    auth_hint = "OpenCode has no provider configured: run `opencode auth login`."

    def extra_env(self) -> dict[str, str]:
        return {"OPENCODE_CONFIG_CONTENT": OPENCODE_LOCKDOWN_CONFIG}

    def build(self, request: AdapterRequest, workdir: str) -> tuple[list[str], str | None]:
        argv = [self.binary, "run", "--format", "json", "--dir", workdir]
        if request.model_or_none:
            argv += ["--model", request.model_or_none]
        return [*argv, request.combined()], None


class CopilotLLM(CLIAdapterLLM):
    """`copilot -p PROMPT -s --no-ask-user --deny-tool shell --deny-tool write [--model M]` (GitHub's CLI
    programmatic reference). `-s` prints only the agent's response as plain text."""

    runtime_id = "copilot"
    auth_hint = "GitHub Copilot CLI is not signed in: run `copilot` and use /login, or set GH_TOKEN."
    json_output = False
    passthrough_env = ("COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN", "COPILOT_HOME")

    def build(self, request: AdapterRequest, workdir: str) -> tuple[list[str], str | None]:
        argv = [self.binary, "-s", "--no-ask-user", "--deny-tool", "shell", "--deny-tool", "write"]
        if request.model_or_none:
            argv += ["--model", request.model_or_none]
        return [*argv, "-p", request.combined()], None


class GrokLLM(CLIAdapterLLM):
    """`grok --prompt PROMPT --directory <tmp> [--model M]` (community grok-cli README; not an official xAI
    tool). Plain-text output.

    Unsafe: grok-cli's headless agent loop has bash and file-edit tools, runs them without asking, and has no
    documented switch to turn them off. It only runs when SHOAL_ALLOW_UNSAFE_RUNTIMES lists `grok`."""

    runtime_id = "grok"
    auth_hint = "Grok CLI needs an API key: set GROK_API_KEY."
    json_output = False
    passthrough_env = ("GROK_API_KEY", "GROK_BASE_URL")
    unsafe = True

    def build(self, request: AdapterRequest, workdir: str) -> tuple[list[str], str | None]:
        argv = [self.binary, "--directory", workdir]
        if request.model_or_none:
            argv += ["--model", request.model_or_none]
        return [*argv, "--prompt", request.combined()], None


class DroidLLM(CLIAdapterLLM):
    """`droid exec -o json --cwd <tmp> [-m M] PROMPT` (Factory CLI reference). Without `--auto` droid exec is
    read-only. The JSON result's text field is undocumented; the generic parser looks for `result`/`response`."""

    runtime_id = "droid"
    auth_hint = "Droid is not signed in: run `droid` once to log in, or set FACTORY_API_KEY."
    passthrough_env = ("FACTORY_API_KEY",)

    def build(self, request: AdapterRequest, workdir: str) -> tuple[list[str], str | None]:
        argv = [self.binary, "exec", "--output-format", "json", "--cwd", workdir]
        if request.model_or_none:
            argv += ["--model", request.model_or_none]
        return [*argv, request.combined()], None
