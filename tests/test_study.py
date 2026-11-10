"""Studies (2A): reusing a cast, the per-run digest, the comparison and its prefixed citations. FakeLLM only."""

from __future__ import annotations

import json
import time
from collections.abc import Iterator
from pathlib import Path

import pytest
from fastapi.testclient import TestClient
from typer.testing import CliRunner

from tellfold import cli, runner
from tellfold.compare import COMPARE_FILE, Member, build_compare, digest_run
from tellfold.llm import FakeLLM
from tellfold.models import Post, Report, RunStage, RunStatus, SimConfig
from tellfold.report import UNVERIFIED, audit_texts
from tellfold.server.app import create_app
from tellfold.study import join_study, load_study, plan_variant

EXAMPLE = Path(__file__).resolve().parent.parent / "examples" / "mossbook_pricing.md"
QUESTION = "Will buyers accept the subscription?"


def make_config(**kw) -> SimConfig:
    base = dict(question=QUESTION, seed_files=[str(EXAMPLE)], num_agents=6, rounds=2, batch_size=3)
    return SimConfig(**(base | kw))


def fake() -> FakeLLM:
    return FakeLLM(cli.fake_responder)


async def finished_run(runs: Path, name: str, **kw) -> Path:
    run_dir = runs / name
    run_dir.mkdir(parents=True)
    seeds = runner.copy_seeds(run_dir, [EXAMPLE])
    await runner.run_pipeline(fake(), run_dir, make_config(**kw), seeds)
    return run_dir


# ---------------------------------------------------------------- reusing a cast


async def test_cast_reuse_skips_graph_and_persona_calls(tmp_path: Path):
    runs = tmp_path / "runs"
    source = await finished_run(runs, "a")
    target = runs / "b"
    target.mkdir()
    llm = fake()
    events: list[tuple[str, dict]] = []
    report = await runner.run_pipeline(llm, target, make_config(cast_from="a", num_agents=12), [EXAMPLE],
                                       on_event=lambda n, p: events.append((n, p)))

    assert isinstance(report, Report)
    purposes = {c[0] for c in llm.calls}
    assert not any(p.startswith(("graph.", "personas.")) for p in purposes), purposes
    assert "report.write" in purposes and any(p.startswith("engine.") for p in purposes)
    for name in ("graph.json", "personas.json"):
        assert json.loads((target / name).read_text()) == json.loads((source / name).read_text())
    # the skipped stages are still announced, marked as reused
    stages = [p for n, p in events if n == "stage"]
    assert [s["stage"] for s in stages if s.get("reused")] == ["graph", "personas"]
    statuses = [RunStatus.model_validate(p) for n, p in events if n == "status"]
    assert list(dict.fromkeys(s.stage for s in statuses)) == [
        RunStage.READING, RunStage.GRAPH, RunStage.PERSONAS, RunStage.SEEDING, RunStage.ROUNDS, RunStage.REPORT,
        RunStage.FINISHED]
    # the crowd is the source's, whatever size was asked for
    assert SimConfig.model_validate_json((target / "config.json").read_text()).num_agents == 6
    estimate = next(s["estimated_calls"] for s in stages if s["stage"] == "graph")
    assert estimate == sum(runner.estimate_call_split(make_config(cast_from="a"), 1))


async def test_cast_reuse_from_a_run_without_people_fails(tmp_path: Path):
    runs = tmp_path / "runs"
    (runs / "empty").mkdir(parents=True)
    (runs / "b").mkdir()
    with pytest.raises(runner.RunError, match="no topic map or people"):
        await runner.run_pipeline(fake(), runs / "b", make_config(cast_from="empty"), [EXAMPLE])


def test_estimate_without_graph_and_personas():
    full = runner.estimate_call_split(make_config(num_agents=20, rounds=6, batch_size=6), 10)
    reused = runner.estimate_call_split(make_config(num_agents=20, rounds=6, batch_size=6, cast_from="x"), 10)
    assert reused == (full[0], 1)  # the report only


def test_old_configs_still_load():
    old = {"question": "q", "seed_files": []}
    config = SimConfig.model_validate(old)
    assert (config.cast_from, config.study_id, config.variant) == (None, None, None)


# ---------------------------------------------------------------- studies on disk


