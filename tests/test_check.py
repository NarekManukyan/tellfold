"""Check it (2B): repeat runs of one crowd and what held across them. FakeLLM only."""

from __future__ import annotations

import asyncio
import time
from collections.abc import Iterator
from pathlib import Path

import pytest
from fastapi.testclient import TestClient
from typer.testing import CliRunner

from tellfold import cli, runner
from tellfold.agree import bands, build_agreement, match_camps
from tellfold.compare import Member
from tellfold.llm import FakeLLM, LLMError
from tellfold.models import Agreement, Camp, CampGroupOut, CampMatches, Report, RunState, RunStatus, SimConfig, Study, StudyRun
from tellfold.server.app import create_app
from tellfold.server.manager import RunManager
from tellfold.study import AGREEMENT_FILE, StudyError, load_study, plan_variant, save_study, study_dir

EXAMPLE = Path(__file__).resolve().parent.parent / "examples" / "mossbook_pricing.md"
QUESTION = "Will buyers accept the subscription?"


def test_bands_take_the_lowest_and_highest_scored_run():
    out = bands([[(0, None), (1, 0.2), (2, -0.1)], [(0, None), (1, 0.5), (2, 0.1)], [(1, None), (2, 0.4)]])
    assert [(b.round, b.low, b.high, b.runs) for b in out] == [(0, None, None, 0), (1, 0.2, 0.5, 2), (2, -0.1, 0.4, 3)]


def test_match_camps_keeps_real_camps_once_per_run():
    matched = CampMatches(groups=[
        CampGroupOut(label="Stay on free", members=["A: Stayed on free", "B: Free is enough", "B: Upgraders",
                                                    "C: nope", "D: Stayed on free"]),
        CampGroupOut(label="Upgrade", members=["b: Upgraders", "A: Stayed on free"]),
        CampGroupOut(label="Empty", members=[]),
    ])
    out = match_camps(matched, loaded_camps={"A": ["Stayed on free", "Paid up"], "B": ["Free is enough", "Upgraders"],
                                             "C": ["Angry"]})
    assert [(c.label, c.names, c.runs) for c in out] == [
        ("Stay on free", ["A: Stayed on free", "B: Free is enough"], 2),
        ("Paid up", ["A: Paid up"], 1),  # left out by the model: a group of its own
        ("Upgraders", ["B: Upgraders"], 1),
        ("Angry", ["C: Angry"], 1),
    ]


async def _finished(runs: Path, name: str, seed: int, camps: list[str]) -> Member:
    run_dir = runs / name
    run_dir.mkdir(parents=True)
    seeds = runner.copy_seeds(run_dir, [EXAMPLE])
    config = SimConfig(question=QUESTION, seed_files=[str(s) for s in seeds], num_agents=4, rounds=2, batch_size=2,
                       seed=seed)
    await runner.run_pipeline(FakeLLM(cli.fake_responder), run_dir, config, seeds)
    report = Report.model_validate_json((run_dir / "report.json").read_text())
    report = report.model_copy(update={"camps": [Camp(name=n, members=["a01"], sentiment=0.1) for n in camps]})
    (run_dir / "report.json").write_text(report.model_dump_json())
    return Member(letter="ABC"[seed - 7], run_id=name, variant=f"Run {seed - 6}", run_dir=run_dir)


async def test_build_agreement(tmp_path: Path):
    runs = tmp_path / "runs"
    members = [await _finished(runs, "a", 7, ["Fans", "Doubters"]), await _finished(runs, "b", 8, ["Supporters"]),
               await _finished(runs, "c", 9, ["Happy", "Sceptics"])]
    seen: dict[str, str] = {}

    def respond(purpose, prompt, schema):
        seen[purpose] = prompt
        if purpose == "study.match_camps":
            return {"groups": [{"label": "Fans", "members": ["A: Fans", "B: Supporters", "C: Happy"]},
                               {"label": "Doubters", "members": ["A: Doubters", "C: Sceptics"]}]}
        return {"held": ["Most buyers accept it"], "split": ["Students leave (A and C)"], "verdict": "consistent",
                "summary": "The conclusion held in all three runs."}

    agreement = await build_agreement(FakeLLM(respond), "s1", members, question=QUESTION, model="sonnet")
    assert list(seen) == ["study.match_camps", "study.agree"]
    assert "C: Sceptics" in seen["study.match_camps"] and "## Run C" in seen["study.agree"]
    assert agreement.verdict == "consistent" and agreement.held == ["Most buyers accept it"]
    assert [(c.label, c.runs) for c in agreement.camps] == [("Fans", 3), ("Doubters", 2)]
    assert agreement.runs == 3 and [b.round for b in agreement.bands] == [0, 1, 2]
    assert agreement.spread is None or agreement.spread >= 0


