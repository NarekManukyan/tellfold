"""Shoal Studio server: HTTP API, uploads, SSE, live-run control and the watcher for CLI-started runs.

Every model call goes through FakeLLM (the same schema-driven canned responder `SHOAL_FAKE_LLM=1` uses).
A `Gate` lets a test hold a live run inside round 1 so injection / stop rules can be checked deterministically.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
import time
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import pytest
from fastapi.testclient import TestClient

from shoal.cli import fake_responder
from shoal.llm import FakeLLM, LLMError
from shoal.models import Action, ActionType, Persona, RoundStats, RunStage, RunState, RunStatus, SimConfig
from shoal.runner import now_iso, write_json, write_status
from shoal.server import create_app
from shoal.server.app import safe_filename
from shoal.world import World

SEED_TEXT = (
    "# Harbour Town votes on a new ferry fare\n\n"
    "The council of Harbour Town proposes raising the ferry fare from 2 to 3 euros to fund a new electric ferry. "
    "The commuters' association opposes the rise; local shops support the electric ferry. The mayor, Ana Ruiz, "
    "will decide after a public consultation.\n\n"
    "Students say the fare is already too high. The ferry operator says the old boat is unreliable.\n"
)
TIMEOUT = 30.0


class Gate:
    """Holds `engine.round` calls while closed. Thread-safe: the app runs on TestClient's portal thread."""

    def __init__(self) -> None:
        self.open_event = threading.Event()
        self.open_event.set()
        self.waiting = threading.Event()

    def close(self) -> None:
        self.open_event.clear()
        self.waiting.clear()

    def open(self) -> None:
        self.open_event.set()

    async def wait(self) -> None:
        import asyncio

        while not self.open_event.is_set():
            self.waiting.set()
            await asyncio.sleep(0.02)


class Factory:
    def __init__(self) -> None:
        self.gate = Gate()
        self.made: list[tuple[Path | None, int, bool]] = []
        self.llms: list[FakeLLM] = []

    def __call__(self, run_dir: Path | None, concurrency: int, think: bool) -> FakeLLM:
        self.made.append((run_dir, concurrency, think))

        async def responder(purpose: str, prompt: str, schema: Any) -> Any:
            if purpose == "engine.round":
                await self.gate.wait()
            return fake_responder(purpose, prompt, schema)

        llm = FakeLLM(responder)
        self.llms.append(llm)
        return llm

    def purposes(self) -> list[str]:
        return [c[0] for llm in self.llms for c in llm.calls]


@pytest.fixture
def runs_dir(tmp_path: Path) -> Path:
    path = tmp_path / "runs"
    path.mkdir()
    return path


@pytest.fixture
def factory() -> Iterator[Factory]:
    f = Factory()
    yield f
    f.gate.open()


@pytest.fixture
def client(runs_dir: Path, factory: Factory) -> Iterator[TestClient]:
    app = create_app(runs_dir, factory, fake_llm=True, poll_interval=0.05, shutdown_grace=0.5)
    with TestClient(app) as c:
        yield c
    factory.gate.open()


def wait_for(predicate: Callable[[], Any], timeout: float = TIMEOUT, what: str = "condition") -> Any:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.03)
    raise AssertionError(f"timed out waiting for {what}")


def start_run(client: TestClient, *, rounds: int = 2, agents: int = 4, events: list | None = None,
              files: list | None = None, **extra: str) -> str:
    data = {"question": "Will Harbour Town accept the ferry fare rise?", "agents": str(agents),
            "rounds": str(rounds), "batch": "2", "agent_model": "haiku", "planner_model": "sonnet",
            "concurrency": "2", "language": "English", "think": "false",
            "events": json.dumps(events or []), **extra}
    res = client.post("/api/runs", data=data,
                      files=files or [("files[]", ("harbour.md", SEED_TEXT.encode(), "text/markdown"))])
    assert res.status_code == 200, res.text
    return res.json()["id"]


def detail(client: TestClient, run_id: str) -> dict:
    res = client.get(f"/api/runs/{run_id}")
    assert res.status_code == 200, res.text
    return res.json()


