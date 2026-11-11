"""Adapters for HTTP model servers: Ollama, any OpenAI-compatible server, the Anthropic and OpenAI APIs.

All use `httpx.AsyncClient` (a fresh client per request, so an adapter can be reused across event loops, as the
CLI's chat REPL does). API keys are read from environment variables at call time and only ever placed in request
headers; error messages carry the status and a short body excerpt, never headers.
"""

from __future__ import annotations

import os
import re
from pathlib import Path
from typing import Any

import httpx

from tellfold.runtimes.base import (
    AdapterLLM,
    AdapterReply,
    AdapterRequest,
    BadOutput,
    Fatal,
    Transient,
)
from tellfold.settings import renamed_key_env

ANTHROPIC_BASE_URL = "https://api.anthropic.com"
ANTHROPIC_VERSION = "2026-10-21"
ANTHROPIC_ALIASES = {"haiku": "claude-haiku-5-5", "sonnet": "claude-sonnet-5-5", "opus": "claude-opus-5-5"}
ANTHROPIC_MODELS = ["claude-haiku-5-5", "claude-sonnet-5-5", "claude-opus-5-5"]
ANTHROPIC_MAX_TOKENS = 16000
OPENAI_BASE_URL = "https://api.openai.com/v1"
OPENAI_MODELS = ["gpt-5-mini", "gpt-5", "gpt-5-nano"]
_SCHEMA_NAME = re.compile(r"[^A-Za-z0-9_-]+")


class BadRequest(Fatal):
    """HTTP 400/422: the server rejected the request shape (e.g. an unsupported response_format)."""


def _excerpt(response: httpx.Response) -> str:
    try:
        data = response.json()
    except ValueError:
        return " ".join(response.text.split())[:300]
    if isinstance(data, dict):
        err = data.get("error")
        if isinstance(err, dict):
            return str(err.get("message") or err.get("type") or err)[:300]
        if err:
            return str(err)[:300]
        if data.get("message"):
            return str(data["message"])[:300]
    return " ".join(str(data).split())[:300]


class HTTPAdapterLLM(AdapterLLM):
    runtime_id = "http"
    auth_hint = "The server rejected the API key."

    def __init__(self, *, base_url: str, api_key_env: str | None = None, concurrency: int = 4,
                 log_path: Path | None = None, timeout_s: float = 240, retries: int = 2,
                 transport: httpx.AsyncBaseTransport | None = None):
        super().__init__(concurrency=concurrency, log_path=log_path, timeout_s=timeout_s, retries=retries)
        self.base_url = base_url.rstrip("/")
        self.api_key_env = api_key_env
        self._transport = transport

    def api_key(self) -> str | None:
        if not self.api_key_env:
            return None
        value = (os.environ.get(self.api_key_env) or os.environ.get(renamed_key_env(self.api_key_env), "")).strip()
        return value or None

    def headers(self) -> dict[str, str]:
        key = self.api_key()
        return {"Authorization": f"Bearer {key}"} if key else {}

    def client(self, timeout: float | None = None) -> httpx.AsyncClient:
        return httpx.AsyncClient(transport=self._transport, timeout=httpx.Timeout(timeout or self._timeout_s,
                                                                                   connect=10.0),
                                 follow_redirects=False)

    async def request(self, method: str, path: str, body: dict[str, Any] | None = None, *,
                      timeout: float | None = None) -> Any:
        url = f"{self.base_url}{path}"
        try:
            async with self.client(timeout) as client:
                response = await client.request(method, url, json=body, headers=self.headers())
        except httpx.TimeoutException as err:
            raise Transient(f"{self.runtime_id}: request timed out ({type(err).__name__})") from err
        except httpx.HTTPError as err:  # connection refused, DNS, protocol errors
            raise Transient(f"{self.runtime_id}: cannot reach {self.base_url} ({type(err).__name__})") from err
        status = response.status_code
        if status in (401, 403):
            raise Fatal(f"{self.auth_hint} (HTTP {status}: {_excerpt(response)})")
        if status == 404:
            raise Fatal(f"HTTP 404 from {self.base_url}{path}: {_excerpt(response)}")
        if status == 429:
            raise Transient(f"rate limited (HTTP 429): {_excerpt(response)}", rate_limited=True)
        if status in (400, 422):
            raise BadRequest(f"HTTP {status}: {_excerpt(response)}")
        if status >= 400:
            raise Transient(f"HTTP {status}: {_excerpt(response)}", rate_limited=status in (503, 529))
        try:
            return response.json()
        except ValueError as err:
            raise Transient(f"{self.runtime_id}: the server did not return JSON") from err