async def test_agreement_without_camps_skips_the_match_call(tmp_path: Path):
    runs = tmp_path / "runs"
    members = [await _finished(runs, "a", 7, []), await _finished(runs, "b", 8, [])]
    llm = FakeLLM(cli.fake_responder)
    agreement = await build_agreement(llm, "s1", members, question=QUESTION, model="sonnet")
    assert [c[0] for c in llm.calls] == ["study.agree"]
    assert agreement.camps == [] and agreement.verdict in ("consistent", "mixed", "unstable")


# ---------------------------------------------------------------- server

SEED = b"# Harbour Town ferry fare\n\nThe council proposes raising the ferry fare from 2 to 3 euros.\n"


@pytest.fixture
def client(tmp_path: Path) -> Iterator[tuple[TestClient, Path, list[FakeLLM]]]:
    runs = tmp_path / "runs"
    runs.mkdir()
    made: list[FakeLLM] = []

    def factory(run_dir, concurrency, think):
        llm = FakeLLM(cli.fake_responder)
        llm.run_dir = run_dir
        made.append(llm)
        return llm

    with TestClient(create_app(runs, factory, fake_llm=True, poll_interval=0.05, shutdown_grace=0.5)) as c:
        yield c, runs, made


def _wait(predicate, what: str, timeout: float = 60):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if value := predicate():
            return value
        time.sleep(0.03)
    raise AssertionError(f"timed out waiting for {what}")


def _start(c: TestClient) -> str:
    res = c.post("/api/runs", data={"question": "Will Harbour Town accept the fare?", "agents": "4", "rounds": "2",
                                    "batch": "2"}, files=[("files[]", ("fare.md", SEED, "text/markdown"))])
    assert res.status_code == 200, res.text
    run_id = res.json()["id"]
    _wait(lambda: (d := c.get(f"/api/runs/{run_id}").json())["status"]["state"] == "done" and not d["live"],
          "the first run")
    return run_id


def test_check_it_runs_one_after_another_and_writes_the_agreement(client):
    c, runs, made = client
    source = _start(c)
    res = c.post(f"/api/runs/{source}/check", json={"runs": 3})
    assert res.status_code == 200, res.text
    study_id = res.json()["study_id"]
    first = c.get(f"/api/studies/{study_id}").json()
    assert first["kind"] == "repeat" and first["planned"] == 3

    study = _wait(lambda: (s := c.get(f"/api/studies/{study_id}").json())["agreement"] and s, "the agreement")
    assert [(r["variant"], r["letter"]) for r in study["runs"]] == [("Run 1", "A"), ("Run 2", "B"), ("Run 3", "C")]
    assert study["repeating"] is False and study["compare"] is None
    ids = [r["id"] for r in study["runs"]]
    configs = [SimConfig.model_validate(c.get(f"/api/runs/{i}").json()["config"]) for i in ids]
    assert [cfg.seed for cfg in configs] == [7, 8, 9]
    assert [cfg.cast_from for cfg in configs[1:]] == [source, source]
    assert {cfg.question for cfg in configs} == {configs[0].question}
    assert all((runs / i / "seeds" / "fare.md").read_bytes() == SEED for i in ids)
    # one after another: each new run started after the previous one finished
    statuses = [RunStatus.model_validate(c.get(f"/api/runs/{i}").json()["status"]) for i in ids]
    assert statuses[1].updated_at <= statuses[2].started_at
    # no persona calls for the repeats; the agreement's calls are logged in the study folder
    for i in ids[1:]:
        llm = next(x for x in made if x.run_dir == runs / i)
        assert not any(p.startswith(("graph.", "personas.")) for p, _, _ in llm.calls)
    folder = study_dir(runs, study_id)
    assert any(p == "study.agree" for x in made if x.run_dir == folder for p, _, _ in x.calls)
    agreement = Agreement.model_validate_json((folder / AGREEMENT_FILE).read_text())
    assert agreement.runs == 3 and agreement.verdict in ("consistent", "mixed", "unstable")

    listed = {s["id"]: s for s in c.get("/api/studies").json()}
    assert listed[study_id]["has_compare"] is True and listed[study_id]["planned"] == 3

    # the check can be redone, and an agent model can be swapped for the new runs
    assert c.post(f"/api/studies/{study_id}/compare").status_code == 200
    res = c.post(f"/api/runs/{source}/check", json={"runs": 2, "agent_model": "other-model"})
    second = _wait(lambda: (s := c.get(f"/api/studies/{res.json()['study_id']}").json())["agreement"] and s,
                   "the second check")
    assert c.get(f"/api/runs/{second['runs'][1]['id']}").json()["config"]["agent_model"] == "other-model"


