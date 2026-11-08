"""Read-only access to one run directory, whether the run is finished, live in this server or live in a CLI.

`RunReader` never writes. It opens `world.db` with `mode=ro` (one connection per reader, which the server
creates per request), so it cannot interfere with a writer in this or another process (the DB is WAL). Every
accessor tolerates missing files, missing tables (older runs) and half-written JSON: it returns an empty
value instead of raising.
"""

from __future__ import annotations

import json
import os
import sqlite3
from collections import defaultdict
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, TypeVar

from pydantic import BaseModel, ValidationError

from tellfold.models import (
    Action,
    GodEvent,
    KnowledgeGraph,
    Persona,
    Post,
    Report,
    RoundStats,
    RunStage,
    RunState,
    RunStatus,
    SimConfig,
)
from tellfold.runner import read_status
from tellfold.runtimes import is_agent_purpose

M = TypeVar("M", bound=BaseModel)

_POST_COLUMNS = ("id", "round", "author_id", "content", "parent_id", "repost_of",
                 "likes", "dislikes", "reposts", "replies")
_ACTIVE_STATES = (RunState.RUNNING, RunState.QUEUED)


def iso_from_timestamp(ts: float) -> str:
    return datetime.fromtimestamp(ts, UTC).isoformat(timespec="seconds").replace("+00:00", "Z")


def pid_alive(pid: int | None) -> bool:
    """True when a process with this pid exists. Never sends a real signal."""
    if pid is None or pid <= 0:
        return False
    if os.name == "nt":  # os.kill(pid, 0) would terminate the process on Windows
        return _pid_alive_windows(pid)
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True  # exists, owned by someone else
    except OSError:
        return False
    return True


def _pid_alive_windows(pid: int) -> bool:  # pragma: no cover - exercised on Windows only
    import ctypes

    kernel32 = ctypes.windll.kernel32  # type: ignore[attr-defined]
    handle = kernel32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not handle:
        return False
    try:
        code = ctypes.c_ulong()
        if not kernel32.GetExitCodeProcess(handle, ctypes.byref(code)):
            return False
        return code.value == 259  # STILL_ACTIVE
    finally:
        kernel32.CloseHandle(handle)


