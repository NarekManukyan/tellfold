"""run_pipeline: status.json lifecycle, stopping, mid-run event injection, per-post sentiment. FakeLLM only."""

from __future__ import annotations

import json
import os
import sys
import types
from pathlib import Path

import anyio
import pytest
from typer.testing import CliRunner

from tellfold import cli, runner
from tellfold.llm import FakeLLM, LLMError
from tellfold.models import GodEvent, Report, RunStage, RunState, RunStatus, SimConfig
from tellfold.world import World

EXAMPLE = Path(__file__).resolve().parent.parent / "examples" / "mossbook_pricing.md"


def make_config(**kw) -> SimConfig:
    base = dict(question="Will buyers accept the subscription?", seed_files=[str(EXAMPLE)],
                num_agents=6, rounds=3, batch_size=3)
    return SimConfig(**(base | kw))


def fake(fail_purpose: str | None = None) -> FakeLLM:
    def respond(purpose, prompt, schema):
        if fail_purpose and purpose.startswith(fail_purpose):
            return LLMError(f"simulated {purpose} failure")
        return cli.fake_responder(purpose, prompt, schema)

    return FakeLLM(respond)


class Recorder:
    def __init__(self, hook=None):
        self.events: list[tuple[str, dict]] = []
        self.hook = hook

    def __call__(self, name: str, payload: dict) -> None:
        json.dumps(payload)  # everything must be forwardable over SSE as is
        self.events.append((name, payload))
        if self.hook:
            self.hook(name, payload)

    def names(self) -> list[str]:
        return [n for n, _ in self.events]

    def of(self, name: str) -> list[dict]:
        return [p for n, p in self.events if n == name]


async def test_status_transitions_through_every_stage(run_dir: Path):
    rec = Recorder()
    report = await runner.run_pipeline(fake(), run_dir, make_config(), [EXAMPLE], on_event=rec)

    assert isinstance(report, Report)
    statuses = [RunStatus.model_validate(s) for s in rec.of("status")]
    stages = list(dict.fromkeys(s.stage for s in statuses))
    assert stages == [RunStage.READING, RunStage.GRAPH, RunStage.PERSONAS, RunStage.SEEDING,
                      RunStage.ROUNDS, RunStage.REPORT, RunStage.FINISHED]
    assert all(s.state is RunState.RUNNING for s in statuses[:-1])
    assert [s.round for s in statuses if s.detail.endswith("done") and "round" in s.detail] == [1, 2, 3]

    final = runner.read_status(run_dir)
    assert final == statuses[-1]
    assert (final.state, final.stage, final.round, final.total_rounds) == (RunState.DONE, RunStage.FINISHED, 3, 3)
    assert final.pid == os.getpid() and final.error is None
    assert final.started_at == statuses[0].started_at and final.updated_at >= final.started_at

    stage_events = rec.of("stage")
    assert {e["stage"] for e in stage_events} <= {s.value for s in RunStage}
    assert any(e.get("step") == "follow_graph" for e in stage_events)  # engine steps become SEEDING details
    names = rec.names()
    assert names[-1] == "run_done" and "run_failed" not in names and "run_stopped" not in names
    assert names.count("round_start") == 3 and names.count("round_done") == 4  # round 0 + 3 rounds
    assert names.index("posts_added") < names.index("round_done")
    for name in ("config.json", "graph.json", "personas.json", "world.db", "report.json", "report.html"):
        assert (run_dir / name).exists(), name
    assert not list(run_dir.glob(".status-*"))  # atomic writes leave no temp files


async def test_stop_between_rounds_still_writes_report(run_dir: Path):
    stop = anyio.Event()

    def hook(name, payload):
        if name == "round_start" and payload["round"] == 2:
            stop.set()  # set mid-round: round 2 must still finish

    rec = Recorder(hook)
    report = await runner.run_pipeline(fake(), run_dir, make_config(rounds=5), [EXAMPLE], on_event=rec, stop=stop)

    assert isinstance(report, Report) and (run_dir / "report.json").exists()
    status = runner.read_status(run_dir)
    assert (status.state, status.stage, status.round) == (RunState.STOPPED, RunStage.FINISHED, 2)
    assert [p["round"] for p in rec.of("round_done")] == [0, 1, 2]
    assert rec.names()[-1] == "run_stopped" and "run_done" not in rec.names()
    world = World(run_dir / "world.db")
    assert max(r for r, _ in world.actions()) == 2


