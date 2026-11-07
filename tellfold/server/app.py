"""Tellfold Studio HTTP API: a FastAPI app serving the JSON API, the SSE stream and the single-page frontend.

Every route handler is `async def`: the server is one asyncio thread, and `World` holds sqlite3 connections
that must stay on it. The app is meant to bind 127.0.0.1 only; `LocalOnlyMiddleware` additionally rejects
foreign Host headers (DNS rebinding) and cross-origin writes (a web page posting a form to localhost).
"""

from __future__ import annotations

import asyncio
import json
import os
import re
import unicodedata
from collections.abc import AsyncIterator, Iterable
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from fastapi import FastAPI, HTTPException, Query, Request
from fastapi.responses import FileResponse, HTMLResponse, JSONResponse, Response, StreamingResponse
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel, Field, ValidationError
from starlette.datastructures import UploadFile
from starlette.types import ASGIApp, Receive, Scope, Send

from tellfold import __version__
from tellfold.llm import LLMError
from tellfold.models import (
    GodEvent,
    KnowledgeGraph,
    Persona,
    Post,
    Report,
    RoundStats,
    RunStatus,
    SimConfig,
)
from tellfold.server.manager import BadRequest, Conflict, LLMFactory, RunManager, history_pairs
from tellfold.server.runtime_routes import register_runtime_routes, runtime_form_fields
from tellfold.settings import env, load_settings

WEB_DIR = Path(__file__).resolve().parent.parent / "web"
STATIC_DIR = WEB_DIR / "static"
INDEX_HTML = WEB_DIR / "index.html"

SEED_SUFFIXES = {".pdf", ".md", ".markdown", ".txt"}
MAX_FILE_BYTES = 20 * 1024 * 1024
MAX_FILES = 20
MAX_FORM_BYTES = MAX_FILES * MAX_FILE_BYTES + 1024 * 1024
PING_SECONDS = 15.0
DISCONNECT_CHECK_SECONDS = 1.0
DEFAULT_HOSTS = ("127.0.0.1", "localhost", "::1", "testserver")
FAKE_ENV = "TELLFOLD_FAKE_LLM"

_RUN_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,199}$")
_AGENT_ID = re.compile(r"^@?[A-Za-z0-9_.-]{1,64}$")
_MODEL = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:/@\[\]-]{0,119}$")  # "/" for provider/model names
_UNSAFE_NAME = re.compile(r"[^A-Za-z0-9._ -]+")


# ---------- API shapes ----------


class RunSummary(BaseModel):
    id: str
    question: str
    state: str
    stage: str
    round: int
    total_rounds: int
    agents: int
    created_at: str
    has_report: bool


class CallsSummary(BaseModel):
    count: int = 0
    failed: int = 0
    seconds: float = 0.0


class ReportRebuild(BaseModel):
    running: bool = False
    error: str | None = None  # why the last rebuild started in this server failed


class RunDetail(BaseModel):
    id: str
    config: SimConfig
    status: RunStatus | None
    live: bool
    personas: list[Persona]
    graph: KnowledgeGraph | None
    stats: list[RoundStats]
    events: list[GodEvent]
    follows: list[list[str]]
    report: Report | None
    calls: CallsSummary
    report_rebuild: ReportRebuild = Field(default_factory=ReportRebuild)


class PostOut(Post):
    author_handle: str
    sentiment: float | None = None


class ActionOut(BaseModel):
    round: int
    agent_id: str
    type: str
    target_post_id: int | None = None
    target_agent_id: str | None = None
    content: str = ""
    reasoning: str = ""


class AgentDetail(BaseModel):
    persona: Persona
    memory: list[str]
    posts: list[PostOut]
    followers: list[str]
    following: list[str]
    sentiment_by_round: list[tuple[int, float]]


class Health(BaseModel):
    ok: bool
    fake_llm: bool
    version: str


class Created(BaseModel):
    id: str


class Ok(BaseModel):
    ok: bool = True


class Reply(BaseModel):
    reply: str


class EventIn(BaseModel):
    round: int
    content: str = Field(min_length=1, max_length=2000)


class ChatIn(BaseModel):
    agent_id: str = Field(min_length=1, max_length=64)
    message: str = Field(min_length=1, max_length=4000)
    history: list[Any] = Field(default_factory=list, max_length=50)