def test_check_validation(client):
    c, runs, _ = client
    source = _start(c)
    assert c.post(f"/api/runs/{source}/check", json={"runs": 6}).status_code == 422
    assert c.post(f"/api/runs/{source}/check", json={"runs": 1}).status_code == 422
    assert c.post(f"/api/runs/{source}/check", json={"agent_model": "bad model!"}).status_code == 422
    assert c.post("/api/runs/nope/check", json={}).status_code == 404
    (runs / source / "report.json").unlink()
    res = c.post(f"/api/runs/{source}/check", json={})
    assert res.status_code == 409 and "finished run with a report" in res.json()["detail"]
    assert load_study(runs, "anything") is None


# ---------------------------------------------------------------- CLI


def test_cli_check(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv(cli.FAKE_ENV, "1")
    out = tmp_path / "runs"
    cli_runner = CliRunner()
    args = ["run", str(EXAMPLE), "-q", QUESTION, "--agents", "4", "--rounds", "1", "--batch", "2", "--out", str(out)]
    assert cli_runner.invoke(cli.app, args).exit_code == 0
    (first,) = list(out.iterdir())
    result = cli_runner.invoke(cli.app, ["check", str(first), "--runs", "2"])
    assert result.exit_code == 0, result.output
    assert "across 2 runs" in result.output
    (folder,) = list((out / "studies").iterdir())
    agreement = Agreement.model_validate_json((folder / AGREEMENT_FILE).read_text())
    assert agreement.runs == 2
    study = load_study(out, folder.name)
    assert study.kind == "repeat" and [r.variant for r in study.runs] == ["Run 1", "Run 2"]


def test_a_series_that_stops_early_says_why(tmp_path: Path):
    runs = tmp_path / "runs"
    runs.mkdir()
    broken: set[Path] = set()

    def factory(run_dir, concurrency, think):
        def respond(purpose, prompt, schema):
            if run_dir in broken:
                return LLMError("the model is gone")
            return cli.fake_responder(purpose, prompt, schema)
        return FakeLLM(respond)

    with TestClient(create_app(runs, factory, fake_llm=True, poll_interval=0.05, shutdown_grace=0.5)) as c:
        source = _start(c)
        manager = c.app.state.manager
        real_create = manager.create_run_dir

        def create_broken(question):
            folder = real_create(question)
            broken.add(folder)
            return folder

        manager.create_run_dir = create_broken
        study_id = c.post(f"/api/runs/{source}/check", json={"runs": 3}).json()["study_id"]
        study = _wait(lambda: (s := c.get(f"/api/studies/{study_id}").json())["compare_state"]["error"] and s,
                      "the reason")
        assert study["compare_state"]["error"] == "Run 2 did not finish, so the check stopped after 2 of 3 runs"
        assert study["planned"] == 2 and study["repeating"] is False and len(study["runs"]) == 2
        assert load_study(runs, study_id).planned == 2


async def test_no_check_run_starts_once_the_server_is_closing(tmp_path: Path):
    runs = tmp_path / "runs"
    member = await _finished(runs, "src", 7, [])
    gate = asyncio.Event()

    async def respond(purpose, prompt, schema):
        if purpose.startswith("engine."):
            await gate.wait()
        return cli.fake_responder(purpose, prompt, schema)

    manager = RunManager(runs_dir=runs, llm_factory=lambda *a: FakeLLM(respond), shutdown_grace=2)
    study = await manager.check(member.run_id, 3)
    while len(load_study(runs, study.id).runs) < 2 or not manager.live_ids():
        await asyncio.sleep(0.01)
    manager.run_state = lambda run_id: RunState.DONE  # Run 2 ends normally during the grace wait
    closing = asyncio.create_task(manager.shutdown())
    await asyncio.sleep(0.05)
    gate.set()
    await closing
    await asyncio.sleep(0.1)
    assert [r.variant for r in load_study(runs, study.id).runs] == ["Run 1", "Run 2"]  # Run 3 never started
    assert not manager.live_ids()


def test_a_new_version_of_a_checked_run_starts_its_own_study(tmp_path: Path):
    runs = tmp_path / "runs"
    check = Study(id="chk", kind="repeat", planned=2, runs=[StudyRun(id="src", variant="Run 1"),
                                                             StudyRun(id="r2", variant="Run 2")])
    save_study(runs, check)
    repeat = SimConfig(question=QUESTION, seed_files=[], study_id="chk", variant="Run 2", cast_from="src")
    study, label = plan_variant(runs, "r2", repeat)
    assert study.id != "chk" and study.kind == "compare"
    assert [(r.id, r.variant) for r in study.runs] == [("r2", "Draft A")] and label == "Draft B"
    with pytest.raises(StudyError, match="Check it"):
        plan_variant(runs, "r2", repeat, study_id="chk")
    with pytest.raises(StudyError, match="Check it"):
        plan_variant(runs, "src", SimConfig(question=QUESTION, seed_files=[]), study_id="chk")
