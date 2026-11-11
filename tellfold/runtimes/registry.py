"""The runtime catalogue: what each runtime is, how to detect it, and how to build its LLM."""

from __future__ import annotations

import json as _json
import os
import tempfile
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Literal

import httpx

from tellfold.llm import DEFAULT_THINK_PURPOSES, LLM, PLANNER_THINK_PURPOSES, ClaudeLocal
from tellfold.runtimes import cli_tools, http_api
from tellfold.runtimes.base import TOOL_DEFAULT, Role, RuntimeInfo
from tellfold.runtimes.detect import AUTH_TIMEOUT_S, binary_version, find_binary, run_quick
from tellfold.settings import OpenAIEndpoint, Settings

HTTP_DETECT_TIMEOUT_S = 1.5
Auth = Literal["ok", "missing", "unknown"]


class UnknownRuntime(ValueError):
    """No runtime with that id (or an OpenAI-compatible endpoint that is not configured)."""


def _env_any(*names: str) -> bool:
    return any(os.environ.get(n, "").strip() for n in names)


def _home_file(*parts: str) -> bool:
    return Path.home().joinpath(*parts).is_file()


# ---------- Claude Code (wraps the existing ClaudeLocal) ----------


def _isolated_cwd() -> str:
    path = Path(tempfile.gettempdir()) / "tellfold-runtime-cwd"
    path.mkdir(parents=True, exist_ok=True)
    return str(path)


class IsolatedClaudeLocal(ClaudeLocal):
    """ClaudeLocal whose CLI process starts in an empty scratch folder instead of the caller's directory."""

    def _options(self, system: str, model: str, json_schema: dict[str, Any] | None, purpose: str):
        options = super()._options(system, model, json_schema, purpose)
        if getattr(options, "cwd", None) is None:
            options.cwd = _isolated_cwd()
        return options


# ---------- CLI runtimes ----------

AuthCheck = Callable[[str, bool], tuple[Auth, str | None]]


def _claude_auth(binary: str, check: bool) -> tuple[Auth, str | None]:
    hint = "Run `claude` once and use /login."
    if not check:
        return "unknown", hint
    code, out = run_quick([binary, "auth", "status"], timeout=AUTH_TIMEOUT_S)
    try:
        start = out.index("{")
        data, _ = _json.JSONDecoder().raw_decode(out, start)
        return ("ok" if data.get("loggedIn") else "missing"), hint
    except (ValueError, AttributeError):
        return "unknown", hint


def _cursor_auth(binary: str, check: bool) -> tuple[Auth, str | None]:
    hint = "Run `cursor-agent login` (or set CURSOR_API_KEY)."
    if _env_any("CURSOR_API_KEY"):
        return "ok", hint
    if not check:
        return "unknown", hint
    code, out = run_quick([binary, "status"], timeout=AUTH_TIMEOUT_S + 5)
    lowered = out.lower()
    if "not logged in" in lowered:
        return "missing", hint
    if "logged in" in lowered:
        return "ok", hint
    return "unknown", hint


def _codex_auth(binary: str, check: bool) -> tuple[Auth, str | None]:
    hint = "Run `codex login`."
    if not check:
        return "unknown", hint
    code, _ = run_quick([binary, "login", "status"], timeout=AUTH_TIMEOUT_S)
    return ("unknown" if code is None else "ok" if code == 0 else "missing"), hint


def _file_or_env_auth(hint: str, *, envs: tuple[str, ...] = (), files: tuple[tuple[str, ...], ...] = (),
                      absent: Auth = "unknown") -> AuthCheck:
    def check(binary: str, _: bool) -> tuple[Auth, str | None]:
        if _env_any(*envs) or any(_home_file(*f) for f in files):
            return "ok", hint
        return absent, hint

    return check


@dataclass(frozen=True)
class CLISpec:
    id: str
    name: str
    binaries: tuple[str, ...]
    adapter: type[cli_tools.CLIAdapterLLM] | None  # None: Claude Code (ClaudeLocal)
    auth: AuthCheck
    models: tuple[str, ...] = (TOOL_DEFAULT,)
    default_models: dict[str, str] = field(default_factory=lambda: {"agent": TOOL_DEFAULT, "planner": TOOL_DEFAULT})
    tested: bool = False
    notes: str = ""
    accepts_claude_aliases: bool = False


