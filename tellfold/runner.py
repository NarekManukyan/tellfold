"""The run pipeline shared by the CLI and Tellfold Studio: seeds in, report out, status on disk.

`run_pipeline` reads the seed files, extracts the knowledge graph, creates personas, seeds the world, runs the
rounds and writes the report, exactly once for both front ends. While it works it keeps `status.json` (a
`RunStatus`) up to date in the run directory and reports progress through an `on_event(name, payload)` callback.
Every payload is JSON-safe so a server can forward it as is.
"""

from __future__ import annotations

import json
import logging
import math
import os
import re
import shutil
import tempfile
from collections.abc import Callable
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import anyio
from pydantic import BaseModel, ValidationError

from tellfold.engine import Engine
from tellfold.graph import CHUNKS_PER_CALL, MAX_EXTRACT_CHUNKS, extract_graph, select_chunks
from tellfold.ingest import chunk_text, load_documents
from tellfold.llm import LLM
from tellfold.models import FailedBatch, KnowledgeGraph, Persona, Report, RunStage, RunState, RunStatus, SimConfig
from tellfold.personas import generate_personas
from tellfold.report import build_report, render_html, render_markdown
from tellfold.world import World

OnEvent = Callable[[str, dict], None]

STATUS_FILE = "status.json"
_FAILED_BATCH_ERROR_MAX = 500  # characters of a failed batch's error kept in status.json
log = logging.getLogger(__name__)


class RunError(Exception):
    """A run cannot continue for a reason the user can fix (e.g. empty seed files)."""


# ---------- run directory helpers ----------


def make_run_id(question: str, now: datetime | None = None) -> str:
    slug = re.sub(r"[^a-z0-9]+", "-", question.lower()).strip("-")[:40].rstrip("-") or "run"
    return f"{(now or datetime.now()).strftime('%Y%m%d-%H%M%S')}-{slug}"


def create_run_dir(out: Path, question: str) -> Path:
    base = out / make_run_id(question)
    run_dir, n = base, 2
    while True:
        try:
            run_dir.mkdir(parents=True)
            return run_dir
        except FileExistsError:
            run_dir, n = base.with_name(f"{base.name}-{n}"), n + 1


SEEDS_DIR = "seeds"


def copy_seeds(run_dir: Path, files: list[Path]) -> list[Path]:
    """Copy the seed documents into the run, so the run can be read and run again without the originals."""
    dest = run_dir / SEEDS_DIR
    dest.mkdir(exist_ok=True)
    copies: list[Path] = []
    for f in files:
        target, n = dest / f.name, 2
        while target.exists():
            target, n = dest / f"{f.stem}-{n}{f.suffix}", n + 1
        shutil.copyfile(f, target)
        copies.append(target)
    return copies


def write_json(path: Path, data: BaseModel | list[BaseModel]) -> None:
    if isinstance(data, list):
        text = "[\n" + ",\n".join(item.model_dump_json(indent=2) for item in data) + "\n]\n"
    else:
        text = data.model_dump_json(indent=2) + "\n"
    path.write_text(text, encoding="utf-8")


def write_report(run_dir: Path, report: Report, world: World, config: SimConfig) -> Path:
    write_json(run_dir / "report.json", report)
    (run_dir / "report.md").write_text(render_markdown(report), encoding="utf-8")
    html_path = run_dir / "report.html"
    html_path.write_text(render_html(report, world, config), encoding="utf-8")
    return html_path


# Share of the crowd expected to act in a round. Personas carry an activity of about 0.3 to 0.6 and news rounds
# boost it, so batching every agent each round overstated a real 20-agent run by more than half.
ESTIMATED_ACTIVE_SHARE = 0.4


def estimate_call_split(config: SimConfig, chunks: int) -> tuple[int, int]:
    """(agent, planner) model calls for a run when nothing needs a retry. Mirrored in the Mac app
    (`NewRunRequest.estimatedCallSplit`); the reference table in tests/test_server.py pins both.
    A run that reuses a cast (`cast_from`) makes no graph or persona calls."""
    reused = config.cast_from is not None
    graph = 0 if reused else math.ceil(min(chunks, MAX_EXTRACT_CHUNKS) / CHUNKS_PER_CALL)
    personas = 0 if reused else math.ceil(config.num_agents / 10)
    opening = math.ceil(round(0.3 * config.num_agents) / config.batch_size) + 1  # opening posts + sentiment
    active = max(1, math.ceil(config.num_agents * ESTIMATED_ACTIVE_SHARE))
    per_round = math.ceil(active / config.batch_size) + 1  # decisions + sentiment
    return opening + config.rounds * per_round, graph + personas + 1


def estimate_calls(config: SimConfig, chunks: int) -> int:
    """Model calls for a run when nothing needs a retry, shown so users can judge rate-limit cost."""
    return sum(estimate_call_split(config, chunks))


# ---------- reusing a cast ----------


class Cast(BaseModel):
    """The topic map and the people of a finished run, reused by a run with `cast_from` set."""

    graph: KnowledgeGraph
    personas: list[Persona]