def test_plan_variant_starts_a_study_with_the_source_as_draft_a(tmp_path: Path):
    study, label = plan_variant(tmp_path, "src", make_config())
    assert [(r.id, r.variant) for r in study.runs] == [("src", "Draft A")]
    assert label == "Draft B"
    assert load_study(tmp_path, study.id) is None  # nothing written until the run exists
    join_study(tmp_path, study, "new", label)
    saved = load_study(tmp_path, study.id)
    assert [(r.id, r.variant) for r in saved.runs] == [("src", "Draft A"), ("new", "Draft B")]

    # a third version made from Draft B joins the same study
    third, label = plan_variant(tmp_path, "new", make_config(study_id=study.id, variant="Draft B"))
    assert third.id == study.id and label == "Draft C"
    third, label = plan_variant(tmp_path, "src", make_config(variant="Calm"), study_id=study.id, variant=" Bold ")
    assert label == "Bold" and [r.variant for r in third.runs][:1] == ["Draft A"]


# ---------------------------------------------------------------- digest


async def test_digest_matches_the_run(tmp_path: Path):
    run_dir = await finished_run(tmp_path / "runs", "a")
    digest = digest_run(Member(letter="A", run_id="a", variant="Draft A", run_dir=run_dir), "done")
    report = Report.model_validate_json((run_dir / "report.json").read_text())

    from tellfold.world import World

    world = World(run_dir / "world.db")
    stats, posts = world.round_stats(), world.posts()
    world.close()
    assert [p.round for p in digest.rounds] == [s.round for s in stats] == [0, 1, 2]
    assert [p.sentiment for p in digest.rounds] == [s.sentiment for s in stats]
    assert sum(p.posts for p in digest.rounds) == digest.posts == len(posts)
    scored = [s.sentiment for s in stats if s.sentiment is not None]
    assert digest.final_sentiment == (round(scored[-1], 3) if scored else None)
    assert [(c.name, c.size) for c in digest.camps] == [(c.name, len(c.members)) for c in report.camps]
    assert digest.objections == report.risks
    assert digest.headline == report.title and digest.state == "done"
    assert all(s.first_post < s.last_post for s in digest.shifts)


def test_digest_of_a_run_that_has_not_started(tmp_path: Path):
    (tmp_path / "a").mkdir()
    digest = digest_run(Member(letter="A", run_id="a", variant="Draft A", run_dir=tmp_path / "a"))
    assert (digest.rounds, digest.camps, digest.posts, digest.final_sentiment) == ([], [], 0, None)


# ---------------------------------------------------------------- prefixed citations


def _posts(*items: tuple[int, str]) -> dict[int, Post]:
    return {pid: Post(id=pid, round=1, author_id="a01", content=text) for pid, text in items}


def test_audit_understands_run_prefixes():
    runs = {"A": _posts((1, "The new price is fair for what we get")),
            "B": _posts((2, "I will cancel my plan today"))}
    texts = [
        'A#1 says "the new price is fair" while B#2 threatens to leave.',
        'B#99 does not exist and A#2 is in the other run.',
        'One reader wrote "I will stay forever" (B#2).',
        "A bare #1 is ambiguous in a comparison, @nobody is unknown, @maria is known.",
    ]
    audit = audit_texts(texts, {}, {"maria"}, runs=runs)
    assert audit.unknown_refs == ["B#99", "A#2"]
    assert audit.unknown_posts == [1]
    assert audit.unknown_handles == ["nobody"]
    assert [(q.ref, q.quote) for q in audit.misquotes] == [("B#2", "I will stay forever")]
    assert audit.problems == 5
    # 4 refs in text 1-2, a ref + a quote in text 3, a bare id + 2 handles in text 4, the good quote in text 1
    assert audit.checked == 10


def test_report_audit_ignores_prefixed_refs():
    audit = audit_texts(["Plan A#3 was popular, see #1."], _posts((1, "x")), set())
    assert (audit.checked, audit.problems) == (1, 0)


# ---------------------------------------------------------------- the comparison