CLI_SPECS: tuple[CLISpec, ...] = (
    CLISpec(
        "claude-code", "Claude Code", ("claude",), None, _claude_auth,
        models=("haiku", "sonnet", "opus"), default_models={"agent": "haiku", "planner": "sonnet"},
        tested=True, accepts_claude_aliases=True,
        notes="claude-agent-sdk with tools off, no MCP servers, no project settings, in an empty folder.",
    ),
    CLISpec(
        "cursor-agent", "Cursor Agent", ("cursor-agent", "agent"), cli_tools.CursorAgentLLM, _cursor_auth,
        models=(TOOL_DEFAULT, "gpt-5", "sonnet-4", "sonnet-4-thinking"),
        notes="cursor-agent -p --output-format json --mode ask --sandbox enabled --trust; a real JSON call "
              "and a full run verified with cursor-agent 2026.11.05.",
    ),
    CLISpec(
        "codex", "Codex CLI", ("codex",), cli_tools.CodexLLM, _codex_auth,
        notes="codex exec --sandbox read-only --ephemeral, prompt on stdin; from the official CLI reference.",
    ),
    CLISpec(
        "gemini-cli", "Gemini CLI", ("gemini",), cli_tools.GeminiLLM,
        _file_or_env_auth("Run `gemini` once to sign in, or set GEMINI_API_KEY.",
                          envs=("GEMINI_API_KEY", "GOOGLE_API_KEY"), files=((".gemini", "oauth_creds.json"),)),
        models=("auto", "flash", "pro", "flash-lite"), default_models={"agent": "flash", "planner": "pro"},
        notes="gemini --prompt ... --output-format json --approval-mode default; from the official CLI reference.",
    ),
    CLISpec(
        "opencode", "OpenCode", ("opencode",), cli_tools.OpenCodeLLM,
        _file_or_env_auth("Run `opencode auth login`.", files=((".local", "share", "opencode", "auth.json"),)),
        notes="opencode run --format json [--standalone] in an empty scratch folder, with OPENCODE_CONFIG_CONTENT "
              "denying bash, edit and webfetch; models are provider/model. JSON event shape is parsed defensively.",
    ),
    CLISpec(
        "qwen-code", "Qwen Code", ("qwen",), cli_tools.QwenLLM,
        _file_or_env_auth("Run `qwen` once to sign in.", envs=("DASHSCOPE_API_KEY",),
                          files=((".qwen", "oauth_creds.json"),)),
        notes="qwen --prompt ... --output-format json; from the Qwen Code headless docs.",
    ),
    CLISpec(
        "copilot", "GitHub Copilot CLI", ("copilot",), cli_tools.CopilotLLM,
        _file_or_env_auth("Run `copilot` and use /login, or set GH_TOKEN.",
                          envs=("COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN")),
        notes="copilot -p ... -s --no-ask-user --deny-tool shell --deny-tool write; from GitHub's programmatic "
              "reference.",
    ),
    CLISpec(
        "grok", "Grok CLI", ("grok",), cli_tools.GrokLLM,
        _file_or_env_auth("Set GROK_API_KEY.", envs=("GROK_API_KEY",),
                          files=((".grok", "user-settings.json"),), absent="missing"),
        notes="Community grok-cli (superagent-ai), not an official xAI tool: grok --prompt ... --directory <tmp>. "
              "Off unless TELLFOLD_ALLOW_UNSAFE_RUNTIMES lists grok: it has no way to turn off its bash and edit "
              "tools in headless mode.",
    ),
    CLISpec(
        "droid", "Factory Droid", ("droid",), cli_tools.DroidLLM,
        _file_or_env_auth("Run `droid` once to sign in, or set FACTORY_API_KEY.", envs=("FACTORY_API_KEY",)),
        notes="droid exec -o json --cwd <tmp> (read-only without --auto); from the Factory CLI reference.",
    ),
)
CLI_BY_ID = {spec.id: spec for spec in CLI_SPECS}