def wait_state(client: TestClient, run_id: str, *states: str) -> dict:
    return wait_for(lambda: (d := detail(client, run_id))["status"]["state"] in states and not d["live"] and d,
                    what=f"{run_id} to reach {states}")


def wait_round_blocked(client: TestClient, factory: Factory, run_id: str) -> None:
    """The run is live, in the rounds stage, and round 1's decisions are held at the gate."""
    wait_for(factory.gate.waiting.is_set, what="round 1 to reach the gate")
    assert detail(client, run_id)["live"] is True


class SSEReader:
    """Reads a text/event-stream on a background thread so a test can wait with a timeout."""

    def __init__(self, client: TestClient, run_id: str):
        self.events: list[tuple[str, Any]] = []
        self._cm = client.stream("GET", f"/api/runs/{run_id}/stream")
        self.response = self._cm.__enter__()
        self._thread = threading.Thread(target=self._read, daemon=True)
        self._thread.start()

    def _read(self) -> None:
        name = None
        try:
            for line in self.response.iter_lines():
                if line.startswith("event: "):
                    name = line[7:]
                elif line.startswith("data: ") and name:
                    self.events.append((name, json.loads(line[6:])))
                    name = None
        except Exception:  # noqa: BLE001 - stream closed by the test
            pass

    def names(self) -> list[str]:
        return [n for n, _ in self.events]

    def wait(self, name: str, timeout: float = TIMEOUT) -> Any:
        found = wait_for(lambda: next(([d] for n, d in list(self.events) if n == name), None), timeout,
                         f"SSE event {name}; got {self.names()}")
        return found[0]

    def close(self) -> None:
        try:
            self._cm.__exit__(None, None, None)
        except Exception:  # noqa: BLE001
            pass


# ---------------------------------------------------------------- basics


def test_health_spa_and_unknown_api(client: TestClient) -> None:
    assert client.get("/api/health").json() == {"ok": True, "fake_llm": True, "version": "0.1.0"}
    for path in ("/", "/run/abc", "/new"):
        res = client.get(path)
        assert res.status_code in (200, 503)
        assert res.headers["content-type"].startswith("text/html")
    assert client.get("/api/nope").status_code == 404
    assert client.get("/api/nope").headers["content-type"].startswith("application/json")
    assert client.get("/api/runs").json() == []
    if (Path(__file__).parent.parent / "shoal/web/static/js/api.js").exists():
        res = client.get("/static/js/api.js")
        assert res.status_code == 200 and "export" in res.text
    assert client.get("/static/js/does-not-exist.js").status_code == 404


def test_path_traversal_and_unknown_runs_rejected(client: TestClient, runs_dir: Path) -> None:
    (runs_dir.parent / "secret").mkdir()
    (runs_dir.parent / "secret" / "config.json").write_text("{}")
    for raw in ("%2E%2E", "..", "..%2Fsecret", "%2E%2E%2Fsecret", "a..b", ".hidden", "a%5C..%5Cb"):
        for suffix in ("", "/posts", "/report.html", "/stream", "/agents/a01"):
            res = client.get(f"/api/runs/{raw}{suffix}")
            assert res.status_code in (400, 404), (raw, suffix, res.status_code)
    assert client.get("/api/runs/..foo").status_code == 400
    assert client.get("/api/runs/20990101-000000-nothing").status_code == 404
    assert client.post("/api/runs/20990101-000000-nothing/stop").status_code == 404


def test_local_only_guard(client: TestClient) -> None:
    assert client.get("/api/health", headers={"host": "evil.example"}).status_code == 403
    res = client.post("/api/runs", headers={"origin": "http://evil.example"}, data={"question": "x"})
    assert res.status_code == 403
    ok = client.get("/api/health", headers={"host": "127.0.0.1:8765"})
    assert ok.status_code == 200