def load_cast(source_dir: Path) -> Cast:
    """The graph and personas of `source_dir`; RunError when the run never got that far."""
    try:
        graph = KnowledgeGraph.model_validate_json((source_dir / "graph.json").read_text(encoding="utf-8"))
        raw = (source_dir / "personas.json").read_text(encoding="utf-8")
        personas = [Persona.model_validate(p) for p in json.loads(raw)]
    except (OSError, ValueError, ValidationError) as e:
        raise RunError(f"cannot reuse the people of {source_dir.name}: it has no topic map or people yet") from e
    if not personas:
        raise RunError(f"cannot reuse the people of {source_dir.name}: it has no people")
    return Cast(graph=graph, personas=personas)


# ---------- status.json ----------


def now_iso() -> str:
    return datetime.now(UTC).isoformat(timespec="seconds").replace("+00:00", "Z")


def write_status(run_dir: Path, status: RunStatus) -> None:
    """Write status.json atomically (temp file + os.replace) so readers never see a partial file."""
    fd, tmp = tempfile.mkstemp(prefix=".status-", suffix=".tmp", dir=run_dir)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write(status.model_dump_json(indent=2) + "\n")
        os.replace(tmp, run_dir / STATUS_FILE)
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise


def read_status(run_dir: Path) -> RunStatus | None:
    """The run's status, or None when status.json is missing or unreadable (older runs)."""
    try:
        return RunStatus.model_validate_json((run_dir / STATUS_FILE).read_text(encoding="utf-8"))
    except (OSError, ValueError, ValidationError):
        return None


# ---------- pipeline ----------

_ENGINE_STEPS = {
    "follow_graph": "building the follow graph",
    "follow_graph_done": "follow graph ready",
    "opening_posts": "writing opening posts",
    "opening_posts_done": "opening posts written",
}


class _Tracker:
    """Owns the RunStatus of one run: every change is written to disk and announced."""

    def __init__(self, run_dir: Path, config: SimConfig, on_event: OnEvent | None):
        self.run_dir = run_dir
        self._on_event = on_event
        self.status = RunStatus(
            state=RunState.RUNNING, stage=RunStage.READING, total_rounds=config.rounds,
            pid=os.getpid(), started_at=now_iso(), updated_at=now_iso(),
        )

    def emit(self, name: str, payload: dict[str, Any]) -> None:
        if self._on_event is None:
            return
        try:
            self._on_event(name, payload)
        except Exception:  # noqa: BLE001 - a broken listener must never break the run
            log.exception("on_event listener failed for %s", name)

    def update(self, **changes: Any) -> None:
        self.status = self.status.model_copy(update={**changes, "updated_at": now_iso()})
        write_status(self.run_dir, self.status)
        self.emit("status", self.status.model_dump(mode="json"))

    def stage(self, stage: RunStage, detail: str = "", **extra: Any) -> None:
        self.update(stage=stage, detail=detail)
        self.emit("stage", {"stage": stage.value, "detail": detail, **extra})

    def detail(self, detail: str, **extra: Any) -> None:
        """Same stage, new activity text (re-announced as a `stage` event so the UI can show it)."""
        self.stage(self.status.stage, detail, **extra)

    def engine_event(self, name: str, payload: dict) -> None:
        if name == "stage":  # the engine's own seeding steps become details of the SEEDING stage
            step = str(payload.get("stage", ""))
            self.detail(_ENGINE_STEPS.get(step, step.replace("_", " ")), step=step)
            return
        if name == "round_start":
            r = payload.get("round")
            self.update(detail=f"round {r} of {self.status.total_rounds}: {payload.get('batches', 0)} batches deciding")
        elif name == "batch_failed":
            self._record_failed_batch(payload)
        self.emit(name, payload)

    def _record_failed_batch(self, payload: dict) -> None:
        """Keep failed batches in status.json so they survive a reload (the SSE event alone is transient)."""
        try:
            batch = FailedBatch(
                round=int(payload.get("round", self.status.round)),
                batch=payload.get("batch", "?"),
                agents=[str(a) for a in payload.get("agents") or []],
                error=str(payload.get("error") or "")[:_FAILED_BATCH_ERROR_MAX],
            )
        except (TypeError, ValueError):
            log.warning("could not record failed batch %r", payload)
            return
        _safely(lambda: self.update(failed_batches=[*self.status.failed_batches, batch]))