class CLIRuntime:
    kind = "cli"

    def __init__(self, spec: CLISpec, settings: Settings):
        self.spec = spec
        self.settings = settings
        self.id = spec.id
        self.accepts_claude_aliases = spec.accepts_claude_aliases
        self._info: RuntimeInfo | None = None

    def binary(self) -> str | None:
        return find_binary(*self.spec.binaries)

    def detect(self, *, check_auth: bool = False) -> RuntimeInfo:
        path = self.binary()
        spec = self.spec
        info = RuntimeInfo(id=spec.id, kind="cli", name=spec.name, installed=path is not None, path=path,
                           models=list(spec.models), default_models=dict(spec.default_models),
                           tested=spec.tested, notes=spec.notes or None)
        if path is None:
            info.auth, info.auth_hint = "unknown", f"Install {spec.name} (`{spec.binaries[0]}` was not found)."
            return info
        info.version = binary_version(path)
        info.auth, info.auth_hint = spec.auth(path, check_auth)
        if spec.adapter is not None and spec.adapter.unsafe and not cli_tools.unsafe_runtime_allowed(spec.id):
            info.auth = "missing"
            info.auth_hint = (f"Disabled: {spec.name} cannot turn off its shell and file tools, so text in a seed "
                              f"document could make it run commands. Set {cli_tools.UNSAFE_OPT_IN_ENV}={spec.id} "
                              "to use it anyway.")
        self._info = info
        return info

    @property
    def info(self) -> RuntimeInfo:
        if self._info is None:
            self._info = self.detect()
        return self._info

    def static_default(self, role: Role) -> str:
        return self.spec.default_models.get(role, TOOL_DEFAULT)

    def llm(self, *, concurrency: int, log_path: Path | None, think: bool = False, **options: Any) -> LLM:
        path = self.binary()
        if self.spec.adapter is None:
            return IsolatedClaudeLocal(concurrency=concurrency, log_path=log_path, cli_path=path,
                                       think_purposes=PLANNER_THINK_PURPOSES if think else DEFAULT_THINK_PURPOSES,
                                       **options)
        return self.spec.adapter(path or self.spec.binaries[0], concurrency=concurrency, log_path=log_path,
                                 **options)


# ---------- HTTP runtimes ----------


def _get_json(url: str, headers: dict[str, str] | None = None, *, timeout: float = HTTP_DETECT_TIMEOUT_S,
              transport: httpx.BaseTransport | None = None) -> tuple[int | None, Any]:
    try:
        with httpx.Client(timeout=timeout, transport=transport, follow_redirects=False) as client:
            response = client.get(url, headers=headers or {})
    except httpx.HTTPError:
        return None, None
    try:
        return response.status_code, response.json()
    except ValueError:
        return response.status_code, None


class HTTPRuntime:
    kind = "http"
    accepts_claude_aliases = False
    sync_transport: httpx.BaseTransport | None = None  # tests inject httpx.MockTransport here
    async_transport: httpx.AsyncBaseTransport | None = None

    def __init__(self, settings: Settings):
        self.settings = settings
        self._info: RuntimeInfo | None = None

    @property
    def info(self) -> RuntimeInfo:
        if self._info is None:
            self._info = self.detect()
        return self._info

    def detect(self, *, check_auth: bool = False) -> RuntimeInfo:  # pragma: no cover - abstract
        raise NotImplementedError

    def _settings_model(self, role: Role) -> str | None:
        s = self.settings
        runtime, model = (s.agent_runtime, s.agent_model) if role == "agent" else (s.planner_runtime, s.planner_model)
        return model if runtime == self.id and model else None