def ollama_context_length(text: str) -> int:
    """A context window that fits the prompt plus room for the answer. Ollama's default (4k tokens below 24 GB of
    VRAM) is shorter than most Tellfold prompts and silently cuts the middle out of them."""
    needed = len(text) // 3 + 4096  # about 3 characters per token, plus the reply
    size = 8192
    while size < needed and size < 65536:
        size *= 2
    return size


class OllamaLLM(HTTPAdapterLLM):
    """`POST {base}/api/chat` with `stream: false` and `format` = the JSON schema for json() calls."""

    runtime_id = "ollama"

    def __init__(self, *, base_url: str, **kw: Any):
        super().__init__(base_url=base_url, **kw)
        self._default_model: str | None = None

    async def list_models(self) -> list[str]:
        data = await self.request("GET", "/api/tags", timeout=5)
        return [m["name"] for m in data.get("models", []) if isinstance(m, dict) and m.get("name")]

    async def _model(self, request: AdapterRequest) -> str:
        if request.model_or_none:
            return request.model_or_none
        if self._default_model is None:
            models = await self.list_models()
            if not models:
                raise Fatal("Ollama has no models: run `ollama pull llama3.2` (or any model) first")
            self._default_model = models[0]
        return self._default_model

    async def _send(self, request: AdapterRequest) -> AdapterReply:
        model = await self._model(request)
        body: dict[str, Any] = {
            "model": model,
            "stream": False,
            "messages": [{"role": "system", "content": request.system},
                         {"role": "user", "content": request.prompt}],
        }
        if request.json_schema is not None:
            body["format"] = request.json_schema
        body["options"] = {"num_ctx": ollama_context_length(request.system + request.prompt)}
        try:
            data = await self.request("POST", "/api/chat", body)
        except Fatal as err:
            if "not found" in str(err).lower():
                raise Fatal(f"Ollama model {model!r} is not pulled: run `ollama pull {model}`") from err
            raise
        message = data.get("message") if isinstance(data, dict) else None
        content = message.get("content") if isinstance(message, dict) else None
        if not isinstance(content, str):
            raise BadOutput("Ollama returned no message content", reply_text=str(data)[:2000])
        return AdapterReply(text=content, model=data.get("model"), output_tokens=int(data.get("eval_count") or 0))


class OpenAICompatLLM(HTTPAdapterLLM):
    """`POST {base}/chat/completions`. For json(): `response_format` json_schema, falling back to json_object,
    then to the prompt-only JSON contract, when the server rejects a mode (HTTP 400/422). The fallback sticks for
    the life of this adapter."""

    runtime_id = "openai-compatible"
    JSON_MODES = ("json_schema", "json_object", "prompt")

    def __init__(self, *, base_url: str, runtime_id: str | None = None, default_model: str | None = None,
                 **kw: Any):
        super().__init__(base_url=base_url, **kw)
        if runtime_id:
            self.runtime_id = runtime_id
        self._default_model = default_model or None
        self._json_mode = 0

    async def list_models(self) -> list[str]:
        data = await self.request("GET", "/models", timeout=5)
        items = data.get("data", []) if isinstance(data, dict) else []
        return [m["id"] for m in items if isinstance(m, dict) and isinstance(m.get("id"), str)]

    async def _model(self, request: AdapterRequest) -> str:
        if request.model_or_none:
            return request.model_or_none
        if self._default_model is None:
            models = await self.list_models()
            if not models:
                raise Fatal(f"{self.runtime_id}: no model configured and the server lists none")
            self._default_model = models[0]
        return self._default_model

    def _response_format(self, request: AdapterRequest) -> dict[str, Any] | None:
        if request.json_schema is None:
            return None
        mode = self.JSON_MODES[self._json_mode]
        if mode == "json_schema":
            name = _SCHEMA_NAME.sub("_", request.schema_name)[:64] or "result"
            return {"type": "json_schema", "json_schema": {"name": name, "schema": request.json_schema,
                                                           "strict": False}}
        if mode == "json_object":
            return {"type": "json_object"}
        return None

    async def _send(self, request: AdapterRequest) -> AdapterReply:
        model = await self._model(request)
        messages = [{"role": "system", "content": request.system}, {"role": "user", "content": request.prompt}]
        while True:
            body: dict[str, Any] = {"model": model, "messages": messages}
            mode = self._json_mode
            response_format = self._response_format(request)
            if response_format is not None:
                body["response_format"] = response_format
            try:
                data = await self.request("POST", "/chat/completions", body)
            except BadRequest:
                if request.json_schema is not None and mode < len(self.JSON_MODES) - 1:
                    # only the first of several requests refused in this mode moves on; the others retry with the
                    # current mode, so concurrent 400s never skip json_object
                    if self._json_mode == mode:
                        self._json_mode = mode + 1
                    continue
                raise
            return self._reply(data)

    @staticmethod
    def _reply(data: Any) -> AdapterReply:
        try:
            choice = data["choices"][0]
            content = choice["message"].get("content")
        except (KeyError, IndexError, TypeError, AttributeError) as err:
            raise BadOutput("the server returned no choices", reply_text=str(data)[:2000]) from err
        if isinstance(content, list):  # some servers return content parts
            content = "".join(p.get("text", "") for p in content if isinstance(p, dict))
        if not isinstance(content, str) or not content.strip():
            refusal = choice.get("message", {}).get("refusal") if isinstance(choice, dict) else None
            raise BadOutput(f"empty reply{f' (refusal: {refusal})' if refusal else ''}")
        usage = data.get("usage") or {}
        return AdapterReply(text=content, model=data.get("model"),
                            output_tokens=int(usage.get("completion_tokens") or 0))


