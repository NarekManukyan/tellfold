"""Rewrite suggestion (2C): one report.rewrite call, cites audited, saved as rewrite.json. FakeLLM only."""

from __future__ import annotations

import time
from pathlib import Path

import pytest
from fastapi.testclient import TestClient
from typer.testing import CliRunner

from tellfold import cli, runner
from tellfold.llm import FakeLLM
from tellfold.models import Report, RewriteSuggestion, SimConfig
from tellfold.report import UNVERIFIED
from tellfold.rewrite import REWRITE_FILE, RewriteError, build_rewrite, editable_seed
from tellfold.server.app import create_app
from tellfold.world import World

EXAMPLE = Path(__file__).resolve().parent.parent / "examples" / "mossbook_pricing.md"
QUESTION = "Will buyers accept the subscription?"


async def _run(tmp_path: Path) -> tuple[Path, SimConfig]:
    run_dir = tmp_path / "runs" / "a"
    run_dir.mkdir(parents=True)
    seeds = runner.copy_seeds(run_dir, [EXAMPLE])
    config = SimConfig(question=QUESTION, seed_files=[str(s) for s in seeds], num_agents=4, rounds=2, batch_size=2)
    await runner.run_pipeline(FakeLLM(cli.fake_responder), run_dir, config, seeds)
    return run_dir, config


def test_editable_seed_prefers_the_pasted_description(tmp_path: Path):
    seeds = tmp_path / "seeds"
    seeds.mkdir()
    (seeds / "brief.pdf").write_bytes(b"%PDF")
    (seeds / "a_notes.txt").write_text("notes")
    assert editable_seed(tmp_path) == ("a_notes.txt", "notes")
    (seeds / "description.md").write_text("  The text.  ")
    assert editable_seed(tmp_path) == ("description.md", "The text.")
    (seeds / "description.md").write_text(" ")
    with pytest.raises(RewriteError, match="empty"):
        editable_seed(tmp_path)
    with pytest.raises(RewriteError, match="no text document"):
        editable_seed(tmp_path / "nowhere")


async def test_build_rewrite_audits_the_cites(tmp_path: Path):
    run_dir, config = await _run(tmp_path)
    report = Report.model_validate_json((run_dir / "report.json").read_text())
    world = World(run_dir / "world.db")
    posts = world.posts()
    handle = world.personas()[0].handle
    world.close()
    real = posts[0]
    seen: dict[str, str] = {}

    def respond(purpose, prompt, schema):
        seen[purpose] = prompt
        return {"revised_text": "Mossbook stays one-time for existing buyers.",
                "changes": [{"what": "Keep existing buyers", "why": f'@{handle} wrote "{real.content}" in #{real.id}.',
                             "cites": [real.id, 99999, real.id]},
                            {"what": "Clearer price", "why": "As #88888 said, it was vague.", "cites": [88888]},
                            {"what": " ", "why": "dropped", "cites": []}],
                "kept_on_purpose": ["The price itself, since @nobody_here liked it."]}

    s = await build_rewrite(FakeLLM(respond), run_dir, config, report, model="sonnet")
    prompt = seen["report.rewrite"]
    assert "ORIGINAL TEXT (mossbook_pricing.md)" in prompt and EXAMPLE.read_text()[:60] in prompt
    assert f"#{real.id}" in prompt and report.title in prompt
    assert s.source_name == "mossbook_pricing.md" and s.created_at
    assert [c.what for c in s.changes] == ["Keep existing buyers", "Clearer price"]
    assert s.changes[0].cites == [real.id] and s.changes[1].cites == []
    assert UNVERIFIED not in s.changes[0].why
    assert UNVERIFIED in s.changes[1].why and UNVERIFIED in s.kept_on_purpose[0]
    audit = s.citation_audit
    assert set(audit.unknown_posts) == {88888, 99999} and audit.unknown_handles == ["nobody_here"]
    assert audit.marked == 2 + 2  # two marked in the text, two unknown cites dropped