class OllamaRuntime(HTTPRuntime):
    id = "ollama"
    name = "Ollama"

    @property
    def base_url(self) -> str:
        return self.settings.ollama_base_url

    def detect(self, *, check_auth: bool = False) -> RuntimeInfo:
        status, tags = _get_json(f"{self.base_url}/api/tags", transport=self.sync_transport)
        models = [m["name"] for m in (tags or {}).get("models", []) if isinstance(m, dict) and m.get("name")] \
            if isinstance(tags, dict) else []
        reachable = status == 200
        version = None
        if reachable:
            _, ver = _get_json(f"{self.base_url}/api/version", transport=self.sync_transport)
            version = ver.get("version") if isinstance(ver, dict) else None
        binary = find_binary("ollama")
        default = models[0] if models else ""
        info = RuntimeInfo(
            id=self.id, kind="http", name=self.name, installed=reachable, version=version, path=self.base_url,
            auth="ok" if reachable else "unknown",
            auth_hint=None if reachable else ("Start Ollama (open the app or run `ollama serve`)." if binary
                                               else "Install Ollama from ollama.com, then pull a model."),
            models=models,
            default_models={r: self._settings_model(r) or default for r in ("agent", "planner")},
            notes="POST /api/chat with format = the JSON schema." + ("" if models or not reachable
                                                                     else " No models pulled yet: `ollama pull llama3.2`."),
        )
        self._info = info
        return info

    def static_default(self, role: Role) -> str:
        return self._settings_model(role) or ""

    def llm(self, *, concurrency: int, log_path: Path | None, think: bool = False, **options: Any) -> LLM:
        return http_api.OllamaLLM(base_url=self.base_url, concurrency=concurrency, log_path=log_path,
                                  transport=self.async_transport, **options)


class OpenAICompatRuntime(HTTPRuntime):
    def __init__(self, settings: Settings, endpoint: OpenAIEndpoint):
        super().__init__(settings)
        self.endpoint = endpoint
        self.id = endpoint.runtime_id
        self.name = endpoint.name

    def _headers(self) -> dict[str, str]:
        key = os.environ.get(self.endpoint.api_key_env, "").strip()
        return {"Authorization": f"Bearer {key}"} if key else {}

    def detect(self, *, check_auth: bool = False) -> RuntimeInfo:
        ep = self.endpoint
        has_key = bool(self._headers())
        status, data = _get_json(f"{ep.base_url}/models", self._headers(), transport=self.sync_transport)
        listed = [m["id"] for m in data.get("data", []) if isinstance(m, dict) and isinstance(m.get("id"), str)] \
            if isinstance(data, dict) else []
        models = list(dict.fromkeys([*ep.models, *listed]))
        if status in (401, 403):
            auth: Auth = "missing"
        elif status == 200:
            auth = "ok"  # the server answered with our credentials (or needs none)
        else:
            auth = "ok" if has_key else "unknown"
        default = models[0] if models else ""
        info = RuntimeInfo(
            id=self.id, kind="http", name=ep.name, installed=status is not None and status < 500,
            path=ep.base_url, auth=auth,
            auth_hint=f"If the server needs a key, set {ep.api_key_env} (the Mac app does this from the Keychain).",
            models=models, default_models={r: self._settings_model(r) or default for r in ("agent", "planner")},
            notes="POST /chat/completions with response_format json_schema, falling back to json_object, then to "
                  "prompt-only JSON.",
        )
        self._info = info
        return info

    def static_default(self, role: Role) -> str:
        return self._settings_model(role) or (self.endpoint.models[0] if self.endpoint.models else "")

    def llm(self, *, concurrency: int, log_path: Path | None, think: bool = False, **options: Any) -> LLM:
        return http_api.OpenAICompatLLM(base_url=self.endpoint.base_url, runtime_id=self.id,
                                        api_key_env=self.endpoint.api_key_env, concurrency=concurrency,
                                        log_path=log_path, transport=self.async_transport, **options)


