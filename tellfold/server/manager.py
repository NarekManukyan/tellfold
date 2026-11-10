"""In-process runs, server-sent-event fan-out, and a watcher for runs started elsewhere (e.g. by the CLI).

Everything here runs on the server's single asyncio event loop. A live run is an `asyncio.Task` owned by the
manager, never by a request, so it survives every client disconnecting. Each SSE client gets its own bounded
queue; events from `run_pipeline`'s `on_event` are serialised once and pushed to every queue of that run.

For runs this server is not executing, a per-run watcher task (alive only while someone is subscribed) polls
`status.json` and `world.db` once per `poll_interval` seconds and synthesises `status`, `posts_added`,
`round_done` and terminal events, so watching a CLI run live works too.

An optional read-only examples root (the Mac app's bundled example runs) is listed next to the normal runs. Each
example's id is `example-<folder name>`; examples can be read, streamed and (with an AI tool set up) interviewed,
but never changed: they are opened with SQLite's `immutable` flag and nothing is ever written next to them.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import logging
import threading
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import anyio
from pydantic_core import to_jsonable_python

from tellfold.chat import ask_report, interview
from tellfold.agree import MAX_RUNS, build_agreement
from tellfold.compare import COMPARE_FILE, Member, build_compare
from tellfold.llm import LLM
from tellfold.models import (
    Agreement,
    CompareResult,
    GodEvent,
    KnowledgeGraph,
    RunState,
    RunStatus,
    SimConfig,
    Study,
    StudyRun,
)
from tellfold.report import build_report
from tellfold.runner import (
    SEEDS_DIR,
    copy_seeds,
    create_run_dir,
    now_iso,
    run_pipeline,
    write_json,
    write_report,
    write_status,
)
from tellfold.server.reader import RunReader
from tellfold.settings import load_settings
from tellfold.study import AGREEMENT_FILE, letter, load_study, new_study_id, save_study, study_dir
from tellfold.world import World

log = logging.getLogger(__name__)

LLMFactory = Callable[[Path | None, int, bool], LLM]
RuntimeReady = Callable[[], Awaitable[bool]]
EXAMPLE_PREFIX = "example-"
QUEUE_SIZE = 2000
_CLOSE = None  # sentinel pushed into a client queue to end its stream
_TERMINAL_EVENTS = {RunState.DONE: "run_done", RunState.FAILED: "run_failed", RunState.STOPPED: "run_stopped"}


class Conflict(Exception):
    """The request is valid but the run is not in a state that allows it (HTTP 409)."""


class BadRequest(Exception):
    """Invalid input for the run (HTTP 400)."""


class ReadOnly(Exception):
    """The run is a built-in example, which cannot be changed (HTTP 403)."""


EXAMPLE_READ_ONLY = "this is a built-in example run; it cannot be changed. Use Run again to make your own copy"


@dataclass
class LiveRun:
    run_id: str
    run_dir: Path
    config: SimConfig
    stop: anyio.Event
    task: asyncio.Task | None = None
    current_round: int = 0  # highest round that has started


@dataclass
class _Watch:
    task: asyncio.Task
    last_post_id: int | None = None
    last_db: tuple[float, float, int] | None = None
    last_round: int | None = None
    last_state: RunState | None = None


@dataclass
class _Sent:
    """What the server last told clients about a run, so polling only sends changes."""

    status_json: str | None = None


@dataclass
class RunManager:
    runs_dir: Path
    llm_factory: LLMFactory
    poll_interval: float = 1.0
    shutdown_grace: float = 5.0
    examples_dir: Path | None = None  # read-only example runs, listed as `example-<folder>`
    settings_path: Path | None = None  # whose runtimes and models interviews of examples use
    runtime_ready: RuntimeReady | None = None  # whether an AI tool is set up; None: assume it is
    _live: dict[str, LiveRun] = field(default_factory=dict)
    _subs: dict[str, set[asyncio.Queue]] = field(default_factory=dict)
    _watches: dict[str, _Watch] = field(default_factory=dict)
    _sent: dict[str, _Sent] = field(default_factory=dict)
    _rebuilding: dict[str, asyncio.Task] = field(default_factory=dict)
    _rebuild_errors: dict[str, str] = field(default_factory=dict)  # why the last rebuild failed
    _comparing: dict[str, asyncio.Task] = field(default_factory=dict)  # study id -> compare task
    _compare_errors: dict[str, str] = field(default_factory=dict)  # why the last comparison failed
    _repeating: dict[str, asyncio.Task] = field(default_factory=dict)  # study id -> "Check it" series
    _closing: bool = False  # set by shutdown(): no new run starts
    _loop: asyncio.AbstractEventLoop | None = None
    _loop_thread: int | None = None

    # ---------- lookup ----------

    def is_live(self, run_id: str) -> bool:
        run = self._live.get(run_id)
        return run is not None and (run.task is None or not run.task.done())

    def live_ids(self) -> set[str]:
        return {rid for rid in self._live if self.is_live(rid)}

    def examples(self) -> dict[str, Path]:
        """Example id -> folder, for every folder in the examples root that holds a run."""
        root = self.examples_dir
        if root is None or not root.is_dir():
            return {}
        return {EXAMPLE_PREFIX + path.name: path for path in sorted(root.iterdir())
                if path.is_dir() and not path.name.startswith(".") and (path / "config.json").is_file()}

    def is_example(self, run_id: str) -> bool:
        return run_id.startswith(EXAMPLE_PREFIX) and run_id in self.examples()

    def run_path(self, run_id: str) -> Path:
        """The folder of a run: an example's folder for an example id, else `runs_dir/run_id`."""
        if run_id.startswith(EXAMPLE_PREFIX):
            example = self.examples().get(run_id)
            if example is not None:
                return example
        return self.runs_dir / run_id

    def reader(self, run_id: str) -> RunReader:
        if self.is_example(run_id):
            return RunReader(self.run_path(run_id), run_id=run_id, immutable=True)
        return RunReader(self.runs_dir / run_id)

    def _writable(self, run_id: str) -> None:
        if self.is_example(run_id):
            raise ReadOnly(EXAMPLE_READ_ONLY)

    def effective_status(self, run_id: str) -> RunStatus | None:
        with self.reader(run_id) as r:
            status = r.status(live=self.is_live(run_id))
        live = self._live.get(run_id)
        if status is None and live is not None:
            status = RunStatus(state=RunState.QUEUED, stage="reading", total_rounds=live.config.rounds,
                               started_at=now_iso(), updated_at=now_iso())
        return status

    # ---------- starting / controlling runs ----------

    def create_run_dir(self, question: str) -> Path:
        self.runs_dir.mkdir(parents=True, exist_ok=True)
        return create_run_dir(self.runs_dir, question)

    async def start(self, run_dir: Path, config: SimConfig, seeds: list[Path]) -> str:
        """Start `config` in `run_dir` (already created, seeds already saved) as a background task.

        With `config.cast_from` set, the cast is read from that run's folder (an example's too)."""
        self._bind_loop()
        run_id = run_dir.name
        write_json(run_dir / "config.json", config)
        llm = self.llm_factory(run_dir, config.concurrency, config.think)
        live = LiveRun(run_id=run_id, run_dir=run_dir, config=config, stop=anyio.Event())
        self._live[run_id] = live
        cast_dir = self.run_path(config.cast_from) if config.cast_from else None
        live.task = asyncio.create_task(self._execute(live, llm, seeds, cast_dir), name=f"tellfold-run-{run_id}")
        return run_id

    async def _execute(self, live: LiveRun, llm: LLM, seeds: list[Path], cast_dir: Path | None = None) -> None:
        on_event = self._listener(live)
        try:
            await run_pipeline(llm, live.run_dir, live.config, seeds, on_event=on_event, stop=live.stop,
                               cast_dir=cast_dir)
        except asyncio.CancelledError:
            self._mark_stopped(live.run_dir, "stopped: server shut down")
            raise
        except Exception:  # noqa: BLE001 - the pipeline already recorded the failure in status.json
            log.warning("run %s failed", live.run_id, exc_info=True)
        finally:
            self._live.pop(live.run_id, None)
            self.refresh_status(live.run_id)
        if live.config.study_id:
            self._compare_when_ready(live.config.study_id)

    def _listener(self, live: LiveRun) -> Callable[[str, dict], None]:
        def on_event(name: str, data: dict) -> None:
            if name == "round_start" and isinstance(data.get("round"), int):
                live.current_round = max(live.current_round, data["round"])
            self._call_on_loop(self._dispatch, live.run_id, name, data)

        return on_event

    def _dispatch(self, run_id: str, name: str, data: Any) -> None:
        if name == "status":
            try:
                status = RunStatus.model_validate(data)
            except ValueError:
                return
            self._send_status(run_id, status)
            return
        self.publish(run_id, name, data)

    def inject_event(self, run_id: str, round: int, content: str) -> None:
        self._writable(run_id)
        live = self._live.get(run_id)
        if live is None or not self.is_live(run_id):
            raise Conflict("events can only be injected into a run that is live in this server")
        content = content.strip()
        if not content:
            raise BadRequest("event content is empty")
        if not 1 <= round <= live.config.rounds:
            raise BadRequest(f"round must be between 1 and {live.config.rounds}")
        status = self.effective_status(run_id)
        current = max(live.current_round, status.round if status else 0)
        if round <= current:
            raise Conflict(f"round {round} has already started (current round is {current})")
        world = World(live.run_dir / "world.db", seed=live.config.seed)
        try:
            world.add_event(GodEvent(round=round, content=content))
        finally:
            world.close()

    def stop(self, run_id: str) -> None:
        self._writable(run_id)
        live = self._live.get(run_id)
        if live is None or not self.is_live(run_id):
            raise Conflict("only a run that is live in this server can be stopped")
        live.stop.set()

    # ---------- chat, ask, report ----------

    def _open_world(self, run_id: str) -> tuple[SimConfig, World]:
        with self.reader(run_id) as r:
            config = r.config()
        db = self.run_path(run_id) / "world.db"
        if config is None or not db.is_file():
            raise Conflict("this run has no simulated world yet")
        return config, World(db, seed=config.seed, read_only=self.is_example(run_id))

    async def _talker(self, run_id: str, config: SimConfig) -> tuple[LLM, SimConfig]:
        """The model for interviews and follow-up questions, and the config that names its models.

        A normal run uses its own runtimes. An example uses the AI tool set up on this machine (its recorded
        runtime may not be installed here), logs no calls (its folder is read-only), and needs a tool at all."""
        if not self.is_example(run_id):
            return self.llm_factory(self.runs_dir / run_id, 1, False), config
        config = await self._local_tool(config, "to interview people or ask questions in an example run")
        return self.llm_factory(None, 1, False), config

    async def _local_tool(self, config: SimConfig, purpose: str) -> SimConfig:
        """An example's config with the AI tool and models set up on this machine instead of the recorded ones (the
        recorded runtime may not be installed here). Conflict when no tool is set up; `purpose` finishes that message."""
        if self.runtime_ready is not None and not await self.runtime_ready():
            raise Conflict(f"set up an AI tool first {purpose}")
        settings = load_settings(self.settings_path)
        return config.model_copy(update={
            "agent_runtime": settings.agent_runtime, "planner_runtime": settings.planner_runtime,
            "agent_model": settings.agent_model or config.agent_model,
            "planner_model": settings.planner_model or config.planner_model})

    async def chat(self, run_id: str, agent_id: str, message: str, history: list[tuple[str, str]]) -> str:
        config, world = self._open_world(run_id)
        try:
            ids = {p.id for p in world.personas()}
            if agent_id not in ids:
                raise KeyError(agent_id)
            llm, config = await self._talker(run_id, config)
            return await interview(llm, world, agent_id, message, model=config.agent_model, history=history)
        finally:
            world.close()

    async def ask(self, run_id: str, question: str) -> str:
        with self.reader(run_id) as r:
            report = r.report()
        if report is None:
            raise Conflict("this run has no report yet")
        config, world = self._open_world(run_id)
        try:
            llm, config = await self._talker(run_id, config)
            return await ask_report(llm, world, report, question, model=config.planner_model)
        finally:
            world.close()

    def rebuild_report(self, run_id: str) -> None:
        self._writable(run_id)
        if self.is_live(run_id):
            raise Conflict("the run is still in progress; it writes its own report")
        task = self._rebuilding.get(run_id)
        if task is not None and not task.done():
            raise Conflict("the report is already being rebuilt")
        config, world = self._open_world(run_id)
        world.close()
        self._rebuild_errors.pop(run_id, None)
        self._rebuilding[run_id] = asyncio.create_task(self._rebuild(run_id, config), name=f"tellfold-report-{run_id}")

    async def _rebuild(self, run_id: str, config: SimConfig) -> None:
        run_dir = self.runs_dir / run_id
        with self.reader(run_id) as r:
            graph = r.graph() or KnowledgeGraph()
        world = World(run_dir / "world.db", seed=config.seed)
        try:
            llm = self.llm_factory(run_dir, config.concurrency, config.think)
            report = await build_report(llm, world, graph, config, model=config.planner_model)
            write_report(run_dir, report, world, config)
            self.publish(run_id, "report_rebuilt", {"ok": True})
        except Exception as e:  # noqa: BLE001 - reported to subscribers, the server keeps running
            log.warning("report rebuild for %s failed", run_id, exc_info=True)
            error = str(e) or type(e).__name__
            self._rebuild_errors[run_id] = error
            self.publish(run_id, "report_rebuilt", {"ok": False, "error": error})
        finally:
            world.close()
            self._rebuilding.pop(run_id, None)

    def rebuild_state(self, run_id: str) -> dict[str, Any]:
        """Whether a report rebuild is running, and why the last one failed (None if it did not)."""
        task = self._rebuilding.get(run_id)
        return {"running": task is not None and not task.done(), "error": self._rebuild_errors.get(run_id)}

    # ---------- studies ----------

    def study(self, study_id: str) -> Study | None:
        return load_study(self.runs_dir, study_id)

    def study_members(self, study: Study) -> list[Member]:
        """The study's runs that still exist, lettered in study order (A is the first run ever added)."""
        members = []
        for i, run in enumerate(study.runs):
            path = self.run_path(run.id)
            if (path / "config.json").is_file():
                members.append(Member(letter=letter(i), run_id=run.id, variant=run.variant, run_dir=path,
                                      immutable=self.is_example(run.id)))
        return members

    def run_state(self, run_id: str) -> RunState | None:
        status = self.effective_status(run_id)
        return status.state if status else None

    def compare_result(self, study_id: str) -> CompareResult | None:
        try:
            path = study_dir(self.runs_dir, study_id) / COMPARE_FILE
            return CompareResult.model_validate_json(path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return None

    def agreement(self, study_id: str) -> Agreement | None:
        try:
            path = study_dir(self.runs_dir, study_id) / AGREEMENT_FILE
            return Agreement.model_validate_json(path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return None

    def compare_state(self, study_id: str) -> dict[str, Any]:
        task = self._comparing.get(study_id)
        return {"running": task is not None and not task.done(), "error": self._compare_errors.get(study_id)}

    def is_repeating(self, study_id: str) -> bool:
        task = self._repeating.get(study_id)
        return task is not None and not task.done()

    def _ready_members(self, study: Study) -> list[Member]:
        """Members that finished with a report; empty while any member is still running."""
        members = self.study_members(study)
        if any(self.is_live(m.run_id) or self.run_state(m.run_id) in (RunState.RUNNING, RunState.QUEUED)
               for m in members):
            return []
        return [m for m in members if (m.run_dir / "report.json").is_file()]

    def compare(self, study_id: str) -> None:
        """Start (or redo) the study's comparison in the background."""
        study = self.study(study_id)
        if study is None:
            raise KeyError(study_id)
        task = self._comparing.get(study_id)
        if task is not None and not task.done():
            raise Conflict("the comparison is already being written")
        if self.is_repeating(study_id):
            raise Conflict("the runs are still going; the check is written after the last one")
        members = self._ready_members(study)
        if len(members) < 2:
            raise Conflict("a comparison needs at least two finished runs with a report")
        self._compare_errors.pop(study_id, None)
        self._comparing[study_id] = asyncio.create_task(self._compare(study, members),
                                                        name=f"tellfold-compare-{study_id}")

    def _compare_when_ready(self, study_id: str) -> None:
        """After a study run ends: compare once every member is finished, unless a comparison is under way."""
        study = self.study(study_id)
        task = self._comparing.get(study_id)
        if study is None or study.kind == "repeat" or (task is not None and not task.done()):
            return  # a "Check it" series writes its agreement itself, after its last run
        if len(self._ready_members(study)) >= 2:
            with contextlib.suppress(Conflict, KeyError):
                self.compare(study_id)

    async def _compare(self, study: Study, members: list[Member]) -> None:
        folder = study_dir(self.runs_dir, study.id)
        try:
            with self.reader(members[-1].run_id) as r:
                config = r.config()
            assert config is not None
            # The study folder gets the newest run's config, so the comparison uses the same AI tool and models,
            # and its calls are logged in the study folder (calls.jsonl), not in one of the runs.
            folder.mkdir(parents=True, exist_ok=True)
            write_json(folder / "config.json", config)
            llm = self.llm_factory(folder, 1, False)
            if study.kind == "repeat":
                agreement = await build_agreement(llm, study.id, members, question=config.question,
                                                  model=config.planner_model, language=config.language)
                write_json(folder / AGREEMENT_FILE, agreement)
                return
            states = {m.run_id: (self.run_state(m.run_id) or RunState.DONE).value for m in members}
            result = await build_compare(llm, study.id, members, question=config.question,
                                         model=config.planner_model, language=config.language, states=states)
            write_json(folder / COMPARE_FILE, result)
        except Exception as e:  # noqa: BLE001 - shown in the study view, the server keeps running
            log.warning("comparison for study %s failed", study.id, exc_info=True)
            self._compare_errors[study.id] = str(e) or type(e).__name__
        finally:
            self._comparing.pop(study.id, None)

    async def check(self, source_id: str, runs: int, agent_model: str | None = None) -> Study:
        """Check it: run `source_id` again `runs - 1` times with the same people, text and settings but the next
        RNG seeds, one run after another so usage limits aren't hammered, then write what held (agreement.json).
        `agent_model` swaps the model the agents use in the new runs. An example's repeats use this machine's AI
        tool, like its interviews."""
        if not 2 <= runs <= MAX_RUNS:
            raise BadRequest(f"runs must be between 2 and {MAX_RUNS}")
        if self.is_live(source_id):
            raise Conflict("the run is still in progress")
        with self.reader(source_id) as r:
            config = r.config()
            has_report = r.has_report()
        source = self.run_path(source_id)
        if config is None or not has_report or not (source / "personas.json").is_file():
            raise Conflict("only a finished run with a report can be checked")
        if self.is_example(source_id):
            config = await self._local_tool(config, "to check an example run")
        study = Study(id=new_study_id(), kind="repeat", created_at=now_iso(), planned=runs,
                      runs=[StudyRun(id=source_id, variant="Run 1")])
        save_study(self.runs_dir, study)
        self._bind_loop()
        self._repeating[study.id] = asyncio.create_task(self._repeat(study, source_id, config, agent_model),
                                                        name=f"tellfold-check-{study.id}")
        return study

    async def _repeat(self, study: Study, source_id: str, source: SimConfig, agent_model: str | None) -> None:
        seeds_dir = self.run_path(source_id) / SEEDS_DIR
        try:
            for i in range(1, study.planned):
                files = sorted(p for p in seeds_dir.iterdir() if p.is_file() and not p.name.startswith(".")) \
                    if seeds_dir.is_dir() else []
                if not files:
                    raise RuntimeError(f"{source_id} has no saved seed documents to run again")
                if self._closing:
                    return
                run_dir = self.create_run_dir(source.question)
                seeds = copy_seeds(run_dir, files)
                variant = f"Run {i + 1}"
                config = source.model_copy(update={
                    "seed_files": [str(p) for p in seeds], "seed": source.seed + i, "cast_from": source_id,
                    "study_id": study.id, "variant": variant,
                    **({"agent_model": agent_model} if agent_model else {})})
                study = self._join(study, run_dir.name, variant)
                await self.start(run_dir, config, seeds)
                live = self._live.get(run_dir.name)
                if live is not None and live.task is not None:
                    await asyncio.shield(live.task)
                if self.run_state(run_dir.name) != RunState.DONE:
                    log.info("check %s stopped: %s did not finish", study.id, run_dir.name)
                    if not self._closing:  # on shutdown the app shows "stopped after N of M runs" itself
                        self._stop_series(study, f"{variant} did not finish, so the check stopped after "
                                                 f"{len(study.runs)} of {study.planned} runs")
                    return
        except asyncio.CancelledError:
            raise
        except Exception as e:  # noqa: BLE001 - shown in the study view
            log.warning("check %s failed", study.id, exc_info=True)
            self._stop_series(study, str(e) or type(e).__name__)
            return
        finally:
            self._repeating.pop(study.id, None)
        with contextlib.suppress(Conflict, KeyError):
            self.compare(study.id)

    def _stop_series(self, study: Study, reason: str) -> None:
        """A "Check it" series ends early: record why, and stop waiting for the runs that will not come."""
        self._compare_errors[study.id] = reason
        save_study(self.runs_dir, study.model_copy(update={"planned": len(study.runs)}))

    def _join(self, study: Study, run_id: str, variant: str) -> Study:
        study = study.model_copy(update={"runs": [*study.runs, StudyRun(id=run_id, variant=variant)]})
        save_study(self.runs_dir, study)
        return study

    # ---------- fan-out ----------

    def subscribe(self, run_id: str) -> asyncio.Queue:
        """A new client queue. The first item is the run's current `status` event."""
        self._bind_loop()
        queue: asyncio.Queue = asyncio.Queue(maxsize=QUEUE_SIZE)
        self._subs.setdefault(run_id, set()).add(queue)
        status = self.effective_status(run_id)
        if status is not None:
            text = status.model_dump_json()
            self._put(queue, ("status", text))
            sent = self._sent.setdefault(run_id, _Sent())
            if sent.status_json is None:
                sent.status_json = text
        if run_id not in self._watches:
            watch = _Watch(task=asyncio.create_task(self._watch(run_id), name=f"tellfold-watch-{run_id}"))
            self._watches[run_id] = watch
            if not self.is_live(run_id):
                self._baseline(run_id, watch, status)
        return queue

    def _baseline(self, run_id: str, watch: _Watch, status: RunStatus | None) -> None:
        """Remember where a run written elsewhere stands, so the watcher only reports what changes next."""
        with self.reader(run_id) as r:
            watch.last_post_id = r.max_post_id()
            watch.last_db = r.db_signature()
        watch.last_round = status.round if status else None
        watch.last_state = status.state if status else None

    def unsubscribe(self, run_id: str, queue: asyncio.Queue) -> None:
        subs = self._subs.get(run_id)
        if subs is not None:
            subs.discard(queue)
            if not subs:
                del self._subs[run_id]
                watch = self._watches.pop(run_id, None)
                if watch is not None:
                    watch.task.cancel()

    def subscriber_count(self, run_id: str) -> int:
        return len(self._subs.get(run_id, ()))

    def publish(self, run_id: str, name: str, data: Any) -> None:
        subs = self._subs.get(run_id)
        if not subs:
            return
        payload = json.dumps(to_jsonable_python(data, fallback=str), ensure_ascii=False, separators=(",", ":"))
        for queue in list(subs):
            self._put(queue, (name, payload))

    @staticmethod
    def _put(queue: asyncio.Queue, item: Any) -> None:
        """Never block the run: a client too slow to keep up loses its oldest events, not the newest."""
        while True:
            try:
                queue.put_nowait(item)
                return
            except asyncio.QueueFull:
                with contextlib.suppress(asyncio.QueueEmpty):
                    queue.get_nowait()

    def _send_status(self, run_id: str, status: RunStatus) -> None:
        text = status.model_dump_json()
        sent = self._sent.setdefault(run_id, _Sent())
        if sent.status_json == text:
            return
        sent.status_json = text
        subs = self._subs.get(run_id)
        for queue in list(subs or ()):
            self._put(queue, ("status", text))

    def refresh_status(self, run_id: str) -> None:
        status = self.effective_status(run_id)
        if status is not None:
            self._send_status(run_id, status)

    # ---------- watcher (runs this server is not executing) ----------

    async def _watch(self, run_id: str) -> None:
        try:
            while True:
                await asyncio.sleep(self.poll_interval)
                watch = self._watches.get(run_id)
                if watch is None:
                    return
                try:
                    self._poll(run_id, watch)
                except Exception:  # noqa: BLE001 - a bad read must not kill the watcher
                    log.debug("watch poll for %s failed", run_id, exc_info=True)
        except asyncio.CancelledError:
            pass

    def _poll(self, run_id: str, watch: _Watch) -> None:
        live = self.is_live(run_id)
        with self.reader(run_id) as r:
            status = r.status(live=live)
            if status is not None:
                self._send_status(run_id, status)
            if live:
                # in-process runs push their own events; re-baseline once they finish
                watch.last_post_id = watch.last_round = watch.last_state = None
                return
            # posts_added for runs written by another process
            signature = r.db_signature()
            if watch.last_post_id is None:
                watch.last_post_id = r.max_post_id()
                watch.last_db = signature
            elif signature != watch.last_db:
                watch.last_db = signature
                new = r.new_post_ids(watch.last_post_id)
                if new:
                    watch.last_post_id = new[-1][1]
                    by_round: dict[int, list[int]] = {}
                    for rnd, pid in new:
                        by_round.setdefault(rnd, []).append(pid)
                    for rnd, ids in by_round.items():
                        self.publish(run_id, "posts_added", {"round": rnd, "post_ids": ids})
            if status is None:
                return
            # round_done for rounds completed elsewhere
            if watch.last_round is None:
                watch.last_round = status.round
                watch.last_state = status.state  # just finished in-process: its own events were already sent
            elif status.round > watch.last_round:
                stats = {s.round: s for s in r.round_stats()}
                for rnd in range(watch.last_round + 1, status.round + 1):
                    if rnd in stats:
                        self.publish(run_id, "round_done",
                                     {**stats[rnd].model_dump(), "total_rounds": status.total_rounds})
                watch.last_round = status.round
            # terminal transition
            if watch.last_state in (RunState.RUNNING, RunState.QUEUED) and status.state in _TERMINAL_EVENTS:
                data = {"error": status.error or ""} if status.state == RunState.FAILED else {}
                self.publish(run_id, _TERMINAL_EVENTS[status.state], data)
            watch.last_state = status.state

    # ---------- lifecycle ----------

    async def shutdown(self) -> None:
        """Ask every live run to stop, give them `shutdown_grace` seconds, then cancel what is left."""
        self._closing = True  # a "Check it" series must not start its next run during the grace wait
        tasks = [run.task for run in self._live.values() if run.task is not None and not run.task.done()]
        for run in self._live.values():
            run.stop.set()
        if tasks:
            _, pending = await asyncio.wait(tasks, timeout=self.shutdown_grace)
            for task in pending:
                task.cancel()
            if pending:
                await asyncio.wait(pending, timeout=2)
        for task in [*self._repeating.values(), *self._rebuilding.values(), *self._comparing.values()]:
            task.cancel()
        for watch in list(self._watches.values()):
            watch.task.cancel()
        self._watches.clear()
        for subs in self._subs.values():
            for queue in subs:
                self._put(queue, _CLOSE)

    @staticmethod
    def _mark_stopped(run_dir: Path, detail: str) -> None:
        reader = RunReader(run_dir)
        status = reader.raw_status()
        if status is not None and status.state in (RunState.RUNNING, RunState.QUEUED):
            with contextlib.suppress(OSError):
                write_status(run_dir, status.model_copy(update={
                    "state": RunState.STOPPED, "detail": detail, "updated_at": now_iso()}))

    # ---------- threading guard ----------

    def _bind_loop(self) -> None:
        if self._loop is None:
            self._loop = asyncio.get_running_loop()
            self._loop_thread = threading.get_ident()

    def _call_on_loop(self, fn: Callable[..., None], *args: Any) -> None:
        """on_event normally fires on the loop thread; if a callback ever comes from another thread, hop."""
        if self._loop is not None and threading.get_ident() != self._loop_thread:
            self._loop.call_soon_threadsafe(fn, *args)
        else:
            fn(*args)


def history_pairs(raw: list[Any]) -> list[tuple[str, str]]:
    """Validate chat history from the client: a list of [question, answer] pairs of strings."""
    pairs: list[tuple[str, str]] = []
    for item in raw:
        if not isinstance(item, (list, tuple)) or len(item) != 2 or not all(isinstance(x, str) for x in item):
            raise BadRequest("history must be a list of [question, answer] string pairs")
        pairs.append((item[0], item[1]))
    return pairs