async def test_stop_before_any_round_returns_none(run_dir: Path):
    stop = anyio.Event()

    def hook(name, payload):
        if name == "stage" and payload["stage"] == RunStage.SEEDING.value:
            stop.set()

    rec = Recorder(hook)
    result = await runner.run_pipeline(fake(), run_dir, make_config(), [EXAMPLE], on_event=rec, stop=stop)

    assert result is None and not (run_dir / "report.json").exists()
    status = runner.read_status(run_dir)
    assert (status.state, status.round) == (RunState.STOPPED, 0)
    assert rec.names()[-1] == "run_stopped" and "round_start" not in rec.names()


async def test_event_injected_mid_run(run_dir: Path):
    config = make_config(events=[GodEvent(round=3, content="Planned news")])
    llm = fake()

    def hook(name, payload):
        if name == "round_done" and payload["round"] == 1:
            ui = World(run_dir / "world.db")  # a second connection, like the server's
            ui.add_event(GodEvent(round=2, content="A rival halves its price"))
            ui.close()

    rec = Recorder(hook)
    await runner.run_pipeline(llm, run_dir, config, [EXAMPLE], on_event=rec)

    assert rec.of("event_injected") == [{"round": 2, "content": "A rival halves its price"}]
    round_prompts = [p for purpose, _, p in llm.calls if purpose == "engine.round"]
    starts = rec.of("round_start")
    round2 = round_prompts[sum(s["batches"] for s in starts[:1]):][0]
    assert "BREAKING NEWS" in round2 and "A rival halves its price" in round2
    world = World(run_dir / "world.db")
    assert {e.content for e in world.events()} == {"Planned news", "A rival halves its price"}


async def test_per_post_sentiment_stored(run_dir: Path):
    await runner.run_pipeline(fake(), run_dir, make_config(rounds=2), [EXAMPLE])
    world = World(run_dir / "world.db")
    scores = world.post_sentiments()
    posts = world.posts()
    assert scores and set(scores) <= {p.id for p in posts}  # the canned responder scores a subset
    by_round = {p.round for p in posts if p.id in scores}
    assert 0 in by_round and by_round & {1, 2}
    assert all(-1 <= v <= 1 for v in scores.values())


async def test_failure_marks_run_failed_and_reraises(run_dir: Path):
    rec = Recorder()
    with pytest.raises(LLMError, match="report.write"):
        await runner.run_pipeline(fake(fail_purpose="report"), run_dir, make_config(rounds=1), [EXAMPLE],
                                  on_event=rec)
    status = runner.read_status(run_dir)
    assert status.state is RunState.FAILED and "report.write" in status.error
    assert status.stage is RunStage.REPORT and status.round == 1
    assert rec.names()[-1] == "run_failed" and "report.write" in rec.of("run_failed")[0]["error"]


async def test_failed_batches_persist_in_status(run_dir: Path):
    """A failed batch is announced over SSE and also kept in status.json, so a reload still shows it."""
    rec = Recorder()
    await runner.run_pipeline(fake(fail_purpose="engine.round"), run_dir, make_config(rounds=2), [EXAMPLE],
                              on_event=rec)
    announced = rec.of("batch_failed")
    assert announced, "every round batch fails with this responder"
    status = runner.read_status(run_dir)
    assert status.state is RunState.DONE  # one failed batch never stops the run
    kept = [(b.round, b.batch, tuple(b.agents)) for b in status.failed_batches]
    assert kept == [(p["round"], p["batch"], tuple(p["agents"])) for p in announced]
    assert {b.round for b in status.failed_batches} == {1, 2}
    assert all("engine.round" in b.error and len(b.error) <= 500 for b in status.failed_batches)
    # the JSON on disk round-trips, and older status files without the field still load
    raw = json.loads((run_dir / "status.json").read_text())
    assert raw["failed_batches"][0]["agents"]
    del raw["failed_batches"]
    assert RunStatus.model_validate(raw).failed_batches == []


async def test_unreadable_seeds_fail_with_run_error(run_dir: Path, tmp_path: Path):
    empty = tmp_path / "empty.md"
    empty.write_text("   \n")
    with pytest.raises(runner.RunError):
        await runner.run_pipeline(fake(), run_dir, make_config(), [empty])
    assert runner.read_status(run_dir).state is RunState.FAILED


async def test_cancellation_marks_run_stopped(run_dir: Path):
    async with anyio.create_task_group() as tg:
        def hook(name, payload):
            if name == "round_done" and payload["round"] == 1:
                tg.cancel_scope.cancel()

        tg.start_soon(lambda: runner.run_pipeline(fake(), run_dir, make_config(), [EXAMPLE], on_event=Recorder(hook)))
    status = runner.read_status(run_dir)
    assert status.state is RunState.STOPPED and status.round == 1


async def test_broken_listener_does_not_break_the_run(run_dir: Path):
    def explode(name, payload):
        raise RuntimeError("listener bug")

    assert await runner.run_pipeline(fake(), run_dir, make_config(rounds=1), [EXAMPLE], on_event=explode)
    assert runner.read_status(run_dir).state is RunState.DONE