class AskIn(BaseModel):
    question: str = Field(min_length=1, max_length=4000)


# ---------- local-only guard ----------


class LocalOnlyMiddleware:
    """Reject requests whose Host is not local (DNS rebinding) and writes whose Origin is another site."""

    def __init__(self, app: ASGIApp, allowed_hosts: Iterable[str]):
        self.app = app
        self.allowed = {h.lower().strip("[]") for h in allowed_hosts}

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] == "http":
            headers = {k.lower(): v.decode("latin-1") for k, v in scope.get("headers", [])}
            host = headers.get(b"host", "")
            if _hostname(host) not in self.allowed:
                await JSONResponse({"detail": "forbidden host"}, status_code=403)(scope, receive, send)
                return
            origin = headers.get(b"origin")
            if scope.get("method") not in ("GET", "HEAD", "OPTIONS") and origin is not None:
                if origin == "null" or urlsplit(origin).netloc.lower() != host.lower():
                    await JSONResponse({"detail": "cross-origin request refused"}, status_code=403)(
                        scope, receive, send)
                    return
        await self.app(scope, receive, send)


def _hostname(host: str) -> str:
    try:
        return (urlsplit("//" + host).hostname or "").lower()
    except ValueError:
        return ""


# ---------- helpers ----------


def fake_mode() -> bool:
    return (env(FAKE_ENV) or "").strip().lower() in {"1", "true", "yes", "on"}


def safe_filename(name: str, taken: set[str]) -> str:
    """A plain file name for an upload: no directories, no odd characters, no hidden files, unique."""
    base = name.replace("\\", "/").rsplit("/", 1)[-1]
    base = unicodedata.normalize("NFKD", base).encode("ascii", "ignore").decode("ascii")
    stem, dot, suffix = base.rpartition(".")
    if not dot:
        stem, suffix = base, ""
    stem = _UNSAFE_NAME.sub("_", stem).strip(" ._-")[:80] or "seed"
    suffix = suffix.lower()
    candidate, n = f"{stem}.{suffix}", 2
    while candidate.lower() in taken:
        candidate, n = f"{stem}-{n}.{suffix}", n + 1
    taken.add(candidate.lower())
    return candidate


def _int_field(form: Any, key: str, default: int, low: int, high: int) -> int:
    raw = form.get(key)
    if raw is None or (isinstance(raw, str) and raw.strip() == ""):
        return default
    try:
        value = int(str(raw).strip())
    except ValueError:
        raise HTTPException(400, f"{key} must be a whole number") from None
    if not low <= value <= high:
        raise HTTPException(400, f"{key} must be between {low} and {high}")
    return value


def _str_field(form: Any, key: str, default: str, *, max_len: int, pattern: re.Pattern | None = None) -> str:
    raw = form.get(key)
    if raw is None or not isinstance(raw, str) or not raw.strip():
        return default
    value = raw.strip()
    if len(value) > max_len or (pattern is not None and not pattern.match(value)):
        raise HTTPException(400, f"invalid {key}")
    return value


def _bool_field(form: Any, key: str) -> bool:
    raw = form.get(key)
    return isinstance(raw, str) and raw.strip().lower() in {"1", "true", "yes", "on"}


def _events_field(form: Any, rounds: int) -> list[GodEvent]:
    raw = form.get("events")
    if raw is None or not isinstance(raw, str) or not raw.strip():
        return []
    try:
        items = json.loads(raw)
        if not isinstance(items, list):
            raise ValueError
        events = [GodEvent.model_validate(item) for item in items]
    except (ValueError, ValidationError):
        raise HTTPException(400, "events must be a JSON list of {round, content}") from None
    out = []
    for e in events:
        content = e.content.strip()
        if not content:
            continue
        if not 1 <= e.round <= rounds:
            raise HTTPException(400, f"event round {e.round} is outside 1..{rounds}")
        if len(content) > 2000:
            raise HTTPException(400, "event text is too long (max 2000 characters)")
        out.append(GodEvent(round=e.round, content=content))
    return out


async def _save_uploads(uploads: list[UploadFile], dest: Path) -> list[Path]:
    dest.mkdir(parents=True, exist_ok=True)
    taken: set[str] = set()
    saved: list[Path] = []
    for upload in uploads:
        name = safe_filename(upload.filename or "seed", taken)
        target = dest / name
        size = 0
        with target.open("wb") as out:
            while chunk := await upload.read(1024 * 1024):
                size += len(chunk)
                if size > MAX_FILE_BYTES:
                    raise HTTPException(413, f"{upload.filename} is larger than 20 MB")
                out.write(chunk)
        if size == 0:
            raise HTTPException(400, f"{upload.filename} is empty")
        saved.append(target)
    return saved


