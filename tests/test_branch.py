"""Branch a run from a round (2D): memory history, truncation, continuing, the route and the CLI. FakeLLM only."""

from __future__ import annotations

import shutil
import sqlite3
import time
from pathlib import Path

import pytest
from fastapi.testclient import TestClient
from typer.testing import CliRunner

from tellfold import cli, runner
from tellfold.branch import branch_rounds, prepare_branch
from tellfold.llm import FakeLLM
from tellfold.models import GodEvent, RunStage, RunStatus, SimConfig
from tellfold.server.app import create_app
from tellfold.study import load_study
from tellfold.world import World

EXAMPLE = Path(__file__).resolve().parent.parent / "examples" / "mossbook_pricing.md"
QUESTION = "Will buyers accept the subscription?"


def make_config(**kw) -> SimConfig:
    return SimConfig(**({"question": QUESTION, "seed_files": [str(EXAMPLE)], "num_agents": 6, "rounds": 3,
                         "batch_size": 3} | kw))


async def finished(runs: Path, name: str, **kw) -> tuple[Path, SimConfig]:
    run_dir = runs / name
    run_dir.mkdir(parents=True)
    seeds = runner.copy_seeds(run_dir, [EXAMPLE])
    config = make_config(seed_files=[str(s) for s in seeds], **kw)
    await runner.run_pipeline(FakeLLM(cli.fake_responder), run_dir, config, seeds)
    return run_dir, config


def table(db: Path, sql: str) -> list[tuple]:
    conn = sqlite3.connect(db)
    try:
        return conn.execute(sql).fetchall()
    finally:
        conn.close()


STATE = {
    "posts": "SELECT id, round, author_id, content, parent_id, repost_of, likes, dislikes, reposts, replies "
             "FROM posts ORDER BY id",
    "reactions": "SELECT agent_id, post_id, kind FROM reactions ORDER BY agent_id, post_id",
    "follows": "SELECT follower, followee FROM follows ORDER BY follower, followee",
    "actions": "SELECT round, data FROM actions ORDER BY seq",
    "round_stats": "SELECT round, data FROM round_stats ORDER BY round",
    "events": "SELECT round, content FROM events ORDER BY seq",
    "memories": "SELECT agent_id, notes FROM memories ORDER BY agent_id",
    "sentiment": "SELECT post_id, score FROM post_sentiment ORDER BY post_id",
}


async def test_memories_are_kept_for_every_round(tmp_path: Path):
    run_dir, _ = await finished(tmp_path / "runs", "a")
    world = World(run_dir / "world.db")
    assert world.memory_rounds() == [0, 1, 2, 3] and world.last_round() == 3
    world.close()
    last = table(run_dir / "world.db", "SELECT agent_id, notes FROM memory_history WHERE round = 3 ORDER BY agent_id")
    assert last == table(run_dir / "world.db", STATE["memories"])


async def test_truncating_gives_the_world_a_shorter_run_would_have(tmp_path: Path):
    runs = tmp_path / "runs"
    long, _ = await finished(runs, "long", events=[GodEvent(round=3, content="A rival cuts prices")])
    short, _ = await finished(runs, "short", rounds=2, events=[GodEvent(round=3, content="A rival cuts prices")])
    copy = tmp_path / "copy.db"
    shutil.copyfile(long / "world.db", copy)
    world = World(copy)
    world.truncate_after(2)
    world.close()
    for name, sql in STATE.items():
        if name == "round_stats":  # llm_seconds differ between runs
            assert [r[0] for r in table(copy, sql)] == [r[0] for r in table(short / "world.db", sql)]
        else:
            assert table(copy, sql) == table(short / "world.db", sql), name
    assert table(copy, "SELECT MAX(round) FROM memory_history") == [(2,)]
    # the counters picked up again where the kept rows end
    assert table(copy, "SELECT seq FROM sqlite_sequence WHERE name = 'posts'") == table(copy, "SELECT MAX(id) FROM posts")