class RunReader:
    """Read-only view of `runs/<id>/`. Use as a context manager (or call `close()`) to release the DB."""

    def __init__(self, run_dir: Path, *, run_id: str | None = None, immutable: bool = False):
        """`immutable` is for runs on read-only media (the app's bundled examples): SQLite then skips all locking."""
        self.run_dir = run_dir
        self.id = run_id or run_dir.name
        self.immutable = immutable
        self._conn: sqlite3.Connection | None = None
        self._conn_failed = False
        self._persona_cache: dict[str, Persona] | None = None

    def __enter__(self) -> RunReader:
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    def close(self) -> None:
        if self._conn is not None:
            self._conn.close()
            self._conn = None

    # ---------- files ----------

    @property
    def db_path(self) -> Path:
        return self.run_dir / "world.db"

    def is_run(self) -> bool:
        """A directory counts as a run once it has a config.json."""
        return (self.run_dir / "config.json").is_file()

    def _json_model(self, name: str, model: type[M]) -> M | None:
        try:
            return model.model_validate_json((self.run_dir / name).read_text(encoding="utf-8"))
        except (OSError, ValueError, ValidationError):
            return None

    def config(self) -> SimConfig | None:
        return self._json_model("config.json", SimConfig)

    def graph(self) -> KnowledgeGraph | None:
        return self._json_model("graph.json", KnowledgeGraph)

    def report(self) -> Report | None:
        return self._json_model("report.json", Report)

    def has_report(self) -> bool:
        return (self.run_dir / "report.json").is_file()

    def raw_status(self) -> RunStatus | None:
        return read_status(self.run_dir)

    def personas(self) -> list[Persona]:
        if self._persona_cache is None:
            people: list[Persona] = []
            try:
                raw = json.loads((self.run_dir / "personas.json").read_text(encoding="utf-8"))
                people = [Persona.model_validate(p) for p in raw]
            except (OSError, ValueError, ValidationError, TypeError):
                people = [Persona.model_validate_json(r[0]) for r in self._rows("SELECT data FROM personas ORDER BY id")
                          if _valid_json(r[0])]
            self._persona_cache = {p.id: p for p in people}
        return list(self._persona_cache.values())

    def _handles(self) -> dict[str, str]:
        self.personas()
        return {pid: p.handle for pid, p in (self._persona_cache or {}).items()}

    def calls(self) -> dict[str, Any]:
        """Totals from calls.jsonl: {count, failed, seconds, agent: {...}, planner: {...}}.

        `agent` covers the calls routed to the agent runtime (rounds, sentiment, interviews), `planner` the rest
        (graph, personas, report, ask), split the same way `tellfold.runtimes.build_llm` routes them."""
        totals = {role: {"count": 0, "failed": 0, "seconds": 0.0} for role in ("all", "agent", "planner")}
        try:
            with (self.run_dir / "calls.jsonl").open(encoding="utf-8") as f:
                for line in f:
                    try:
                        rec = json.loads(line)
                    except ValueError:
                        continue  # a line still being written
                    if not isinstance(rec, dict):
                        continue
                    try:
                        seconds = float(rec.get("seconds") or 0)
                    except (TypeError, ValueError):
                        seconds = 0.0
                    role = "agent" if is_agent_purpose(str(rec.get("purpose") or "")) else "planner"
                    for key in ("all", role):
                        totals[key]["count"] += 1
                        totals[key]["failed"] += rec.get("ok") is False
                        totals[key]["seconds"] += seconds
        except OSError:
            pass
        for t in totals.values():
            t["seconds"] = round(t["seconds"], 2)
        return {**totals["all"], "agent": totals["agent"], "planner": totals["planner"]}

    def mtime(self, name: str) -> float:
        try:
            return (self.run_dir / name).stat().st_mtime
        except OSError:
            return 0.0

    def db_signature(self) -> tuple[float, float, int]:
        """Changes whenever the database (or its WAL) is written."""
        wal = self.run_dir / "world.db-wal"
        try:
            wal_size = wal.stat().st_size
        except OSError:
            wal_size = 0
        return self.mtime("world.db"), self.mtime("world.db-wal"), wal_size

    # ---------- database ----------

    def _db(self) -> sqlite3.Connection | None:
        if self._conn is not None or self._conn_failed:
            return self._conn
        if not self.db_path.is_file():
            return None
        try:
            query = "mode=ro&immutable=1" if self.immutable else "mode=ro"
            conn = sqlite3.connect(f"{self.db_path.resolve().as_uri()}?{query}", uri=True, timeout=5)
            conn.execute("SELECT 1 FROM sqlite_master LIMIT 1").fetchall()
        except sqlite3.Error:
            self._conn_failed = True
            return None
        self._conn = conn
        return conn

    def _rows(self, sql: str, args: tuple = ()) -> list[tuple]:
        conn = self._db()
        if conn is None:
            return []
        try:
            return conn.execute(sql, args).fetchall()
        except sqlite3.Error:  # missing table in an older or half-initialised run
            return []

    def round_stats(self) -> list[RoundStats]:
        return [RoundStats.model_validate_json(r[0]) for r in self._rows("SELECT data FROM round_stats ORDER BY round")
                if _valid_json(r[0])]

    def events(self) -> list[GodEvent]:
        return [GodEvent(round=r[0], content=r[1])
                for r in self._rows("SELECT round, content FROM events ORDER BY round, seq")]

    def follows(self) -> list[list[str]]:
        return [[r[0], r[1]] for r in self._rows("SELECT follower, followee FROM follows ORDER BY follower, followee")]

    def sentiments(self) -> dict[int, float]:
        return {int(r[0]): float(r[1]) for r in self._rows("SELECT post_id, score FROM post_sentiment")}

    def max_post_id(self) -> int:
        rows = self._rows("SELECT MAX(id) FROM posts")
        return int(rows[0][0]) if rows and rows[0][0] is not None else 0

    def _posts(self, where: str = "", args: tuple = ()) -> list[Post]:
        sql = f"SELECT {', '.join(_POST_COLUMNS)} FROM posts {where} ORDER BY id"
        return [Post(**dict(zip(_POST_COLUMNS, r, strict=True))) for r in self._rows(sql, args)]

    def _post_out(self, posts: list[Post]) -> list[dict[str, Any]]:
        handles = self._handles()
        scores = self.sentiments() if posts else {}
        return [{**p.model_dump(), "author_handle": handles.get(p.author_id, p.author_id),
                 "sentiment": scores.get(p.id)} for p in posts]

    def posts(self, since_id: int = 0) -> list[dict[str, Any]]:
        """PostOut dicts with id > since_id, ascending."""
        return self._post_out(self._posts("WHERE id > ?", (since_id,)))

    def new_post_ids(self, since_id: int) -> list[tuple[int, int]]:
        """(round, id) of posts newer than since_id, ascending by id."""
        return [(int(r[1]), int(r[0])) for r in self._rows("SELECT id, round FROM posts WHERE id > ? ORDER BY id",
                                                           (since_id,))]

    def actions(self, round: int | None = None) -> list[dict[str, Any]]:
        sql, args = "SELECT round, data FROM actions", ()
        if round is not None:
            sql, args = sql + " WHERE round = ?", (round,)
        out = []
        for r, data in self._rows(sql + " ORDER BY seq", args):
            try:
                a = Action.model_validate_json(data)
            except (ValueError, ValidationError):
                continue
            out.append({"round": r, "agent_id": a.agent_id, "type": a.type.value,
                        "target_post_id": a.target_post_id, "target_agent_id": a.target_agent_id,
                        "content": a.content, "reasoning": a.reasoning})
        return out

    def agent(self, agent_id: str) -> dict[str, Any] | None:
        """AgentDetail dict, or None for an unknown agent."""
        self.personas()
        persona = (self._persona_cache or {}).get(agent_id)
        if persona is None:
            return None
        memory: list[str] = []
        rows = self._rows("SELECT notes FROM memories WHERE agent_id = ?", (agent_id,))
        if rows and _valid_json(rows[0][0]):
            notes = json.loads(rows[0][0])
            memory = [str(n) for n in notes] if isinstance(notes, list) else []
        posts = self._post_out(self._posts("WHERE author_id = ?", (agent_id,)))
        by_round: dict[int, list[float]] = defaultdict(list)
        for p in posts:
            if p["sentiment"] is not None:
                by_round[p["round"]].append(p["sentiment"])
        return {
            "persona": persona,
            "memory": memory,
            "posts": posts,
            "followers": [r[0] for r in self._rows(
                "SELECT follower FROM follows WHERE followee = ? ORDER BY follower", (agent_id,))],
            "following": [r[0] for r in self._rows(
                "SELECT followee FROM follows WHERE follower = ? ORDER BY followee", (agent_id,))],
            "sentiment_by_round": [[r, round(sum(v) / len(v), 3)] for r, v in sorted(by_round.items())],
        }

    # ---------- status ----------

    def status(self, *, live: bool) -> RunStatus | None:
        """The run's effective status.

        `live` says whether this server is executing the run right now. A run whose status.json says
        `running` but whose owner process is gone (or is this server, which no longer runs it) is `stopped`.
        Older runs without status.json are `done` when report.json exists, else `stopped`.
        """
        config = self.config()
        status = self.raw_status()
        if status is None:
            if config is None:
                return None
            return self._derived_status(config, live=live)
        if status.state in _ACTIVE_STATES and not live:
            owner_gone = status.pid is None or status.pid == os.getpid() or not pid_alive(status.pid)
            if owner_gone:
                status = status.model_copy(update={"state": RunState.STOPPED})
        return status

    def _derived_status(self, config: SimConfig, *, live: bool) -> RunStatus:
        rounds = [s.round for s in self.round_stats()]
        done = self.has_report()
        if live:
            state, stage = RunState.RUNNING, RunStage.READING
        elif done:
            state, stage = RunState.DONE, RunStage.FINISHED
        else:
            state = RunState.STOPPED
            stage = (RunStage.ROUNDS if rounds else RunStage.PERSONAS if (self.run_dir / "personas.json").exists()
                     else RunStage.GRAPH if (self.run_dir / "graph.json").exists() else RunStage.READING)
        started = self.mtime("config.json") or datetime.now(UTC).timestamp()
        updated = max(started, self.mtime("world.db"), self.mtime("report.json"))
        return RunStatus(state=state, stage=stage, round=max(rounds, default=0), total_rounds=config.rounds,
                         detail="" if live else "no status file (older run)", started_at=iso_from_timestamp(started),
                         updated_at=iso_from_timestamp(updated))

    # ---------- API shapes ----------

    def summary(self, *, live: bool) -> dict[str, Any] | None:
        config = self.config()
        if config is None:
            return None
        status = self.status(live=live)
        assert status is not None
        return {
            "id": self.id,
            "question": config.question,
            "state": status.state.value,
            "stage": status.stage.value,
            "round": status.round,
            "total_rounds": status.total_rounds or config.rounds,
            "agents": config.num_agents,
            "created_at": status.started_at,
            "has_report": self.has_report(),
        }

    def detail(self, *, live: bool) -> dict[str, Any] | None:
        config = self.config()
        if config is None:
            return None
        return {
            "id": self.id,
            "config": config,
            "status": self.status(live=live),
            "live": live,
            "personas": self.personas(),
            "graph": self.graph(),
            "stats": self.round_stats(),
            "events": self.events(),
            "follows": self.follows(),
            "report": self.report(),
            "calls": self.calls(),
        }


def _valid_json(text: Any) -> bool:
    if not isinstance(text, str):
        return False
    try:
        json.loads(text)
    except ValueError:
        return False
    return True
