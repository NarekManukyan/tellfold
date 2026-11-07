"""User settings shared by the CLI, `shoal serve` and the Mac app: `$SHOAL_HOME/settings.json`.

No secrets live here. API keys come only from environment variables (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`,
`SHOAL_KEY_<PROVIDER_ID>` or an endpoint's own `key_env`); the Mac app keeps them in the Keychain and passes
them to the engine process as environment variables.
"""

from __future__ import annotations

import os
import re
import tempfile
from pathlib import Path

from pydantic import BaseModel, Field, ValidationError, field_validator, model_validator

HOME_ENV = "SHOAL_HOME"
SETTINGS_FILE = "settings.json"
DEFAULT_OLLAMA_URL = "http://127.0.0.1:11434"

_SLUG = re.compile(r"^[a-z0-9][a-z0-9_-]{0,39}$")
_ENV_NAME = re.compile(r"^[A-Za-z_][A-Za-z0-9_]{0,99}$")
_RUNTIME_ID = re.compile(r"^[a-z0-9][a-z0-9._-]{0,39}(:[a-z0-9][a-z0-9_-]{0,39})?$")
_MODEL = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:/@\[\]-]{0,119}$")


def shoal_home() -> Path:
    """`$SHOAL_HOME`, else `~/Library/Application Support/Shoal`."""
    raw = os.environ.get(HOME_ENV, "").strip()
    if raw:
        return Path(raw).expanduser()
    return Path.home() / "Library" / "Application Support" / "Shoal"


def default_settings_path() -> Path:
    return shoal_home() / SETTINGS_FILE


def key_env_for(provider_id: str) -> str:
    """The default environment variable that holds a provider's API key: `SHOAL_KEY_<PROVIDER_ID>`."""
    return "SHOAL_KEY_" + re.sub(r"[^A-Z0-9]+", "_", provider_id.upper()).strip("_")


def _check_url(value: str) -> str:
    value = value.strip().rstrip("/")
    if not re.match(r"^https?://[^\s/]+", value):
        raise ValueError("must be an http:// or https:// URL")
    if "@" in value.split("//", 1)[1].split("/", 1)[0]:
        raise ValueError("must not contain credentials")
    return value


class OpenAIEndpoint(BaseModel):
    """One OpenAI-compatible server: LM Studio, llama.cpp, vLLM, OpenRouter, DeepSeek, ..."""

    id: str = Field(description="slug, e.g. 'lmstudio'; the runtime id becomes 'openai-compatible:<id>'")
    name: str = Field(min_length=1, max_length=80)
    base_url: str = Field(description="e.g. http://127.0.0.1:1234/v1 (the part before /chat/completions)")
    models: list[str] = Field(default_factory=list)
    key_env: str | None = Field(default=None, description="env var holding the API key; "
                                "default SHOAL_KEY_<ID>. A name, never the key itself.")

    @field_validator("id")
    @classmethod
    def _id(cls, v: str) -> str:
        v = v.strip().lower()
        if not _SLUG.match(v):
            raise ValueError("id must be a short slug: a-z, 0-9, '-' or '_'")
        return v

    @field_validator("base_url")
    @classmethod
    def _url(cls, v: str) -> str:
        return _check_url(v)

    @field_validator("models")
    @classmethod
    def _models(cls, v: list[str]) -> list[str]:
        out = [m.strip() for m in v if m.strip()]
        for m in out:
            if not _MODEL.match(m):
                raise ValueError(f"invalid model name {m!r}")
        return list(dict.fromkeys(out))[:200]

    @field_validator("key_env")
    @classmethod
    def _key_env(cls, v: str | None) -> str | None:
        if v is None or not v.strip():
            return None
        v = v.strip()
        if not _ENV_NAME.match(v):
            raise ValueError("key_env must be an environment variable name, not a key")
        return v

    @property
    def runtime_id(self) -> str:
        return f"openai-compatible:{self.id}"

    @property
    def api_key_env(self) -> str:
        return self.key_env or key_env_for(self.id)


class Settings(BaseModel):
    runs_dir: str | None = None  # None: the caller's default (the CLI's ./runs, the Mac app's own folder)
    agent_runtime: str = "claude-code"
    agent_model: str = "haiku"
    planner_runtime: str = "claude-code"
    planner_model: str = "sonnet"
    ollama_base_url: str = DEFAULT_OLLAMA_URL
    openai_endpoints: list[OpenAIEndpoint] = Field(default_factory=list)
    concurrency: int = Field(default=4, ge=1, le=16)
    think: bool = False

    @field_validator("agent_runtime", "planner_runtime")
    @classmethod
    def _runtime(cls, v: str) -> str:
        v = v.strip()
        if not _RUNTIME_ID.match(v):
            raise ValueError("invalid runtime id")
        return v

    @field_validator("agent_model", "planner_model")
    @classmethod
    def _model(cls, v: str) -> str:
        v = v.strip()
        if v and not _MODEL.match(v):
            raise ValueError("invalid model name")
        return v

    @field_validator("ollama_base_url")
    @classmethod
    def _ollama(cls, v: str) -> str:
        return _check_url(v)

    @model_validator(mode="after")
    def _unique_endpoints(self) -> "Settings":
        ids = [e.id for e in self.openai_endpoints]
        if len(ids) != len(set(ids)):
            raise ValueError("OpenAI-compatible endpoint ids must be unique")
        return self

    def endpoint(self, endpoint_id: str) -> OpenAIEndpoint | None:
        return next((e for e in self.openai_endpoints if e.id == endpoint_id), None)


def load_settings(path: Path | None = None) -> Settings:
    """Settings from disk; defaults when the file is missing or unreadable (never raises)."""
    path = path or default_settings_path()
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return Settings()
    try:
        return Settings.model_validate_json(text)
    except ValidationError:
        return Settings()


def save_settings(settings: Settings, path: Path | None = None) -> Path:
    """Write atomically (temp file + rename), creating the folder if needed."""
    path = path or default_settings_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=".settings-", suffix=".json", dir=path.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write(settings.model_dump_json(indent=2) + "\n")
        os.replace(tmp, path)
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise
    return path