async def test_branch_continues_with_new_events(tmp_path: Path):
    runs = tmp_path / "runs"
    source, config = await finished(runs, "src", events=[GodEvent(round=3, content="The paper runs the story")])
    target = runs / "branch"
    target.mkdir()
    new = prepare_branch(source, target, config, 1, [GodEvent(round=2, content="The owner apologises")],
                         study_id="s1", variant="Branch at round 1")
    assert (new.branched_from, new.branch_round, new.cast_from) == ("src", 1, "src")
    assert [(e.round, e.content) for e in new.events] == [(2, "The owner apologises")]  # round 3's story is dropped
    assert (target / "seeds" / EXAMPLE.name).is_file() and (target / "personas.json").is_file()

    llm = FakeLLM(cli.fake_responder)
    events: list[tuple[str, dict]] = []
    seeds = [Path(p) for p in new.seed_files]
    await runner.run_pipeline(llm, target, new, seeds, on_event=lambda n, p: events.append((n, p)))

    purposes = [c[0] for c in llm.calls]
    assert not any(p.startswith(("graph.", "personas.", "engine.seed")) for p in purposes)
    assert [p["round"] for n, p in events if n == "round_start"] == [2, 3]
    statuses = [RunStatus.model_validate(p) for n, p in events if n == "status"]
    assert RunStage.SEEDING in {s.stage for s in statuses} and statuses[-1].state.value == "done"
    src_db, new_db = source / "world.db", target / "world.db"
    early = "SELECT id, round, author_id, content, parent_id, repost_of FROM posts WHERE round <= 1 ORDER BY id"
    assert table(new_db, early) == table(src_db, early)  # rounds 0 and 1 are shared exactly
    assert table(new_db, "SELECT round, content FROM events ORDER BY seq") == [(2, "The owner apologises")]
    kept = table(src_db, "SELECT MAX(id) FROM posts WHERE round <= 1")[0][0]
    later = [r[0] for r in table(new_db, "SELECT id FROM posts WHERE round > 1 ORDER BY id")]
    assert later and later[0] == kept + 1
    assert [r[0] for r in table(new_db, "SELECT round FROM round_stats ORDER BY round")] == [0, 1, 2, 3]
    assert (target / "report.json").is_file()


async def test_older_runs_branch_only_from_round_zero_or_their_last_round(tmp_path: Path):
    runs = tmp_path / "runs"
    source, config = await finished(runs, "src")
    db = source / "world.db"
    conn = sqlite3.connect(db)
    conn.execute("DROP TABLE memory_history")  # a run made before memories were kept per round
    conn.commit()
    conn.close()
    assert branch_rounds(source) == (3, [0, 3])
    with pytest.raises(runner.RunError, match="branched from round 0, 3"):
        prepare_branch(source, runs / "x", config, 1, [])
    (runs / "y").mkdir()
    prepare_branch(source, runs / "y", config, 0, [])
    assert table(runs / "y" / "world.db", "SELECT COUNT(*) FROM memories") == [(0,)]
    assert table(runs / "y" / "world.db", "SELECT MAX(round) FROM posts") == [(0,)]
    with pytest.raises(runner.RunError, match="needs at least one round"):
        prepare_branch(source, runs / "z", config, 3, [])
    with pytest.raises(runner.RunError, match="outside 1..3"):
        prepare_branch(source, runs / "z", config, 0, [GodEvent(round=0, content="too early")])


def test_estimate_of_a_branch():
    full = runner.estimate_call_split(make_config(rounds=6), 4)
    branch = runner.estimate_call_split(make_config(rounds=6, branched_from="a", branch_round=4), 4)
    per_round = (full[0] - (runner.estimate_call_split(make_config(rounds=5), 4)[0]))
    assert branch == (2 * per_round, 1)


# ---------------------------------------------------------------- server and CLI

SEED = b"# Harbour Town ferry fare\n\nThe council proposes raising the ferry fare from 2 to 3 euros.\n"


def test_branch_route_joins_a_study_and_compares(tmp_path: Path):
    runs = tmp_path / "runs"
    runs.mkdir()
    app = create_app(runs, lambda d, c, t: FakeLLM(cli.fake_responder), fake_llm=True, poll_interval=0.05)
    with TestClient(app) as c:
        rid = c.post("/api/runs", data={"question": "Will they accept it?", "agents": "4", "rounds": "3"},
                     files=[("files[]", ("fare.md", SEED, "text/markdown"))]).json()["id"]
        deadline = time.monotonic() + 30
        while c.get(f"/api/runs/{rid}").json()["status"]["state"] != "done" and time.monotonic() < deadline:
            time.sleep(0.03)
        assert c.post(f"/api/runs/{rid}/branch", json={"round": 3}).status_code == 400  # nothing left to run
        assert c.post(f"/api/runs/{rid}/branch", json={"round": -1}).status_code == 422
        res = c.post(f"/api/runs/{rid}/branch",
                     json={"round": 1, "events": [{"round": 2, "content": "The mayor apologises"}]})
        assert res.status_code == 200, res.text
        body = res.json()
        study = None
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            study = c.get(f"/api/studies/{body['study_id']}").json()
            if study["compare"]:
                break
            time.sleep(0.05)
        assert [(r["id"], r["variant"]) for r in study["runs"]] == [(rid, "Original"), (body["id"], "Branch at round 1")]
        config = c.get(f"/api/runs/{body['id']}").json()["config"]
        assert config["branched_from"] == rid and config["branch_round"] == 1
        assert study["compare"] is not None
        assert [d.name for d in runs.iterdir() if d.is_dir()].count("studies") == 1

        # Check it on the branch: its repeats are whole runs with the branch's people, not branches
        check_id = c.post(f"/api/runs/{body['id']}/check", json={"runs": 2}).json()["study_id"]
        deadline = time.monotonic() + 30
        while not (check := c.get(f"/api/studies/{check_id}").json())["agreement"] and time.monotonic() < deadline:
            assert not check["compare_state"]["error"], check
            time.sleep(0.05)
        repeat = c.get(f"/api/runs/{check['runs'][1]['id']}").json()
        assert repeat["status"]["state"] == "done"
        assert (repeat["config"]["branched_from"], repeat["config"]["cast_from"]) == (None, body["id"])