def test_safe_filename() -> None:
    taken: set[str] = set()
    assert safe_filename("../../etc/passwd.md", taken) == "passwd.md"
    assert safe_filename("..\\..\\win.TXT", taken) == "win.txt"
    assert safe_filename("passwd.md", taken) == "passwd-2.md"
    assert safe_filename(".hidden.md", taken) == "hidden.md"
    assert safe_filename("rés umé;rm -rf.pdf", taken) == "res ume_rm -rf.pdf"


def test_upload_validation(client: TestClient, runs_dir: Path) -> None:
    def post(data: dict, files: list | None) -> int:
        base = {"question": "Will it work?", "agents": "4", "rounds": "2"}
        return client.post("/api/runs", data={**base, **data}, files=files).status_code

    good = [("files[]", ("a.md", b"hello world", "text/markdown"))]
    assert post({}, None) == 400                                                     # no files
    assert post({}, [("files[]", ("a.exe", b"MZ", "application/octet-stream"))]) == 400
    assert post({}, [("files[]", ("a.md", b"", "text/markdown"))]) == 400            # empty file
    assert post({"question": "  "}, good) == 400
    assert post({"agents": "1"}, good) == 400
    assert post({"agents": "201"}, good) == 400
    assert post({"rounds": "0"}, good) == 400
    assert post({"rounds": "abc"}, good) == 400
    assert post({"batch": "21"}, good) == 400
    assert post({"agent_model": "haiku; rm -rf /"}, good) == 400
    assert post({"events": "not json"}, good) == 400
    assert post({"events": json.dumps([{"round": 9, "content": "late"}])}, good) == 400
    big = b"x" * (20 * 1024 * 1024 + 1)
    assert post({}, [("files[]", ("big.txt", big, "text/plain"))]) == 413
    assert client.get("/api/runs").json() == []
    assert [p for p in runs_dir.iterdir()] == []  # failed uploads leave no run directory behind


# ---------------------------------------------------------------- a full run