class _KeyedAPIRuntime(HTTPRuntime):
    key_env = ""
    base_url = ""
    models_path = ""
    static_models: list[str] = []
    defaults: dict[str, str] = {}

    def _auth_headers(self, key: str) -> dict[str, str]:
        return {"Authorization": f"Bearer {key}"}

    def detect(self, *, check_auth: bool = False) -> RuntimeInfo:
        key = os.environ.get(self.key_env, "").strip()
        auth: Auth = "unknown" if key else "missing"
        models = list(self.static_models)
        if key and check_auth:
            status, data = _get_json(f"{self.base_url}{self.models_path}", self._auth_headers(key), timeout=5,
                                     transport=self.sync_transport)
            if status in (401, 403):
                auth = "missing"
            elif status == 200:
                auth = "ok"
        info = RuntimeInfo(
            id=self.id, kind="http", name=self.name, installed=bool(key), path=self.base_url, auth=auth,
            auth_hint=f"Set {self.key_env} (the Mac app stores it in the Keychain).", models=models,
            default_models={r: self._settings_model(r) or self.defaults[r] for r in ("agent", "planner")},
            notes=self.notes,
        )
        self._info = info
        return info

    def static_default(self, role: Role) -> str:
        return self._settings_model(role) or self.defaults[role]


class AnthropicAPIRuntime(_KeyedAPIRuntime):
    id = "anthropic-api"
    name = "Anthropic API"
    key_env = "ANTHROPIC_API_KEY"
    base_url = http_api.ANTHROPIC_BASE_URL
    models_path = "/v1/models"
    static_models = list(http_api.ANTHROPIC_MODELS)
    defaults = {"agent": "claude-haiku-5-5", "planner": "claude-sonnet-5-5"}
    notes = "Messages API, billed per token. haiku/sonnet/opus map to the current model ids."
    accepts_claude_aliases = True

    def _auth_headers(self, key: str) -> dict[str, str]:
        return {"x-api-key": key, "anthropic-version": http_api.ANTHROPIC_VERSION}

    def llm(self, *, concurrency: int, log_path: Path | None, think: bool = False, **options: Any) -> LLM:
        return http_api.AnthropicAPILLM(concurrency=concurrency, log_path=log_path, transport=self.async_transport,
                                        **options)


class OpenAIAPIRuntime(_KeyedAPIRuntime):
    id = "openai-api"
    name = "OpenAI API"
    key_env = "OPENAI_API_KEY"
    base_url = http_api.OPENAI_BASE_URL
    models_path = "/models"
    static_models = list(http_api.OPENAI_MODELS)
    defaults = {"agent": "gpt-5-mini", "planner": "gpt-5"}
    notes = "Chat Completions, billed per token. Default model names are suggestions; pick any your key can use."

    def llm(self, *, concurrency: int, log_path: Path | None, think: bool = False, **options: Any) -> LLM:
        return http_api.OpenAIAPILLM(concurrency=concurrency, log_path=log_path, transport=self.async_transport,
                                     **options)


AnyRuntime = CLIRuntime | OllamaRuntime | OpenAICompatRuntime | AnthropicAPIRuntime | OpenAIAPIRuntime


def all_runtimes(settings: Settings) -> list[AnyRuntime]:
    runtimes: list[AnyRuntime] = [CLIRuntime(spec, settings) for spec in CLI_SPECS]
    runtimes.append(OllamaRuntime(settings))
    runtimes.extend(OpenAICompatRuntime(settings, ep) for ep in settings.openai_endpoints)
    runtimes.append(AnthropicAPIRuntime(settings))
    runtimes.append(OpenAIAPIRuntime(settings))
    return runtimes


def known_ids(settings: Settings) -> list[str]:
    return [r.id for r in all_runtimes(settings)]


def get_runtime(runtime_id: str, settings: Settings) -> AnyRuntime:
    if runtime_id in CLI_BY_ID:
        return CLIRuntime(CLI_BY_ID[runtime_id], settings)
    if runtime_id == OllamaRuntime.id:
        return OllamaRuntime(settings)
    if runtime_id == AnthropicAPIRuntime.id:
        return AnthropicAPIRuntime(settings)
    if runtime_id == OpenAIAPIRuntime.id:
        return OpenAIAPIRuntime(settings)
    prefix, _, endpoint_id = runtime_id.partition(":")
    if prefix == "openai-compatible" and endpoint_id:
        endpoint = settings.endpoint(endpoint_id)
        if endpoint is not None:
            return OpenAICompatRuntime(settings, endpoint)
        raise UnknownRuntime(f"no OpenAI-compatible endpoint {endpoint_id!r} is configured in settings")
    raise UnknownRuntime(f"unknown runtime {runtime_id!r}")
