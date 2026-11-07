"""Runtime and settings endpoints, shared by `tellfold ui` and `tellfold serve`.

GET  /api/runtimes?check_auth=0|1   -> [RuntimeInfo]
GET  /api/settings                  -> Settings
PUT  /api/settings                  -> Settings (validated, saved atomically)
POST /api/runtimes/{id}/test        -> {ok, seconds, reply, error, model}   (one tiny real call)
"""

from __future__ import annotations

import re
from pathlib import Path
from typing import Any

import anyio
from fastapi import FastAPI, HTTPException, Query
from pydantic import BaseModel, Field

from tellfold.runtimes import RuntimeInfo, UnknownRuntime, detect_all, known_ids, test_runtime
from tellfold.settings import Settings, load_settings, save_settings

_RUNTIME_ID = re.compile(r"^[a-z0-9][a-z0-9._-]{0,39}(:[a-z0-9][a-z0-9_-]{0,39})?$")
_MODEL = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:/@\[\]-]{0,119}$")


class RuntimeTestIn(BaseModel):
    model: str | None = Field(default=None, max_length=120)


class RuntimeTestOut(BaseModel):
    ok: bool
    seconds: float
    reply: str | None = None
    error: str | None = None
    model: str | None = None


def check_runtime_id(runtime_id: str, settings: Settings, *, field: str = "runtime") -> str:
    """400 for a malformed or unknown runtime id; returns the id."""
    runtime_id = runtime_id.strip()
    if not _RUNTIME_ID.match(runtime_id):
        raise HTTPException(400, f"invalid {field}")
    if runtime_id not in known_ids(settings):
        raise HTTPException(400, f"unknown {field} {runtime_id!r}; see GET /api/runtimes")
    return runtime_id


def runtime_form_fields(form: Any, settings: Settings) -> dict[str, str]:
    """`agent_runtime` / `planner_runtime` from a POST /api/runs form; settings defaults when omitted."""
    out: dict[str, str] = {}
    for key, default in (("agent_runtime", settings.agent_runtime), ("planner_runtime", settings.planner_runtime)):
        raw = form.get(key)
        value = raw.strip() if isinstance(raw, str) and raw.strip() else default
        out[key] = check_runtime_id(value, settings, field=key)
    return out


def register_runtime_routes(app: FastAPI, *, settings_path: Path | None, fake: bool) -> None:
    """Add the routes to `app`. Call before any catch-all `/api/{rest}` route is registered."""

    def current() -> Settings:
        return load_settings(settings_path)

    @app.get("/api/runtimes", response_model=list[RuntimeInfo])
    async def list_runtimes(check_auth: int = Query(0, ge=0, le=1)) -> list[RuntimeInfo]:
        settings = current()
        return await anyio.to_thread.run_sync(lambda: detect_all(settings, check_auth=bool(check_auth)))

    @app.get("/api/settings", response_model=Settings)
    async def get_settings() -> Settings:
        return current()

    @app.put("/api/settings", response_model=Settings)
    async def put_settings(body: Settings) -> Settings:
        check_runtime_id(body.agent_runtime, body, field="agent_runtime")
        check_runtime_id(body.planner_runtime, body, field="planner_runtime")
        try:
            save_settings(body, settings_path)
        except OSError as e:
            raise HTTPException(500, f"could not save settings: {e.strerror or e}") from None
        return body

    @app.post("/api/runtimes/{runtime_id}/test", response_model=RuntimeTestOut)
    async def test(runtime_id: str, body: RuntimeTestIn | None = None) -> dict[str, Any]:
        if not _RUNTIME_ID.match(runtime_id):
            raise HTTPException(400, "invalid runtime id")
        model = body.model.strip() if body and body.model and body.model.strip() else None
        if model is not None and not _MODEL.match(model):
            raise HTTPException(400, "invalid model")
        settings = current()
        if runtime_id not in known_ids(settings):
            raise HTTPException(404, "runtime not found")
        if fake:
            return {"ok": True, "seconds": 0.0, "reply": "ok (fake LLM)", "error": None, "model": model}
        try:
            return await test_runtime(runtime_id, settings, model=model)
        except UnknownRuntime:
            raise HTTPException(404, "runtime not found") from None