def test_full_run_and_every_endpoint(client: TestClient, factory: Factory, runs_dir: Path) -> None:
    files = [("files[]", ("../../notes.md", SEED_TEXT.encode(), "text/markdown")),
             ("files", ("extra.txt", b"Commuters plan a protest at the pier.", "text/plain"))]
    run_id = start_run(client, rounds=2, events=[{"round": 1, "content": "The mayor delays the vote"}], files=files)
    assert (runs_dir / run_id / "seeds" / "notes.md").is_file()
    assert (runs_dir / run_id / "seeds" / "extra.txt").is_file()
    assert not (runs_dir.parent / "notes.md").exists()
    assert factory.made[0] == (runs_dir / run_id, 2, False)

    d = wait_state(client, run_id, "done")
    assert d["id"] == run_id and d["live"] is False
    assert d["config"]["num_agents"] == 4 and d["config"]["rounds"] == 2 and d["config"]["batch_size"] == 2
    assert d["status"]["stage"] == "finished" and d["status"]["round"] == 2
    assert len(d["personas"]) == 4
    assert d["graph"] is not None and "entities" in d["graph"]
    assert [s["round"] for s in d["stats"]] == [0, 1, 2]
    assert {"round": 1, "content": "The mayor delays the vote"} in d["events"]
    assert all(len(edge) == 2 for edge in d["follows"])
    assert d["report"] is not None and d["report"]["confidence"] in ("low", "medium", "high")
    assert set(d["calls"]) == {"count", "failed", "seconds"}

    runs = client.get("/api/runs").json()
    assert [r["id"] for r in runs] == [run_id]
    assert runs[0] | {"created_at": None} == {
        "id": run_id, "question": "Will Harbour Town accept the ferry fare rise?", "state": "done",
        "stage": "finished", "round": 2, "total_rounds": 2, "agents": 4, "created_at": None, "has_report": True}

    posts = client.get(f"/api/runs/{run_id}/posts").json()
    assert posts and [p["id"] for p in posts] == sorted(p["id"] for p in posts)
    handles = {p["id"]: p["handle"] for p in d["personas"]}
    assert all(p["author_handle"] == handles[p["author_id"]] for p in posts)
    assert all("sentiment" in p for p in posts)
    later = client.get(f"/api/runs/{run_id}/posts", params={"since_id": posts[0]["id"]}).json()
    assert [p["id"] for p in later] == [p["id"] for p in posts[1:]]
    assert client.get(f"/api/runs/{run_id}/posts", params={"since_id": posts[-1]["id"]}).json() == []
    assert client.get(f"/api/runs/{run_id}/posts", params={"since_id": -1}).status_code == 422

    actions = client.get(f"/api/runs/{run_id}/actions").json()
    assert actions and set(actions[0]) == {"round", "agent_id", "type", "target_post_id", "target_agent_id",
                                           "content", "reasoning"}
    round1 = client.get(f"/api/runs/{run_id}/actions", params={"round": 1}).json()
    assert round1 and all(a["round"] == 1 for a in round1)

    author = posts[0]["author_id"]
    agent = client.get(f"/api/runs/{run_id}/agents/{author}").json()
    assert agent["persona"]["id"] == author
    assert set(agent) == {"persona", "memory", "posts", "followers", "following", "sentiment_by_round"}
    assert all(p["author_id"] == author for p in agent["posts"])
    assert all(len(pair) == 2 for pair in agent["sentiment_by_round"])
    by_handle = client.get(f"/api/runs/{run_id}/agents/@{handles[author]}").json()
    assert by_handle["persona"]["id"] == author
    assert client.get(f"/api/runs/{run_id}/agents/a99").status_code == 404

    page = client.get(f"/api/runs/{run_id}/report.html")
    assert page.status_code == 200 and page.headers["content-type"].startswith("text/html")

    # chat and ask round-trips
    res = client.post(f"/api/runs/{run_id}/chat",
                      json={"agent_id": author, "message": "Why did you post that?", "history": [["hi", "hello"]]})
    assert res.status_code == 200, res.text
    assert "offline test reply" in res.json()["reply"]
    assert "chat.interview" in factory.purposes()
    assert client.post(f"/api/runs/{run_id}/chat", json={"agent_id": "a99", "message": "hi"}).status_code == 404
    assert client.post(f"/api/runs/{run_id}/chat",
                       json={"agent_id": author, "message": "hi", "history": [["only one"]]}).status_code == 400
    res = client.post(f"/api/runs/{run_id}/ask", json={"question": "What drove the outcome?"})
    assert res.status_code == 200 and res.json()["reply"]
    assert "chat.ask" in factory.purposes()

    # a finished run cannot be stopped or receive events
    assert client.post(f"/api/runs/{run_id}/events", json={"round": 2, "content": "x"}).status_code == 409
    assert client.post(f"/api/runs/{run_id}/stop").status_code == 409

    # rebuild the report in the background
    before = factory.purposes().count("report.write")
    (runs_dir / run_id / "report.html").unlink()
    assert client.post(f"/api/runs/{run_id}/report").json() == {"ok": True}
    wait_for(lambda: (runs_dir / run_id / "report.html").exists(), what="rebuilt report.html")
    assert factory.purposes().count("report.write") == before + 1


def test_report_rebuild_state_reports_failure_and_success(runs_dir: Path) -> None:
    """A finished run has no SSE stream, so the rebuild outcome must be visible on GET /api/runs/{id}."""
    fail = {"on": True}

    class FailingReports(Factory):
        def __call__(self, run_dir: Path | None, concurrency: int, think: bool) -> FakeLLM:
            async def responder(purpose: str, prompt: str, schema: Any) -> Any:
                if fail["on"] and purpose.startswith("report."):
                    raise LLMError("the model is down")
                return fake_responder(purpose, prompt, schema)

            llm = FakeLLM(responder)
            self.llms.append(llm)
            return llm

    run_id = make_external_run(runs_dir)
    app = create_app(runs_dir, FailingReports(), fake_llm=True, poll_interval=0.05, shutdown_grace=0.5)
    with TestClient(app) as client:
        assert detail(client, run_id)["report_rebuild"] == {"running": False, "error": None}

        assert client.post(f"/api/runs/{run_id}/report").status_code == 200
        state = wait_for(lambda: (rb := detail(client, run_id)["report_rebuild"])["running"] is False and rb,
                         what="the failing rebuild to finish")
        assert state["error"] and "the model is down" in state["error"]

        fail["on"] = False
        assert client.post(f"/api/runs/{run_id}/report").status_code == 200
        state = wait_for(lambda: (rb := detail(client, run_id)["report_rebuild"])["running"] is False and rb,
                         what="the second rebuild to finish")
        assert state == {"running": False, "error": None}  # a new rebuild clears the old failure
        assert (runs_dir / run_id / "report.html").exists()