def test_cli_branch(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv(cli.FAKE_ENV, "1")
    out = tmp_path / "runs"
    args = ["run", str(EXAMPLE), "-q", QUESTION, "--agents", "4", "--rounds", "2", "--batch", "2", "--out", str(out),
            "--event", "2:The paper runs the story"]
    assert CliRunner().invoke(cli.app, args).exit_code == 0
    (source,) = list(out.iterdir())
    result = CliRunner().invoke(cli.app, ["branch", str(source), "--at", "1", "--event", "2:The owner apologises"])
    assert result.exit_code == 0, result.output
    branch = next(p for p in out.iterdir() if p.is_dir() and p not in (source, out / "studies"))
    config = SimConfig.model_validate_json((branch / "config.json").read_text())
    assert [e.content for e in config.events] == ["The paper runs the story", "The owner apologises"]
    study = load_study(out, config.study_id)
    assert [r.variant for r in study.runs] == ["Original", "Branch at round 1"]
    bad = CliRunner().invoke(cli.app, ["branch", str(source), "--at", "2"])
    assert bad.exit_code == 1 and "at least one round" in bad.output
    assert len([p for p in out.iterdir() if p.is_dir()]) == 3  # the failed branch left no folder


def test_cli_branch_keeps_events_added_while_the_run_went(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv(cli.FAKE_ENV, "1")
    out = tmp_path / "runs"
    args = ["run", str(EXAMPLE), "-q", QUESTION, "--agents", "4", "--rounds", "2", "--batch", "2", "--out", str(out)]
    assert CliRunner().invoke(cli.app, args).exit_code == 0
    (source,) = list(out.iterdir())
    world = World(source / "world.db")
    world.add_event(GodEvent(round=2, content="Added from the app mid-run"))  # not in config.json
    world.close()
    result = CliRunner().invoke(cli.app, ["branch", str(source), "--at", "1", "--event", "2:The owner apologises"])
    assert result.exit_code == 0, result.output
    branch = next(p for p in out.iterdir() if p.is_dir() and p not in (source, out / "studies"))
    config = SimConfig.model_validate_json((branch / "config.json").read_text())
    assert [e.content for e in config.events] == ["Added from the app mid-run", "The owner apologises"]


def test_a_branched_run_can_be_checked_and_its_repeats_branch_into_a_new_study(tmp_path: Path,
                                                                             monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv(cli.FAKE_ENV, "1")
    out = tmp_path / "runs"
    args = ["run", str(EXAMPLE), "-q", QUESTION, "--agents", "4", "--rounds", "2", "--batch", "2", "--out", str(out)]
    assert CliRunner().invoke(cli.app, args).exit_code == 0
    (source,) = list(out.iterdir())
    assert CliRunner().invoke(cli.app, ["branch", str(source), "--at", "1"]).exit_code == 0
    branch = next(p for p in out.iterdir() if p.is_dir() and p not in (source, out / "studies"))
    result = CliRunner().invoke(cli.app, ["check", str(branch), "--runs", "2"])
    assert result.exit_code == 0, result.output  # the repeat is a whole run, not a branch without a world
    repeat_dir = next(p for p in out.iterdir() if p.is_dir() and p not in (source, branch, out / "studies"))
    repeat = SimConfig.model_validate_json((repeat_dir / "config.json").read_text())
    assert (repeat.branched_from, repeat.branch_round, repeat.cast_from) == (None, None, branch.name)
    assert (repeat_dir / "report.json").is_file()
    check = load_study(out, repeat.study_id)
    assert check.kind == "repeat"

    # a branch of a repeat starts a study of its own: the check holds only repeats
    assert CliRunner().invoke(cli.app, ["branch", str(repeat_dir), "--at", "1"]).exit_code == 0
    assert load_study(out, repeat.study_id).runs == check.runs
    newest = max((p for p in out.iterdir() if p.is_dir() and p.name != "studies"), key=lambda p: p.stat().st_mtime)
    study = load_study(out, SimConfig.model_validate_json((newest / "config.json").read_text()).study_id)
    assert study.kind == "compare" and [r.variant for r in study.runs] == ["Original", "Branch at round 1"]