async def test_build_compare_cites_both_runs_and_marks_bad_refs(tmp_path: Path):
    runs = tmp_path / "runs"
    a = await finished_run(runs, "a")
    b = await finished_run(runs, "b", seed=8)
    members = [Member(letter="A", run_id="a", variant="Draft A", run_dir=a),
               Member(letter="B", run_id="b", variant="Draft B", run_dir=b)]
    from tellfold.world import World

    wb = World(b / "world.db")
    post_b = wb.posts()[0]
    wb.close()
    wa = World(a / "world.db")
    post_a = wa.posts()[0]
    wa.close()
    seen: dict[str, str] = {}

    def respond(purpose, prompt, schema):
        seen[purpose] = prompt
        return {
            "headline": "The crowd preferred Draft B",
            "better_variant": "draft b",
            "why": f"A#{post_a.id} pushed back while B#{post_b.id} agreed; B#9999 is made up.",
            "per_variant": [{"variant": "A", "summary": f"Cooler, see A#{post_a.id}.", "objections": ["Price"]},
                            {"variant": "Draft B", "summary": f"Warmer, see B#{post_b.id}.", "objections": []},
                            {"variant": "Draft Z", "summary": "not a version", "objections": []}],
            "objections_only_in": [{"variant": "Draft A", "objections": ["Too vague"]},
                                   {"variant": "Draft B", "objections": []}],
            "caveats": ["Small crowd."],
        }

    result = await build_compare(FakeLLM(respond), "s1", members, question=QUESTION, model="sonnet")
    prompt = seen["compare.write"]
    assert f"A#{post_a.id}" in prompt and f"B#{post_b.id}" in prompt
    assert "A = Draft A" in prompt and "TEXT THE CROWD READ" in prompt

    report = result.report
    assert report.better_variant == "Draft B"
    assert [v.variant for v in report.per_variant] == ["Draft A", "Draft B"]
    assert [v.variant for v in report.objections_only_in] == ["Draft A"]
    assert UNVERIFIED in report.why and f"B#{post_b.id}" in report.why and "B#9999" not in report.why
    assert report.citation_audit.unknown_refs == ["B#9999"] and report.citation_audit.marked == 1
    assert report.caveats[0].startswith("Each version was played once")
    assert [d.letter for d in result.runs] == ["A", "B"]


async def test_build_compare_tie_and_unknown_label(tmp_path: Path):
    runs = tmp_path / "runs"
    a = await finished_run(runs, "a")
    b = await finished_run(runs, "b")
    members = [Member(letter="A", run_id="a", variant="Draft A", run_dir=a),
               Member(letter="B", run_id="b", variant="Draft B", run_dir=b)]
    llm = FakeLLM(lambda purpose, prompt, schema: {"headline": "A tie", "better_variant": "Draft Q", "why": "",
                                                   "per_variant": [], "objections_only_in": [], "caveats": []})
    result = await build_compare(llm, "s1", members, question=QUESTION, model="sonnet")
    assert result.report.better_variant is None
    assert [c[0] for c in llm.calls] == ["compare.write"]


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


def _start(c: TestClient, text: bytes = SEED, **extra: str) -> str:
    data = {"question": "Will Harbour Town accept the fare?", "agents": "4", "rounds": "2", "batch": "2", **extra}
    res = c.post("/api/runs", data=data, files=[("files[]", ("fare.md", text, "text/markdown"))])
    assert res.status_code == 200, res.text
    return res.json()["id"]