def test_ask_needs_a_report(client: TestClient, runs_dir: Path) -> None:
    run_id = make_external_run(runs_dir, with_report=False)
    assert client.post(f"/api/runs/{run_id}/ask", json={"question": "Why?"}).status_code == 409


# ---------------------------------------------------------------- live control


def test_event_injection_rules_and_pickup(client: TestClient, factory: Factory) -> None:
    factory.gate.close()
    run_id = start_run(client, rounds=3)
    wait_round_blocked(client, factory, run_id)

    def inject(r: int, text: str = "Breaking: the ferry breaks down") -> int:
        return client.post(f"/api/runs/{run_id}/events", json={"round": r, "content": text}).status_code

    assert inject(1) == 409          # round 1 already started
    assert inject(0) == 400
    assert inject(4) == 400          # beyond the last round
    assert client.post(f"/api/runs/{run_id}/events", json={"round": 2, "content": ""}).status_code == 422
    assert inject(2) == 200

    sse = SSEReader(client, run_id)
    try:
        assert sse.wait("status")["state"] == "running"
        assert sse.names()[0] == "status"
        factory.gate.open()
        picked = sse.wait("event_injected")
        assert picked == {"round": 2, "content": "Breaking: the ferry breaks down"}
        sse.wait("run_done")
        assert "posts_added" in sse.names() and "round_done" in sse.names()
    finally:
        sse.close()
    d = wait_state(client, run_id, "done")
    assert {"round": 2, "content": "Breaking: the ferry breaks down"} in d["events"]


def test_stop_live_run(client: TestClient, factory: Factory, runs_dir: Path) -> None:
    factory.gate.close()
    run_id = start_run(client, rounds=4)
    wait_round_blocked(client, factory, run_id)
    assert client.post(f"/api/runs/{run_id}/stop").json() == {"ok": True}
    factory.gate.open()
    d = wait_state(client, run_id, "stopped")
    assert d["status"]["round"] == 1           # the current round finished, then it stopped
    assert d["report"] is not None             # a report is still written after >= 1 round
    assert client.post(f"/api/runs/{run_id}/stop").status_code == 409


def test_runs_survive_client_disconnect(client: TestClient, factory: Factory) -> None:
    factory.gate.close()
    run_id = start_run(client, rounds=2)
    sse = SSEReader(client, run_id)
    sse.wait("status")
    sse.close()
    manager = client.app.state.manager
    wait_for(lambda: manager.subscriber_count(run_id) == 0, what="the SSE subscriber to be released")
    factory.gate.open()
    wait_state(client, run_id, "done")


def test_shutdown_stops_live_runs(runs_dir: Path, factory: Factory) -> None:
    app = create_app(runs_dir, factory, fake_llm=True, poll_interval=0.05, shutdown_grace=0.3)
    factory.gate.close()
    with TestClient(app) as c:
        run_id = start_run(c, rounds=3)
        wait_round_blocked(c, factory, run_id)
    status = RunStatus.model_validate_json((runs_dir / run_id / "status.json").read_text())
    assert status.state == RunState.STOPPED


# ---------------------------------------------------------------- runs started elsewhere