class OpenAIAPILLM(OpenAICompatLLM):
    runtime_id = "openai-api"
    auth_hint = "OPENAI_API_KEY is missing or invalid."

    def __init__(self, **kw: Any):
        kw.setdefault("base_url", OPENAI_BASE_URL)
        kw.setdefault("api_key_env", "OPENAI_API_KEY")
        super().__init__(runtime_id="openai-api", **kw)

    async def _send(self, request: AdapterRequest) -> AdapterReply:
        if not self.api_key():
            raise Fatal("OPENAI_API_KEY is not set")
        return await super()._send(request)


class AnthropicAPILLM(HTTPAdapterLLM):
    """Messages API (`POST /v1/messages`) with `ANTHROPIC_API_KEY`. Model aliases haiku/sonnet/opus map to the
    current model ids. No sampling parameters and no `thinking` field are sent (current models reject some of
    them); JSON comes from the prompt contract plus validation and repair."""

    runtime_id = "anthropic-api"
    auth_hint = "ANTHROPIC_API_KEY is missing or invalid."

    def __init__(self, **kw: Any):
        kw.setdefault("base_url", ANTHROPIC_BASE_URL)
        kw.setdefault("api_key_env", "ANTHROPIC_API_KEY")
        super().__init__(**kw)

    def headers(self) -> dict[str, str]:
        key = self.api_key()
        headers = {"anthropic-version": ANTHROPIC_VERSION}
        if key:
            headers["x-api-key"] = key
        return headers

    @staticmethod
    def model_id(model: str | None) -> str:
        if not model:
            return ANTHROPIC_MODELS[0]
        return ANTHROPIC_ALIASES.get(model, model)

    async def list_models(self) -> list[str]:
        data = await self.request("GET", "/v1/models", timeout=5)
        return [m["id"] for m in data.get("data", []) if isinstance(m, dict) and m.get("id")]

    async def _send(self, request: AdapterRequest) -> AdapterReply:
        if not self.api_key():
            raise Fatal("ANTHROPIC_API_KEY is not set")
        body = {
            "model": self.model_id(request.model_or_none),
            "max_tokens": ANTHROPIC_MAX_TOKENS,
            "system": request.system,
            "messages": [{"role": "user", "content": request.prompt}],
        }
        try:
            data = await self.request("POST", "/v1/messages", body)
        except BadRequest as err:
            raise Fatal(str(err)) from err
        if not isinstance(data, dict):
            raise BadOutput("unexpected response shape")
        if data.get("stop_reason") == "refusal":
            raise BadOutput("the model declined this request")
        text = "".join(b.get("text", "") for b in data.get("content", []) if isinstance(b, dict)
                       and b.get("type") == "text")
        usage = data.get("usage") or {}
        return AdapterReply(text=text, model=data.get("model"), output_tokens=int(usage.get("output_tokens") or 0))