def test_server_rewrite_route(tmp_path: Path):
    runs = tmp_path / "runs"
    runs.mkdir()
    made: list[FakeLLM] = []

    def factory(run_dir, concurrency, think):
        made.append(FakeLLM(cli.fake_responder))
        return made[-1]

    seed = b"# Harbour Town ferry fare\n\nThe council proposes raising the ferry fare from 2 to 3 euros.\n"
    with TestClient(create_app(runs, factory, fake_llm=True, poll_interval=0.05)) as c:
        rid = c.post("/api/runs", data={"question": "Will they accept it?", "agents": "4", "rounds": "2"},
                     files=[("files[]", ("fare.md", seed, "text/markdown"))]).json()["id"]
        assert c.get(f"/api/runs/{rid}/rewrite").status_code == 404
        deadline = time.monotonic() + 30
        while c.get(f"/api/runs/{rid}").json()["status"]["state"] != "done" and time.monotonic() < deadline:
            time.sleep(0.03)
        res = c.post(f"/api/runs/{rid}/rewrite")
        assert res.status_code == 200, res.text
        body = res.json()
        assert body["source_name"] == "fare.md" and body["revised_text"] and "citation_audit" in body
        assert RewriteSuggestion.model_validate_json((runs / rid / REWRITE_FILE).read_text()).revised_text
        assert c.get(f"/api/runs/{rid}/rewrite").json() == body
        assert any(p == "report.rewrite" for llm in made for p, _, _ in llm.calls)

        (runs / rid / "seeds" / "fare.md").unlink()
        res = c.post(f"/api/runs/{rid}/rewrite")
        assert res.status_code == 400 and "no text document" in res.json()["detail"]
        (runs / rid / "report.json").unlink()
        assert c.post(f"/api/runs/{rid}/rewrite").status_code == 409


def test_server_rewrite_of_an_example_keeps_its_folder(tmp_path: Path):
    examples = Path(__file__).resolve().parent.parent / "mac" / "Tellfold" / "Resources" / "Examples"
    before = sorted(p.name for p in (examples / "corner-cafe").iterdir())
    app = create_app(tmp_path / "runs", lambda d, c, t: FakeLLM(cli.fake_responder), fake_llm=True,
                     examples_dir=examples)
    with TestClient(app) as c:
        res = c.post("/api/runs/example-corner-cafe/rewrite")
        assert res.status_code == 200, res.text
        assert c.get("/api/runs/example-corner-cafe/rewrite").json() == res.json()
    assert sorted(p.name for p in (examples / "corner-cafe").iterdir()) == before


async def test_a_text_too_long_to_rewrite_whole_is_refused(tmp_path: Path):
    run_dir, config = await _run(tmp_path)
    report = Report.model_validate_json((run_dir / "report.json").read_text())
    (run_dir / "seeds" / EXAMPLE.name).write_text("A long announcement. " * 700)  # 14,700 characters
    llm = FakeLLM(cli.fake_responder)
    with pytest.raises(RewriteError, match="too long to rewrite"):
        await build_rewrite(llm, run_dir, config, report, model="sonnet")
    assert llm.calls == []  # refused before any model call, rather than rewriting only the start


def test_cli_rewrite(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv(cli.FAKE_ENV, "1")
    out = tmp_path / "runs"
    args = ["run", str(EXAMPLE), "-q", QUESTION, "--agents", "4", "--rounds", "1", "--batch", "2", "--out", str(out)]
    assert CliRunner().invoke(cli.app, args).exit_code == 0
    (run_dir,) = list(out.iterdir())
    result = CliRunner().invoke(cli.app, ["rewrite", str(run_dir)])
    assert result.exit_code == 0, result.output
    assert (run_dir / REWRITE_FILE).is_file() and "--cast-from" in result.output
    revised = run_dir / "rewrite.md"
    suggestion = RewriteSuggestion.model_validate_json((run_dir / REWRITE_FILE).read_text())
    assert revised.read_text().strip() == suggestion.revised_text.strip()
    assert f"tellfold run {revised}" in " ".join(result.output.split())  # the file the command names exists