def test_read_status_tolerates_missing_and_garbage(tmp_path: Path):
    assert runner.read_status(tmp_path) is None
    (tmp_path / "status.json").write_text('{"state": "runn')
    assert runner.read_status(tmp_path) is None


def test_mark_stopped_only_touches_live_runs(tmp_path: Path):
    live = RunStatus(state=RunState.RUNNING, stage=RunStage.ROUNDS, round=2, total_rounds=4,
                     started_at="2026-01-01T00:00:00Z", updated_at="2026-01-01T00:00:00Z")
    runner.write_status(tmp_path, live)
    runner.mark_stopped(tmp_path)
    assert runner.read_status(tmp_path).state is RunState.STOPPED
    done = live.model_copy(update={"state": RunState.DONE})
    runner.write_status(tmp_path, done)
    runner.mark_stopped(tmp_path)
    assert runner.read_status(tmp_path).state is RunState.DONE


# ---------- CLI integration ----------


def test_cli_run_writes_done_status(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv(cli.FAKE_ENV, "1")
    result = CliRunner().invoke(cli.app, ["run", str(EXAMPLE), "-q", "q?", "--agents", "4", "--rounds", "1",
                                          "--out", str(tmp_path)])
    assert result.exit_code == 0, result.output
    (run_dir,) = list(tmp_path.iterdir())
    status = runner.read_status(run_dir)
    assert status.state is RunState.DONE and status.round == 1


def test_ui_command_builds_app_and_serves_on_localhost(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    import uvicorn

    monkeypatch.setenv(cli.FAKE_ENV, "1")
    seen: dict = {}

    def create_app(runs_dir, llm_factory):
        seen["runs_dir"], seen["factory"] = runs_dir, llm_factory
        return "the-app"

    package = types.ModuleType("tellfold.server")
    module = types.ModuleType("tellfold.server.app")
    module.create_app = create_app
    package.app = module
    monkeypatch.setitem(sys.modules, "tellfold.server", package)
    monkeypatch.setitem(sys.modules, "tellfold.server.app", module)
    monkeypatch.setattr(uvicorn, "run", lambda app, **kw: seen.update(app=app, **kw))
    opened: list = []
    monkeypatch.setattr(cli.webbrowser, "open", lambda url: opened.append(url))
    monkeypatch.setattr(cli, "port_free", lambda port: True)

    runs = tmp_path / "runs"
    result = CliRunner().invoke(cli.app, ["ui", "--runs", str(runs), "--port", "8799", "--no-open"])
    assert result.exit_code == 0, result.output
    assert "http://127.0.0.1:8799/" in result.output and "fake LLM" in result.output
    assert seen["app"] == "the-app" and seen["host"] == "127.0.0.1" and seen["port"] == 8799
    assert seen["runs_dir"] == runs.resolve() and runs.is_dir()
    assert isinstance(seen["factory"](None, 2, False), FakeLLM)
    assert isinstance(seen["factory"](tmp_path, 1, True), FakeLLM)
    assert not opened


def test_ui_refuses_busy_port(monkeypatch: pytest.MonkeyPatch, tmp_path: Path):
    monkeypatch.setattr(cli, "port_free", lambda port: False)
    result = CliRunner().invoke(cli.app, ["ui", "--runs", str(tmp_path), "--no-open"])
    assert result.exit_code == 1 and "already in use" in result.output


async def test_language_reaches_every_writing_prompt(run_dir: Path):
    """A French run asks for French in the persona, opening, round and report prompts."""
    systems: dict[str, str] = {}

    class Spy(FakeLLM):
        async def text(self, prompt, *, system, model, purpose):
            systems.setdefault(purpose, system)
            return await super().text(prompt, system=system, model=model, purpose=purpose)

        async def json(self, prompt, *, system, model, purpose, schema):
            systems.setdefault(purpose, system)
            return await super().json(prompt, system=system, model=model, purpose=purpose, schema=schema)

    llm = Spy(lambda purpose, prompt, schema: cli.fake_responder(purpose, prompt, schema))
    await runner.run_pipeline(llm, run_dir, make_config(language="French"), [EXAMPLE])
    persona_prompts = [prompt for purpose, _, prompt in llm.calls if purpose == "personas.batch"]
    assert persona_prompts and all("in French" in p for p in persona_prompts)
    assert "in French" in systems["report.write"] and "camp names" in systems["report.write"]
    engine_systems = [s for purpose, s in systems.items() if purpose.startswith("engine.") and "in French" in s]
    assert engine_systems, sorted(systems)