def _wait(predicate, what: str, timeout: float = 30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if value := predicate():
            return value
        time.sleep(0.03)
    raise AssertionError(f"timed out waiting for {what}")


def _done(c: TestClient, run_id: str) -> dict:
    return _wait(lambda: (d := c.get(f"/api/runs/{run_id}").json())["status"]["state"] == "done"
                 and not d["live"] and d, f"{run_id} to finish")


def test_try_another_version_and_compare(client):
    c, runs, made = client
    first = _start(c)
    _done(c, first)
    second = _start(c, b"# Harbour Town ferry fare\n\nThe fare rises to 3 euros; students keep 2.\n",
                    cast_from=first, agents="9")

    detail = _done(c, second)
    config = detail["config"]
    assert config["cast_from"] == first and config["variant"] == "Draft B" and config["num_agents"] == 4
    study_id = config["study_id"]
    second_llm = next(llm for llm in made if llm.run_dir == runs / second)
    assert not any(p.startswith(("graph.", "personas.")) for p, _, _ in second_llm.calls)

    study = _wait(lambda: (s := c.get(f"/api/studies/{study_id}").json())["compare"] and s, "the comparison")
    assert [(r["id"], r["variant"], r["letter"]) for r in study["runs"]] == [
        (first, "Draft A", "A"), (second, "Draft B", "B")]
    assert all(r["state"] == "done" and r["digest"]["rounds"] for r in study["runs"])
    assert study["compare_state"] == {"running": False, "error": None}
    assert (runs / "studies" / study_id / COMPARE_FILE).is_file()
    compare_llm = next(llm for llm in made if llm.run_dir == runs / "studies" / study_id)
    assert [p for p, _, _ in compare_llm.calls] == ["compare.write"]

    listed = c.get("/api/studies").json()
    assert [(s["id"], s["has_compare"], [r["letter"] for r in s["runs"]]) for s in listed] == [
        (study_id, True, ["A", "B"])]
    runs_list = {r["id"]: r for r in c.get("/api/runs").json()}
    assert runs_list[second]["study_id"] == study_id and runs_list[second]["variant"] == "Draft B"
    assert runs_list[first]["study_id"] is None  # joined through study.json only

    # rebuilding the comparison makes one more call
    assert c.post(f"/api/studies/{study_id}/compare").json() == {"ok": True}
    _wait(lambda: not c.get(f"/api/studies/{study_id}").json()["compare_state"]["running"], "the rebuild")
    assert sum(p == "compare.write" for llm in made for p, _, _ in llm.calls) == 2

    # a third version from Draft B joins the same study
    third = _start(c, cast_from=second, variant="Students free")
    assert c.get(f"/api/runs/{third}").json()["config"]["study_id"] == study_id
    assert [r.variant for r in load_study(runs, study_id).runs] == ["Draft A", "Draft B", "Students free"]
    _done(c, third)


def test_create_run_validates_study_fields(client):
    c, runs, _ = client
    data = {"question": "q?"}
    files = [("files[]", ("fare.md", SEED, "text/markdown"))]
    assert c.post("/api/runs", data=data | {"cast_from": "nope"}, files=files).status_code == 400
    assert c.post("/api/runs", data=data | {"cast_from": "../x"}, files=files).status_code == 400
    assert c.post("/api/runs", data=data | {"study_id": "abc"}, files=files).status_code == 400
    (runs / "half").mkdir()
    (runs / "half" / "config.json").write_text(make_config().model_dump_json())
    res = c.post("/api/runs", data=data | {"cast_from": "half"}, files=files)
    assert res.status_code == 400 and "no people" in res.json()["detail"]
    assert [p.name for p in runs.iterdir()] == ["half"]  # nothing created
    assert c.get("/api/studies/nope").status_code == 404
    assert c.get("/api/studies/..bad").status_code == 400
    assert c.get("/api/studies").json() == []


def test_compare_needs_two_finished_runs(client):
    c, runs, _ = client
    first = _start(c)
    _done(c, first)
    study, label = plan_variant(runs, first, make_config())
    join_study(runs, study, first, "Draft A")
    res = c.post(f"/api/studies/{study.id}/compare")
    assert res.status_code == 409 and "two finished runs" in res.json()["detail"]


# ---------------------------------------------------------------- CLI


def test_cli_cast_from_and_compare(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv(cli.FAKE_ENV, "1")
    out = tmp_path / "runs"
    cli_runner = CliRunner()
    args = ["run", str(EXAMPLE), "-q", QUESTION, "--agents", "4", "--rounds", "1", "--batch", "2", "--out", str(out)]
    assert cli_runner.invoke(cli.app, args).exit_code == 0
    (first,) = [p for p in out.iterdir()]
    result = cli_runner.invoke(cli.app, [*args, "--cast-from", str(first), "--variant", "Shorter"])
    assert result.exit_code == 0, result.output
    assert "tellfold compare" in result.output
    second = next(p for p in out.iterdir() if p.is_dir() and p not in (first, out / "studies"))
    config = SimConfig.model_validate_json((second / "config.json").read_text())
    assert (config.cast_from, config.variant) == (first.name, "Shorter")

    result = cli_runner.invoke(cli.app, ["compare", str(second)])
    assert result.exit_code == 0, result.output
    assert (out / "studies" / config.study_id / COMPARE_FILE).is_file()

    elsewhere = tmp_path / "other"
    result = cli_runner.invoke(cli.app, [*args[:-1], str(elsewhere), "--cast-from", str(first)])
    assert result.exit_code == 1 and "--out folder" in result.output
    assert cli_runner.invoke(cli.app, ["compare", str(first)]).exit_code == 0  # the source is in the study too
