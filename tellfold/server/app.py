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
from tellfold.compare import digest_run
from tellfold.llm import LLMError
from tellfold.models import (
    Agreement,
    CompareResult,
    GodEvent,
    KnowledgeGraph,
    Persona,
    Post,
    Report,
    RewriteSuggestion,
    RoundStats,
    RunDigest,
    RunStatus,
    SimConfig,
)
from tellfold.runner import now_iso
from tellfold.server.manager import (
    BadRequest,
    Conflict,
    LLMFactory,
    ReadOnly,
    RunManager,
    RuntimeReady,
    history_pairs,
)
from tellfold.server.runtime_routes import register_runtime_routes, runtime_form_fields
from tellfold.settings import env, load_settings
from tellfold.study import STUDY_ID, StudyError, join_study, plan_variant

WEB_DIR = Path(__file__).resolve().parent.parent / "web"
STATIC_DIR = WEB_DIR / "static"
INDEX_HTML = WEB_DIR / "index.html"

SEED_SUFFIXES = {".pdf", ".md", ".markdown", ".txt"}
MAX_FILE_BYTES = 20 * 1024 * 1024
MAX_FILES = 100  # a whole folder of project docs
MAX_TOTAL_BYTES = 100 * 1024 * 1024
MAX_FORM_BYTES = MAX_TOTAL_BYTES + 1024 * 1024
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
    example: bool = False  # a built-in example run: read-only
    study_id: str | None = None  # the study the run was made for ("Try another version…")
    variant: str | None = None  # its label in that study


class CallTotals(BaseModel):
    count: int = 0
    failed: int = 0
    seconds: float = 0.0


class CallsSummary(CallTotals):
    agent: CallTotals = Field(default_factory=CallTotals)  # engine.* and chat.interview calls
    planner: CallTotals = Field(default_factory=CallTotals)  # graph, personas, report and ask calls


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
    example: bool = False  # a built-in example run: events, stop and report rebuild are refused


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


class StudyMemberOut(BaseModel):
    id: str
    variant: str
    letter: str  # citation prefix in the comparison: A#12 is post 12 of run A


class StudySummary(BaseModel):
    id: str
    kind: str
    created_at: str
    question: str
    runs: list[StudyMemberOut]
    has_compare: bool  # compare.json, or agreement.json for a repeat study
    comparing: bool
    planned: int = 0  # runs a "Check it" study will have


class StudyRunOut(StudyMemberOut):
    question: str
    state: str
    has_report: bool
    example: bool = False
    digest: RunDigest


class CompareState(BaseModel):
    running: bool = False
    error: str | None = None  # why the last comparison in this server failed


class StudyDetail(BaseModel):
    id: str
    kind: str
    created_at: str
    runs: list[StudyRunOut]
    compare: CompareResult | None
    compare_state: CompareState
    planned: int = 0  # runs a "Check it" study will have; more than `runs` while the series is going
    repeating: bool = False  # the "Check it" series is still starting runs
    agreement: Agreement | None = None  # what held across the runs of a repeat study


class BranchIn(BaseModel):
    round: int = Field(ge=0)
    events: list[EventIn] = Field(default_factory=list, max_length=50)  # what happens after `round`


class BranchOut(BaseModel):
    id: str
    study_id: str


class CheckIn(BaseModel):
    runs: int = Field(3, ge=2, le=5)
    agent_model: str | None = Field(None, max_length=120, pattern=_MODEL.pattern)


class CheckOut(BaseModel):
    study_id: str


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
    total = 0
    for upload in uploads:
        name = safe_filename(upload.filename or "seed", taken)
        target = dest / name
        size = 0
        with target.open("wb") as out:
            while chunk := await upload.read(1024 * 1024):
                size += len(chunk)
                if size > MAX_FILE_BYTES:
                    raise HTTPException(413, f"{upload.filename} is larger than 20 MB")
                if total + size > MAX_TOTAL_BYTES:
                    raise HTTPException(413, "the seed files add up to more than 100 MB")
                out.write(chunk)
        if size == 0:
            raise HTTPException(400, f"{upload.filename} is empty")
        total += size
        saved.append(target)
    return saved