async def run_pipeline(
    llm: LLM,
    run_dir: Path,
    config: SimConfig,
    seeds: list[Path],
    *,
    on_event: OnEvent | None = None,
    stop: anyio.Event | None = None,
    cast_dir: Path | None = None,
) -> Report | None:
    """Run a whole simulation in `run_dir`.

    With `config.cast_from` set, the graph and persona stages are skipped: their files are copied from `cast_dir`
    (default: the sibling run folder named `cast_from`), and both stages are still announced as done.

    Returns the report, or None when the run was stopped before a single round completed. `stop` is checked
    between stages and between rounds: the current round always finishes, then the run is marked `stopped`
    and the report is still written if at least one round completed. On an error the run is marked `failed`,
    `run_failed` is emitted and the exception propagates; on cancellation (Ctrl-C) it is marked `stopped`.
    """
    run_dir.mkdir(parents=True, exist_ok=True)
    if not (run_dir / "config.json").exists():
        write_json(run_dir / "config.json", config)
    track = _Tracker(run_dir, config, on_event)
    world: World | None = None

    def stopping() -> bool:
        return stop is not None and stop.is_set()

    def finish_stopped() -> None:
        track.update(state=RunState.STOPPED, detail=f"stopped after round {track.status.round}")
        track.emit("run_stopped", {})

    try:
        track.stage(RunStage.READING, f"reading {len(seeds)} seed file{'s' if len(seeds) != 1 else ''}")
        try:
            chunks = chunk_text(load_documents(seeds))
        except (ValueError, OSError, RuntimeError) as e:
            raise RunError(f"could not read seed files: {e}") from e
        if not chunks:
            raise RunError("no readable text found in the seed files")
        if stopping():
            finish_stopped()
            return None

        if config.cast_from:
            source = cast_dir or run_dir.parent / config.cast_from
            cast = load_cast(source)
            graph, personas = cast.graph, cast.personas
            if config.num_agents != len(personas):  # the crowd is the source's, whatever size was asked for
                config = config.model_copy(update={"num_agents": len(personas)})
                write_json(run_dir / "config.json", config)
            track.stage(RunStage.GRAPH, f"reusing the topic map of {config.cast_from}", chunks=len(chunks),
                        reused=True, estimated_calls=estimate_calls(config, len(chunks)))
            write_json(run_dir / "graph.json", graph)
            track.stage(RunStage.PERSONAS, f"reusing the {len(personas)} people of {config.cast_from}",
                        entities=len(graph.entities), relations=len(graph.relations), reused=True)
            write_json(run_dir / "personas.json", personas)
        else:
            selected = select_chunks(chunks, config.question)
            detail = (f"extracting from {len(chunks)} chunks" if len(selected) == len(chunks) else
                      f"extracting from the {len(selected)} of {len(chunks)} chunks most relevant to the question")
            track.stage(RunStage.GRAPH, detail, chunks=len(chunks),
                        estimated_calls=estimate_calls(config, len(chunks)))
            graph = await extract_graph(llm, selected, question=config.question, model=config.planner_model,
                                        on_progress=track.detail)
            write_json(run_dir / "graph.json", graph)
            if stopping():
                finish_stopped()
                return None

            track.stage(RunStage.PERSONAS, f"creating {config.num_agents} personas",
                        entities=len(graph.entities), relations=len(graph.relations))
            personas = await generate_personas(llm, graph, question=config.question, n=config.num_agents,
                                               model=config.planner_model, language=config.language)
            write_json(run_dir / "personas.json", personas)
        if stopping():
            finish_stopped()
            return None

        world = World(run_dir / "world.db", seed=config.seed)
        world.add_personas(personas)
        engine = Engine(llm, world, config, on_event=track.engine_event)

        track.stage(RunStage.SEEDING, "seeding follows and opening posts", personas=len(personas))
        await engine.seed_world(graph)
        track.update(round=0)

        track.stage(RunStage.ROUNDS, f"simulating {config.rounds} rounds")
        for r in range(1, config.rounds + 1):
            if stopping():
                break
            await engine.run_round(r)
            track.update(round=r, detail=f"round {r} of {config.rounds} done")

        stopped = stopping()
        if stopped and track.status.round < 1:
            finish_stopped()
            return None

        note = f" (stopped after round {track.status.round})" if stopped else ""
        track.stage(RunStage.REPORT, f"writing the report{note}")
        report = await build_report(llm, world, graph, config, model=config.planner_model)
        write_report(run_dir, report, world, config)

        if stopped:
            track.update(stage=RunStage.FINISHED)
            finish_stopped()
        else:
            track.update(state=RunState.DONE, stage=RunStage.FINISHED, detail="report ready")
            track.emit("run_done", {})
        return report
    except (anyio.get_cancelled_exc_class(), KeyboardInterrupt):
        _safely(finish_stopped)
        raise
    except Exception as e:
        message = str(e) or type(e).__name__
        _safely(lambda: track.update(state=RunState.FAILED, error=message, detail="failed"))
        track.emit("run_failed", {"error": message})
        raise
    finally:
        if world is not None:
            world.close()


def mark_stopped(run_dir: Path) -> None:
    """Mark a run that is still `running` on disk as `stopped` (e.g. after Ctrl-C outside the pipeline)."""
    status = read_status(run_dir)
    if status is not None and status.state in (RunState.RUNNING, RunState.QUEUED):
        write_status(run_dir, status.model_copy(update={
            "state": RunState.STOPPED, "detail": f"stopped after round {status.round}", "updated_at": now_iso()}))


def _safely(fn: Callable[[], None]) -> None:
    try:
        fn()
    except Exception:  # noqa: BLE001 - status bookkeeping must not mask the original error
        log.exception("could not update run status")
