"""CLI tests. Everything runs on the canned FakeLLM via SHOAL_FAKE_LLM=1; the real CLI is never spawned."""

from __future__ import annotations

import json
from datetime import datetime
from pathlib import Path
from typing import Literal

import pytest
import typer
from pydantic import BaseModel, Field
from typer.testing import CliRunner

from shoal import cli
from shoal.models import Action, KnowledgeGraph, Persona, Report, SimConfig

EXAMPLE = Path(__file__).resolve().parent.parent / "examples" / "mossbook_pricing.md"
runner = CliRunner()


@pytest.fixture(autouse=True)
def fake_llm(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv(cli.FAKE_ENV, "1")


@pytest.fixture(scope="module")
def fake_run(tmp_path_factory: pytest.TempPathFactory) -> Path:
    out = tmp_path_factory.mktemp("runs")
    with pytest.MonkeyPatch.context() as mp:
        mp.setenv(cli.FAKE_ENV, "1")
        result = runner.invoke(cli.app, [
            "run", str(EXAMPLE), "-q", "Will existing buyers accept the subscription?",
            "--agents", "6", "--rounds", "2", "--batch", "3", "--out", str(out),
            "--event", "2:A rival app announces it will stay one-time purchase",
        ])
    assert result.exit_code == 0, result.output
    (run_dir,) = list(out.iterdir())
    return run_dir


# ---------- helpers ----------


def test_parse_event() -> None:
    event = cli.parse_event("3: Breaking: price cut", rounds=5)
    assert (event.round, event.content) == (3, "Breaking: price cut")


@pytest.mark.parametrize("spec", ["no colon", "x:text", "3:", "9:too late", "0:too early"])
def test_parse_event_rejects_bad_input(spec: str) -> None:
    with pytest.raises(typer.BadParameter):
        cli.parse_event(spec, rounds=5)


def test_run_id_is_timestamped_slug() -> None:
    now = datetime(2026, 3, 3, 9, 5, 7)
    assert cli.make_run_id("Will users quit Mossbook?!", now) == "20260303-090507-will-users-quit-mossbook"
    assert cli.make_run_id("¿¡!?", now) == "20260303-090507-run"


def test_create_run_dir_never_reuses_a_folder(tmp_path: Path) -> None:
    first = cli.create_run_dir(tmp_path, "same question")
    second = cli.create_run_dir(tmp_path, "same question")
    assert first != second and first.is_dir() and second.is_dir()


def test_expand_seeds_reads_supported_files_from_folders(tmp_path: Path) -> None:
    for name in ("b.md", "a.txt", "c.pdf", "skip.png"):
        (tmp_path / name).write_text("x")
    assert [p.name for p in cli.expand_seeds([tmp_path])] == ["a.txt", "b.md", "c.pdf"]


# ---------- fake responder ----------


class _Decision(BaseModel):
    agent_id: str
    actions: list[Action] = Field(min_length=1, max_length=3)
    memory_note: str


class _Decisions(BaseModel):
    decisions: list[_Decision]


class _Scored(BaseModel):
    score: float = Field(ge=-1, le=1)
    label: Literal["calm", "angry"]
    counts: dict[str, int] = Field(default_factory=dict)
    note: str | None = None


@pytest.mark.parametrize("schema", [Report, KnowledgeGraph, Persona, SimConfig, _Scored])
def test_fake_responder_output_validates(schema: type[BaseModel]) -> None:
    schema.model_validate(cli.fake_responder("any.purpose", "prompt", schema))


def test_fake_responder_targets_agents_and_posts_from_prompt() -> None:
    prompt = "Agents in this batch: a02, a05.\nFeed for a02: [post #7] ... [post #9] ..."
    decisions = _Decisions.model_validate(cli.fake_responder("engine.round", prompt, _Decisions))
    assert [d.agent_id for d in decisions.decisions] == ["a02", "a05"]
    for d in decisions.decisions:
        assert all(a.agent_id == d.agent_id for a in d.actions)
        assert all(a.target_post_id in (None, 7, 9) for a in d.actions)


def test_fake_responder_relations_point_at_entities() -> None:
    graph = KnowledgeGraph.model_validate(cli.fake_responder("graph.extract", "chunks", KnowledgeGraph))
    known = {e.id for e in graph.entities} | {e.name for e in graph.entities}
    assert graph.relations and all({r.source, r.target} <= known for r in graph.relations)


def test_fake_responder_text() -> None:
    assert "chat.interview" in cli.fake_responder("chat.interview", "hi", None)


# ---------- commands ----------


def test_help_lists_every_command() -> None:
    result = runner.invoke(cli.app, ["--help"])
    assert result.exit_code == 0
    for command in ("run", "report", "chat", "ask", "agents", "doctor"):
        assert command in result.output


def test_run_rejects_bad_event(tmp_path: Path) -> None:
    result = runner.invoke(cli.app, ["run", str(EXAMPLE), "-q", "q", "--rounds", "2",
                                     "--event", "5:too late", "--out", str(tmp_path)])
    assert result.exit_code != 0
    assert not any(tmp_path.iterdir())


def test_full_fake_run_writes_every_artifact(fake_run: Path) -> None:
    for name in ("config.json", "world.db", "graph.json", "personas.json",
                 "report.json", "report.html", "report.md"):
        assert (fake_run / name).exists(), name
    config = SimConfig.model_validate_json((fake_run / "config.json").read_text())
    assert config.num_agents == 6 and config.rounds == 2 and config.events[0].round == 2
    Report.model_validate_json((fake_run / "report.json").read_text())
    assert isinstance(json.loads((fake_run / "personas.json").read_text()), list)
    assert "<html" in (fake_run / "report.html").read_text().lower()


def test_report_rebuilds_from_run(fake_run: Path) -> None:
    (fake_run / "report.html").unlink()
    result = runner.invoke(cli.app, ["report", str(fake_run)])
    assert result.exit_code == 0, result.output
    assert (fake_run / "report.html").exists()


def test_agents_table(fake_run: Path) -> None:
    result = runner.invoke(cli.app, ["agents", str(fake_run)])
    assert result.exit_code == 0, result.output
    assert "a01" in result.output


def test_ask(fake_run: Path) -> None:
    result = runner.invoke(cli.app, ["ask", str(fake_run), "Who changed their mind?"])
    assert result.exit_code == 0, result.output
    assert "offline test reply" in result.output


def test_chat_repl(fake_run: Path) -> None:
    agent_id = json.loads((fake_run / "personas.json").read_text())[0]["id"]
    result = runner.invoke(cli.app, ["chat", str(fake_run), "--agent", agent_id], input="Why?\n\nexit\n")
    assert result.exit_code == 0, result.output
    assert result.output.count("offline test reply") == 1


def test_chat_unknown_agent(fake_run: Path) -> None:
    result = runner.invoke(cli.app, ["chat", str(fake_run), "--agent", "zz99"])
    assert result.exit_code == 1
    assert "unknown agent" in result.output


def test_commands_reject_non_run_folder(tmp_path: Path) -> None:
    result = runner.invoke(cli.app, ["agents", str(tmp_path)])
    assert result.exit_code == 1
    assert "config.json" in result.output


def test_doctor_reports_missing_cli(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(cli.shutil, "which", lambda _: None)
    result = runner.invoke(cli.app, ["doctor"])
    assert result.exit_code == 1
    assert "not found" in result.output and "FAIL" in result.output