def make_external_run(runs_dir: Path, *, run_id: str = "20260101-120000-cli-run", with_report: bool = True,
                      status: RunStatus | None = None, rounds: int = 2) -> str:
    run_dir = runs_dir / run_id
    run_dir.mkdir()
    config = SimConfig(question="Will the CLI run show up?", seed_files=["x.md"], num_agents=2, rounds=rounds)
    write_json(run_dir / "config.json", config)
    people = [Persona(id=f"a0{i}", handle=f"user{i}", name=f"User {i}", bio="b", background="bg", stance="s")
              for i in (1, 2)]
    write_json(run_dir / "personas.json", people)
    world = World(run_dir / "world.db", seed=7)
    world.add_personas(people)
    world.apply(0, Action(agent_id="a01", type=ActionType.POST, content="Opening post"))
    world.save_round_stats(RoundStats(round=0, new_posts=1))
    world.close()
    if with_report:
        (run_dir / "report.json").write_text(json.dumps({
            "title": "T", "summary": "S", "prediction": "P", "confidence": "low"}))
    if status is not None:
        write_status(run_dir, status)
    return run_id


def dead_pid() -> int:
    proc = subprocess.Popen([sys.executable, "-c", "pass"])
    proc.wait()
    return proc.pid


def running_status(pid: int | None, round: int = 0) -> RunStatus:
    return RunStatus(state=RunState.RUNNING, stage=RunStage.ROUNDS, round=round, total_rounds=2, pid=pid,
                     started_at=now_iso(), updated_at=now_iso())


def test_older_and_stale_runs_get_a_derived_state(client: TestClient, runs_dir: Path) -> None:
    make_external_run(runs_dir, run_id="20260101-000001-old-done", with_report=True)
    make_external_run(runs_dir, run_id="20260101-000002-old-partial", with_report=False)
    make_external_run(runs_dir, run_id="20260101-000003-dead-cli", with_report=False,
                      status=running_status(dead_pid()))
    make_external_run(runs_dir, run_id="20260101-000004-alive-cli", with_report=False,
                      status=running_status(os.getppid()))
    (runs_dir / "not-a-run").mkdir()
    states = {r["id"]: r["state"] for r in client.get("/api/runs").json()}
    assert states == {"20260101-000001-old-done": "done", "20260101-000002-old-partial": "stopped",
                      "20260101-000003-dead-cli": "stopped", "20260101-000004-alive-cli": "running"}
    d = detail(client, "20260101-000002-old-partial")
    assert d["status"]["state"] == "stopped" and d["report"] is None and d["live"] is False
    assert detail(client, "20260101-000001-old-done")["status"]["stage"] == "finished"


def test_watcher_streams_a_run_written_by_another_process(client: TestClient, runs_dir: Path) -> None:
    run_id = make_external_run(runs_dir, with_report=False, status=running_status(os.getppid()))
    sse = SSEReader(client, run_id)
    try:
        first = sse.wait("status")
        assert first["state"] == "running" and sse.names()[0] == "status"

        # the "CLI" writes round 1 and a post, then finishes
        world = World(runs_dir / run_id / "world.db", seed=7)
        new_id = world.apply(1, Action(agent_id="a02", type=ActionType.POST, content="Round one post"))
        world.save_round_stats(RoundStats(round=1, new_posts=1, sentiment=0.2))
        world.close()
        added = sse.wait("posts_added")
        assert added == {"round": 1, "post_ids": [new_id]}

        write_status(runs_dir / run_id, running_status(os.getppid(), round=1))
        assert sse.wait("round_done")["round"] == 1
        wait_for(lambda: any(n == "status" and d["round"] == 1 for n, d in sse.events), what="status round 1")

        done = running_status(os.getppid(), round=2).model_copy(
            update={"state": RunState.DONE, "stage": RunStage.FINISHED})
        write_status(runs_dir / run_id, done)
        sse.wait("run_done")
        assert sse.events[-1][0] in ("status", "run_done")
    finally:
        sse.close()
    # external runs cannot be controlled from this server
    assert client.post(f"/api/runs/{run_id}/stop").status_code == 409
    assert client.post(f"/api/runs/{run_id}/events", json={"round": 2, "content": "x"}).status_code == 409