def _sse(name: str, data: str) -> str:
    return f"event: {name}\ndata: {data}\n\n"


# ---------- app ----------


def create_app(runs_dir: Path, llm_factory: LLMFactory, *, fake_llm: bool | None = None,
               allowed_hosts: Iterable[str] | None = None, poll_interval: float = 1.0,
               shutdown_grace: float = 5.0, settings_path: Path | None = None) -> FastAPI:
    """The Tellfold Studio app. `llm_factory(run_dir, concurrency, think) -> LLM` makes the model backend."""
    runs_dir = Path(runs_dir)
    manager = RunManager(runs_dir=runs_dir, llm_factory=llm_factory, poll_interval=poll_interval,
                         shutdown_grace=shutdown_grace)
    fake = fake_mode() if fake_llm is None else fake_llm

    @asynccontextmanager
    async def lifespan(_: FastAPI) -> AsyncIterator[None]:
        runs_dir.mkdir(parents=True, exist_ok=True)
        yield
        await manager.shutdown()

    app = FastAPI(title="Tellfold Studio", version=__version__, lifespan=lifespan, docs_url="/api/docs",
                  redoc_url=None, openapi_url="/api/openapi.json")
    app.state.manager = manager
    app.state.runs_dir = runs_dir
    app.add_middleware(LocalOnlyMiddleware, allowed_hosts=allowed_hosts or DEFAULT_HOSTS)

    def run_dir_of(run_id: str) -> Path:
        if not _RUN_ID.match(run_id) or ".." in run_id:
            raise HTTPException(400, "invalid run id")
        path = runs_dir / run_id
        if path.resolve().parent != runs_dir.resolve() or not (path / "config.json").is_file():
            raise HTTPException(404, "run not found")
        return path

    # ----- runs -----

    @app.get("/api/health", response_model=Health)
    async def health() -> dict:
        return {"ok": True, "fake_llm": fake, "version": __version__}

    @app.get("/api/runs", response_model=list[RunSummary])
    async def list_runs() -> list[dict]:
        if not runs_dir.is_dir():
            return []
        live = manager.live_ids()
        out = []
        for path in runs_dir.iterdir():
            if not path.is_dir() or not _RUN_ID.match(path.name):
                continue
            with manager.reader(path.name) as r:
                summary = r.summary(live=path.name in live)
            if summary is not None:
                out.append(summary)
        out.sort(key=lambda s: (s["created_at"], s["id"]), reverse=True)
        return out

    @app.post("/api/runs", response_model=Created)
    async def create_run(request: Request) -> dict:
        length = request.headers.get("content-length")
        if length and length.isdigit() and int(length) > MAX_FORM_BYTES:
            raise HTTPException(413, "upload too large")
        try:
            form = await request.form(max_files=MAX_FILES, max_fields=100)
        except HTTPException:
            raise
        except Exception as e:  # noqa: BLE001 - malformed multipart
            raise HTTPException(400, f"could not read the form: {e}") from None
        try:
            uploads = [f for key in ("files[]", "files") for f in form.getlist(key) if isinstance(f, UploadFile)]
            if not uploads:
                raise HTTPException(400, "add at least one seed file (.pdf, .md, .markdown or .txt)")
            if len(uploads) > MAX_FILES:
                raise HTTPException(400, f"at most {MAX_FILES} seed files")
            for upload in uploads:
                suffix = Path((upload.filename or "").replace("\\", "/")).suffix.lower()
                if suffix not in SEED_SUFFIXES:
                    raise HTTPException(400, f"{upload.filename!r}: only .pdf, .md, .markdown and .txt are supported")
                if upload.size is not None and upload.size > MAX_FILE_BYTES:
                    raise HTTPException(413, f"{upload.filename} is larger than 20 MB")
            question = form.get("question")
            if not isinstance(question, str) or not question.strip():
                raise HTTPException(400, "question is required")
            question = " ".join(question.split())
            if len(question) > 2000:
                raise HTTPException(400, "question is too long (max 2000 characters)")
            rounds = _int_field(form, "rounds", 6, 1, 50)
            config_fields = dict(
                question=question,
                num_agents=_int_field(form, "agents", 20, 2, 200),
                rounds=rounds,
                batch_size=_int_field(form, "batch", 6, 1, 20),
                agent_model=_str_field(form, "agent_model", "haiku", max_len=120, pattern=_MODEL),
                planner_model=_str_field(form, "planner_model", "sonnet", max_len=120, pattern=_MODEL),
                concurrency=_int_field(form, "concurrency", 4, 1, 16),
                language=_str_field(form, "language", "English", max_len=40),
                think=_bool_field(form, "think"),
                events=_events_field(form, rounds),
                seed=_int_field(form, "seed", 7, 0, 2**31 - 1),
            )
            config_fields.update(runtime_form_fields(form, load_settings(settings_path)))
            run_dir = manager.create_run_dir(question)
            try:
                seeds = await _save_uploads(uploads, run_dir / "seeds")
            except BaseException:
                _remove_tree(run_dir)
                raise
        finally:
            await form.close()
        config = SimConfig(seed_files=[str(p) for p in seeds], **config_fields)
        run_id = await manager.start(run_dir, config, seeds)
        return {"id": run_id}

    @app.get("/api/runs/{run_id}", response_model=RunDetail)
    async def get_run(run_id: str) -> dict:
        path = run_dir_of(run_id)
        with manager.reader(path.name) as r:
            detail = r.detail(live=manager.is_live(run_id))
        if detail is None:
            raise HTTPException(404, "run not found")
        if detail["status"] is None:
            detail["status"] = manager.effective_status(run_id)
        detail["report_rebuild"] = manager.rebuild_state(path.name)
        return detail

    @app.get("/api/runs/{run_id}/posts", response_model=list[PostOut])
    async def get_posts(run_id: str, since_id: int = Query(0, ge=0)) -> list[dict]:
        path = run_dir_of(run_id)
        with manager.reader(path.name) as r:
            return r.posts(since_id)

    @app.get("/api/runs/{run_id}/actions", response_model=list[ActionOut])
    async def get_actions(run_id: str, round: int | None = Query(None, ge=0)) -> list[dict]:
        path = run_dir_of(run_id)
        with manager.reader(path.name) as r:
            return r.actions(round)

    @app.get("/api/runs/{run_id}/agents/{agent_id}", response_model=AgentDetail)
    async def get_agent(run_id: str, agent_id: str) -> dict:
        path = run_dir_of(run_id)
        if not _AGENT_ID.match(agent_id):
            raise HTTPException(400, "invalid agent id")
        with manager.reader(path.name) as r:
            agent = r.agent(agent_id)
            if agent is None:  # accept a handle too
                wanted = agent_id.lstrip("@").lower()
                match = next((p.id for p in r.personas() if p.handle.lower() == wanted), None)
                agent = r.agent(match) if match else None
        if agent is None:
            raise HTTPException(404, "agent not found")
        return agent

    @app.post("/api/runs/{run_id}/events", response_model=Ok)
    async def inject_event(run_id: str, body: EventIn) -> dict:
        path = run_dir_of(run_id)
        try:
            manager.inject_event(path.name, body.round, body.content)
        except Conflict as e:
            raise HTTPException(409, str(e)) from None
        except BadRequest as e:
            raise HTTPException(400, str(e)) from None
        return {"ok": True}

    @app.post("/api/runs/{run_id}/stop", response_model=Ok)
    async def stop_run(run_id: str) -> dict:
        path = run_dir_of(run_id)
        try:
            manager.stop(path.name)
        except Conflict as e:
            raise HTTPException(409, str(e)) from None
        return {"ok": True}

    @app.post("/api/runs/{run_id}/chat", response_model=Reply)
    async def chat(run_id: str, body: ChatIn) -> dict:
        path = run_dir_of(run_id)
        if not _AGENT_ID.match(body.agent_id):
            raise HTTPException(400, "invalid agent id")
        try:
            history = history_pairs(body.history)[-20:]
            reply = await manager.chat(path.name, body.agent_id, body.message.strip(), history)
        except BadRequest as e:
            raise HTTPException(400, str(e)) from None
        except Conflict as e:
            raise HTTPException(409, str(e)) from None
        except KeyError:
            raise HTTPException(404, "agent not found") from None
        except LLMError as e:
            raise HTTPException(502, f"the model call failed: {e}") from None
        return {"reply": reply}

    @app.post("/api/runs/{run_id}/ask", response_model=Reply)
    async def ask(run_id: str, body: AskIn) -> dict:
        path = run_dir_of(run_id)
        try:
            reply = await manager.ask(path.name, body.question.strip())
        except Conflict as e:
            raise HTTPException(409, str(e)) from None
        except LLMError as e:
            raise HTTPException(502, f"the model call failed: {e}") from None
        return {"reply": reply}

    @app.post("/api/runs/{run_id}/report", response_model=Ok)
    async def rebuild_report(run_id: str) -> dict:
        path = run_dir_of(run_id)
        try:
            manager.rebuild_report(path.name)
        except Conflict as e:
            raise HTTPException(409, str(e)) from None
        return {"ok": True}

    @app.get("/api/runs/{run_id}/seeds")
    async def list_seeds(run_id: str) -> list[dict]:
        """The seed documents saved with a run, so it can be run again with the same material."""
        folder = run_dir_of(run_id) / "seeds"
        if not folder.is_dir():
            return []
        return [{"name": f.name, "size": f.stat().st_size}
                for f in sorted(folder.iterdir()) if f.is_file() and not f.name.startswith(".")]

    @app.get("/api/runs/{run_id}/seeds/{name}")
    async def get_seed(run_id: str, name: str) -> FileResponse:
        folder = run_dir_of(run_id) / "seeds"
        path = folder / name
        if name != Path(name).name or name.startswith(".") or not path.is_file():
            raise HTTPException(404, "no such seed document")
        return FileResponse(path, filename=name)

    @app.get("/api/runs/{run_id}/report.html")
    async def report_html(run_id: str) -> FileResponse:
        path = run_dir_of(run_id) / "report.html"
        if not path.is_file():
            raise HTTPException(404, "this run has no report yet")
        return FileResponse(path, media_type="text/html", headers={"Cache-Control": "no-cache"})

    @app.get("/api/runs/{run_id}/stream")
    async def stream(run_id: str, request: Request) -> StreamingResponse:
        path = run_dir_of(run_id)
        rid = path.name
        queue = manager.subscribe(rid)

        async def events() -> AsyncIterator[str]:
            loop = asyncio.get_running_loop()
            last_write = loop.time()
            try:
                yield ": connected\n\n"
                while True:
                    try:
                        item = await asyncio.wait_for(queue.get(), timeout=DISCONNECT_CHECK_SECONDS)
                    except TimeoutError:
                        if await request.is_disconnected():
                            return
                        if loop.time() - last_write >= PING_SECONDS:
                            last_write = loop.time()
                            yield ": ping\n\n"
                        continue
                    if item is None:  # server shutting down
                        return
                    name, data = item
                    last_write = loop.time()
                    yield _sse(name, data)
            finally:
                manager.unsubscribe(rid, queue)

        return StreamingResponse(events(), media_type="text/event-stream",
                                 headers={"Cache-Control": "no-cache", "X-Accel-Buffering": "no"})

    # ----- runtimes and settings -----

    register_runtime_routes(app, settings_path=settings_path, fake=fake)

    # ----- frontend -----

    app.mount("/static", StaticFiles(directory=STATIC_DIR, check_dir=False), name="static")

    @app.api_route("/api/{rest:path}", methods=["GET", "POST", "PUT", "PATCH", "DELETE"], include_in_schema=False)
    async def api_not_found(rest: str) -> JSONResponse:
        return JSONResponse({"detail": "not found"}, status_code=404)

    @app.get("/{rest:path}", include_in_schema=False)
    async def spa(rest: str) -> Response:
        if rest == "api" or rest.startswith("api/"):
            return JSONResponse({"detail": "not found"}, status_code=404)
        if INDEX_HTML.is_file():
            return HTMLResponse(INDEX_HTML.read_text(encoding="utf-8"), headers={"Cache-Control": "no-cache"})
        return HTMLResponse("<!doctype html><title>Tellfold Studio</title><p>The Tellfold Studio frontend files are "
                            "missing from this installation (tellfold/web/index.html).</p>", status_code=503)

    return app


def _remove_tree(path: Path) -> None:
    import shutil

    shutil.rmtree(path, ignore_errors=True)