def _sse(name: str, data: str) -> str:
    return f"event: {name}\ndata: {data}\n\n"


# ---------- app ----------


def runtime_installed(settings_path: Path | None) -> RuntimeReady:
    """Whether the AI tools the settings name for agents and planner are installed (no model call)."""

    async def ready() -> bool:
        from tellfold.runtimes import UnknownRuntime, get_runtime

        settings = load_settings(settings_path)

        def probe() -> bool:
            try:
                return all(get_runtime(rid, settings).detect(check_auth=False).installed
                           for rid in {settings.agent_runtime, settings.planner_runtime})
            except (UnknownRuntime, OSError):
                return False

        return await asyncio.to_thread(probe)

    return ready


def create_app(runs_dir: Path, llm_factory: LLMFactory, *, fake_llm: bool | None = None,
               allowed_hosts: Iterable[str] | None = None, poll_interval: float = 1.0,
               shutdown_grace: float = 5.0, settings_path: Path | None = None,
               examples_dir: Path | None = None, runtime_ready: RuntimeReady | None = None) -> FastAPI:
    """The Tellfold Studio app. `llm_factory(run_dir, concurrency, think) -> LLM` makes the model backend.

    `examples_dir` holds read-only example runs. `runtime_ready` says whether an AI tool is set up, which
    interviewing an example needs; by default the settings' tools must be installed (always true with a fake LLM)."""
    runs_dir = Path(runs_dir)
    fake = fake_mode() if fake_llm is None else fake_llm
    if runtime_ready is None and not fake:
        runtime_ready = runtime_installed(settings_path)
    manager = RunManager(runs_dir=runs_dir, llm_factory=llm_factory, poll_interval=poll_interval,
                         shutdown_grace=shutdown_grace, settings_path=settings_path,
                         examples_dir=Path(examples_dir) if examples_dir else None, runtime_ready=runtime_ready)

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
        if manager.is_example(run_id):
            return manager.run_path(run_id)
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
        live = manager.live_ids()
        examples = manager.examples()
        out = []
        for run_id in examples:
            with manager.reader(run_id) as r:
                summary = r.summary(live=False)
            if summary is not None:
                out.append({**summary, "example": True})
        for path in runs_dir.iterdir() if runs_dir.is_dir() else ():
            # a normal run can never take an example's id: the example would shadow it on every route
            if not path.is_dir() or not _RUN_ID.match(path.name) or path.name in examples:
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
            study = None
            cast_from = _str_field(form, "cast_from", "", max_len=200, pattern=_RUN_ID)
            study_id = _str_field(form, "study_id", "", max_len=100, pattern=STUDY_ID) or None
            variant = _str_field(form, "variant", "", max_len=60) or None
            if cast_from:
                source = cast_source(cast_from)
                try:
                    study, variant = plan_variant(runs_dir, cast_from, source, study_id=study_id, variant=variant,
                                                  now=now_iso())
                except StudyError as e:
                    raise HTTPException(400, str(e)) from None
                config_fields.update(cast_from=cast_from, study_id=study.id, variant=variant,
                                     num_agents=source.num_agents)
            elif study_id or variant:
                raise HTTPException(400, "study_id and variant need cast_from: a study compares runs of one crowd")
            run_dir = manager.create_run_dir(question)
            try:
                seeds = await _save_uploads(uploads, run_dir / "seeds")
            except BaseException:
                _remove_tree(run_dir)
                raise
        finally:
            await form.close()
        config = SimConfig(seed_files=[str(p) for p in seeds], **config_fields)
        if study is not None:
            join_study(runs_dir, study, run_dir.name, config.variant or "")
        run_id = await manager.start(run_dir, config, seeds)
        return {"id": run_id}

    def cast_source(run_id: str) -> SimConfig:
        """The config of the run whose people a new run reuses; 400 unless it got as far as having people."""
        if ".." in run_id:
            raise HTTPException(400, "invalid cast_from")
        path = manager.run_path(run_id)
        if not manager.is_example(run_id) and path.resolve().parent != runs_dir.resolve():
            raise HTTPException(400, "invalid cast_from")
        with manager.reader(run_id) as r:
            config = r.config()
        if config is None:
            raise HTTPException(400, f"cast_from: there is no run {run_id}")
        if not (path / "personas.json").is_file() or not (path / "graph.json").is_file():
            raise HTTPException(400, f"cast_from: {run_id} has no people yet")
        return config

    # ----- studies -----

    def study_or_404(study_id: str):
        if not STUDY_ID.match(study_id) or ".." in study_id:
            raise HTTPException(400, "invalid study id")
        study = manager.study(study_id)
        if study is None:
            raise HTTPException(404, "study not found")
        return study

    @app.get("/api/studies", response_model=list[StudySummary])
    async def list_studies() -> list[dict]:
        from tellfold.study import list_studies as studies_on_disk

        out = []
        for study in studies_on_disk(runs_dir):
            members = manager.study_members(study)
            if not members:
                continue
            with manager.reader(members[0].run_id) as r:
                config = r.config()
            out.append({"id": study.id, "kind": study.kind, "created_at": study.created_at,
                        "question": config.question if config else "",
                        "runs": [{"id": m.run_id, "variant": m.variant, "letter": m.letter} for m in members],
                        "has_compare": (manager.agreement(study.id) if study.kind == "repeat"
                                        else manager.compare_result(study.id)) is not None,
                        "comparing": manager.compare_state(study.id)["running"] or manager.is_repeating(study.id),
                        "planned": study.planned})
        out.sort(key=lambda s: (s["created_at"], s["id"]), reverse=True)
        return out

    @app.get("/api/studies/{study_id}", response_model=StudyDetail)
    async def get_study(study_id: str) -> dict:
        study = study_or_404(study_id)
        runs = []
        for m in manager.study_members(study):
            with manager.reader(m.run_id) as r:
                summary = r.summary(live=manager.is_live(m.run_id))
            if summary is None:
                continue
            runs.append({"id": m.run_id, "variant": m.variant, "letter": m.letter, "question": summary["question"],
                         "state": summary["state"], "has_report": summary["has_report"],
                         "example": manager.is_example(m.run_id), "digest": digest_run(m, summary["state"])})
        return {"id": study.id, "kind": study.kind, "created_at": study.created_at, "runs": runs,
                "compare": manager.compare_result(study.id), "compare_state": manager.compare_state(study.id),
                "planned": study.planned, "repeating": manager.is_repeating(study.id),
                "agreement": manager.agreement(study.id)}

    @app.post("/api/runs/{run_id}/branch", response_model=BranchOut)
    async def branch(run_id: str, body: BranchIn) -> dict:
        """Continue a copy of the run from the end of `round` with new events; it joins a study with its source."""
        run_dir_of(run_id)
        events = [GodEvent(round=e.round, content=e.content.strip()) for e in body.events if e.content.strip()]
        try:
            new_id, study_id = await manager.branch(run_id, body.round, events)
        except BadRequest as e:
            raise HTTPException(400, str(e)) from None
        except Conflict as e:
            raise HTTPException(409, str(e)) from None
        return {"id": new_id, "study_id": study_id}

    @app.post("/api/runs/{run_id}/rewrite", response_model=RewriteSuggestion)
    async def rewrite(run_id: str) -> RewriteSuggestion:
        """Suggest a better version of the run's text, saved as rewrite.json (one planner call)."""
        run_dir_of(run_id)
        try:
            return await manager.rewrite(run_id)
        except BadRequest as e:
            raise HTTPException(400, str(e)) from None
        except Conflict as e:
            raise HTTPException(409, str(e)) from None
        except LLMError as e:
            raise HTTPException(502, f"the model call failed: {e}") from None

    @app.get("/api/runs/{run_id}/rewrite", response_model=RewriteSuggestion)
    async def saved_rewrite(run_id: str) -> RewriteSuggestion:
        run_dir_of(run_id)
        suggestion = manager.saved_rewrite(run_id)
        if suggestion is None:
            raise HTTPException(404, "no suggestion for this run yet")
        return suggestion

    @app.post("/api/runs/{run_id}/check", response_model=CheckOut)
    async def check_run(run_id: str, body: CheckIn) -> dict:
        """Check it: run this finished run again with the same people and text, one run after another."""
        run_dir_of(run_id)
        try:
            study = await manager.check(run_id, body.runs, body.agent_model)
        except BadRequest as e:
            raise HTTPException(400, str(e)) from None
        except Conflict as e:
            raise HTTPException(409, str(e)) from None
        return {"study_id": study.id}

    @app.post("/api/studies/{study_id}/compare", response_model=Ok)
    async def compare_study(study_id: str) -> dict:
        study_or_404(study_id)
        try:
            manager.compare(study_id)
        except Conflict as e:
            raise HTTPException(409, str(e)) from None
        return {"ok": True}

    @app.get("/api/runs/{run_id}", response_model=RunDetail)
    async def get_run(run_id: str) -> dict:
        run_dir_of(run_id)
        with manager.reader(run_id) as r:
            detail = r.detail(live=manager.is_live(run_id))
        if detail is None:
            raise HTTPException(404, "run not found")
        if detail["status"] is None:
            detail["status"] = manager.effective_status(run_id)
        detail["report_rebuild"] = manager.rebuild_state(run_id)
        detail["example"] = manager.is_example(run_id)
        return detail

    @app.get("/api/runs/{run_id}/posts", response_model=list[PostOut])
    async def get_posts(run_id: str, since_id: int = Query(0, ge=0)) -> list[dict]:
        run_dir_of(run_id)
        with manager.reader(run_id) as r:
            return r.posts(since_id)

    @app.get("/api/runs/{run_id}/actions", response_model=list[ActionOut])
    async def get_actions(run_id: str, round: int | None = Query(None, ge=0)) -> list[dict]:
        run_dir_of(run_id)
        with manager.reader(run_id) as r:
            return r.actions(round)

    @app.get("/api/runs/{run_id}/agents/{agent_id}", response_model=AgentDetail)
    async def get_agent(run_id: str, agent_id: str) -> dict:
        run_dir_of(run_id)
        if not _AGENT_ID.match(agent_id):
            raise HTTPException(400, "invalid agent id")
        with manager.reader(run_id) as r:
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
        run_dir_of(run_id)
        try:
            manager.inject_event(run_id, body.round, body.content)
        except ReadOnly as e:
            raise HTTPException(403, str(e)) from None
        except Conflict as e:
            raise HTTPException(409, str(e)) from None
        except BadRequest as e:
            raise HTTPException(400, str(e)) from None
        return {"ok": True}

    @app.post("/api/runs/{run_id}/stop", response_model=Ok)
    async def stop_run(run_id: str) -> dict:
        run_dir_of(run_id)
        try:
            manager.stop(run_id)
        except ReadOnly as e:
            raise HTTPException(403, str(e)) from None
        except Conflict as e:
            raise HTTPException(409, str(e)) from None
        return {"ok": True}

    @app.post("/api/runs/{run_id}/chat", response_model=Reply)
    async def chat(run_id: str, body: ChatIn) -> dict:
        run_dir_of(run_id)
        if not _AGENT_ID.match(body.agent_id):
            raise HTTPException(400, "invalid agent id")
        try:
            history = history_pairs(body.history)[-20:]
            reply = await manager.chat(run_id, body.agent_id, body.message.strip(), history)
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
        run_dir_of(run_id)
        try:
            reply = await manager.ask(run_id, body.question.strip())
        except Conflict as e:
            raise HTTPException(409, str(e)) from None
        except LLMError as e:
            raise HTTPException(502, f"the model call failed: {e}") from None
        return {"reply": reply}

    @app.post("/api/runs/{run_id}/report", response_model=Ok)
    async def rebuild_report(run_id: str) -> dict:
        run_dir_of(run_id)
        try:
            manager.rebuild_report(run_id)
        except ReadOnly as e:
            raise HTTPException(403, str(e)) from None
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
        run_dir_of(run_id)
        rid = run_id
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
