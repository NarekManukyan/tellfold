"""Command-line interface: orchestrates a full run and exposes the inspection commands."""

from __future__ import annotations

import importlib
import itertools
import os
import re
import shlex
import shutil
import socket
import subprocess
import sys
import threading
import time
import webbrowser
from collections import Counter
from collections.abc import Callable
from functools import partial
from pathlib import Path
from typing import Annotated, Any, NoReturn

import anyio
import typer
from pydantic import BaseModel
from rich.console import Console, Group, RenderableType
from rich.live import Live
from rich.panel import Panel
from rich.spinner import Spinner
from rich.table import Table
from rich.text import Text

from tellfold import __version__
from tellfold.chat import ask_report, interview
from tellfold.llm import LLM, FakeLLM, LLMError
from tellfold.models import (
    GodEvent,
    Study,
    StudyRun,
    KnowledgeGraph,
    Persona,
    Report,
    RoundStats,
    RunStage,
    SimConfig,
)
from tellfold.report import build_report
from tellfold.runner import (  # noqa: F401 - re-exported: older callers and tests use cli.<name>
    RunError,
    copy_seeds,
    create_run_dir,
    estimate_calls,
    make_run_id,
    mark_stopped,
    run_pipeline,
    write_json,
    write_report,
)
from tellfold.agree import build_agreement
from tellfold.branch import prepare_branch
from tellfold.compare import COMPARE_FILE, Member, build_compare
from tellfold.rewrite import REWRITE_FILE, RewriteError, build_rewrite
from tellfold.runner import load_cast, now_iso
from tellfold.settings import env
from tellfold.study import (
    AGREEMENT_FILE,
    StudyError,
    join_study,
    letter,
    new_study_id,
    plan_variant,
    repeat_config,
    save_study,
    study_dir,
    study_of,
)
from tellfold.world import World

app = typer.Typer(
    name="tellfold",
    help="Seed documents in, a simulated social reaction and a prediction report out. "
    "Runs on your local Claude Code CLI.",
    no_args_is_help=True,
    add_completion=False,
)
console = Console()

SEED_SUFFIXES = {".pdf", ".md", ".markdown", ".txt"}
FAKE_ENV = "TELLFOLD_FAKE_LLM"
FAKE_DELAY_ENV = "TELLFOLD_FAKE_DELAY"

# `tellfold serve` hard-exits this long after stdin closes / after the server stops, if a stuck worker thread keeps the
# process alive (graceful shutdown itself is bounded by uvicorn's timeout_graceful_shutdown=3).
# Longer than the worst graceful shutdown (uvicorn's 3 s + the run manager's 5 s grace + 2 s to reap children),
# so the force-exit only fires when something is actually stuck.
_SERVE_EOF_EXIT_SECONDS = 15.0
_SERVE_SHUTDOWN_EXIT_SECONDS = 5.0


def _force_exit(kill_children: Callable[[], None]) -> None:
    """Kill every agent CLI process group still running (each is in its own session, so it would outlive us),
    then end the process without waiting for stuck threads."""
    try:
        kill_children()
    finally:
        sys.stderr.flush()
        os._exit(0)


def _exit_later(seconds: float) -> None:
    """Start a daemon timer that kills live agent CLIs and ends the process with os._exit(0) unless it has exited
    on its own by then."""
    # Imported now, not in the timer thread: a stuck main thread could be holding the import lock.
    from tellfold.runtimes.cli_tools import kill_live_process_groups

    timer = threading.Timer(seconds, _force_exit, args=(kill_live_process_groups,))
    timer.daemon = True
    timer.start()
EXIT_INTERRUPTED = 130


# ---------- LLM selection ----------


def fake_mode() -> bool:
    return (env(FAKE_ENV) or "").strip().lower() in {"1", "true", "yes", "on"}


def fake_delay() -> float:
    """Seconds each fake model call waits (TELLFOLD_FAKE_DELAY, 0-10, default 0), so a fake run can be watched live."""
    try:
        return min(10.0, max(0.0, float(env(FAKE_DELAY_ENV, "0") or 0)))
    except ValueError:
        return 0.0


def make_llm(run_dir: Path | None, concurrency: int, think: bool = False, config: SimConfig | None = None) -> LLM:
    """The run's model backend (`tellfold.runtimes.build_llm`), or a canned FakeLLM when TELLFOLD_FAKE_LLM=1.

    The agent / planner runtimes come from `config`, else from `run_dir/config.json`, else the settings."""
    if fake_mode():
        delay = fake_delay()
        if delay <= 0:
            return FakeLLM(fake_responder)

        async def slow_responder(purpose: str, prompt: str, schema: type[BaseModel] | None) -> str | dict[str, Any]:
            await anyio.sleep(delay)
            return fake_responder(purpose, prompt, schema)

        return FakeLLM(slow_responder)
    from tellfold.runtimes import UnknownRuntime, build_llm
    from tellfold.settings import load_settings

    log_path = run_dir / "calls.jsonl" if run_dir else None
    settings = load_settings()
    if config is None and run_dir is not None:
        try:
            config = SimConfig.model_validate_json((run_dir / "config.json").read_text(encoding="utf-8"))
        except (OSError, ValueError):
            config = None
    if config is None:
        config = SimConfig(question="", seed_files=[], agent_runtime=settings.agent_runtime,
                           planner_runtime=settings.planner_runtime)
    try:
        return build_llm(config, settings, concurrency=concurrency, log_path=log_path, think=think)
    except UnknownRuntime as e:
        raise LLMError(f"{e}. Run `tellfold runtimes` to see what is available.") from e


# ---------- fake responder (schema-driven, used only with TELLFOLD_FAKE_LLM=1) ----------

_AGENT_ID = re.compile(r"\ba\d{2,3}\b")
_POST_REF = re.compile(r"(?:#|\bpost[ _-]?(?:id)?\s*[:=#]?\s*)(\d+)", re.IGNORECASE)
_CHOICES = re.compile(r"\bone of:\s*([\w\s,|/-]+)", re.IGNORECASE)
_MAX_FAKE_ITEMS = 12


class _FakeBuilder:
    """Builds a deterministic instance of a JSON schema, steered by what the prompt mentions.

    Agent ids (a01, a02...) and post ids (#12, post 12) found in the prompt are reused so fake decisions
    point at real agents and posts; a field description like "one of: a, b, c" is honoured; relation
    endpoints reuse ids/names generated earlier in the same response. Everything else is placeholder text.
    """

    def __init__(self, purpose: str, prompt: str, schema: type[BaseModel]):
        self.purpose = purpose
        self.root = schema.model_json_schema()
        self.defs: dict[str, Any] = self.root.get("$defs", {})
        self.agents = list(dict.fromkeys(_AGENT_ID.findall(prompt)))
        self.post_ids = [int(p) for p in dict.fromkeys(_POST_REF.findall(prompt))]
        self.counter = itertools.count(1)
        self.refs: list[str] = []
        self.agent_cursor = 0

    def build(self) -> Any:
        return self.value(self.root, "", None, nullable=False)

    def resolve(self, node: dict[str, Any]) -> dict[str, Any]:
        while "$ref" in node:
            node = {**self.defs[node["$ref"].rsplit("/", 1)[-1]], **{k: v for k, v in node.items() if k != "$ref"}}
        if len(node.get("allOf", [])) == 1:
            return self.resolve(node["allOf"][0])
        return node

    def value(self, node: dict[str, Any], name: str, agent: str | None, *, nullable: bool) -> Any:
        node = self.resolve(node)
        if "const" in node:
            return node["const"]
        if "enum" in node:
            return node["enum"][next(self.counter) % len(node["enum"])]
        for key in ("anyOf", "oneOf"):
            if key in node:
                options = [o for o in node[key] if self.resolve(o).get("type") != "null"]
                if not options:
                    return None
                merged = {**options[0], **({"description": node["description"]} if "description" in node else {})}
                return self.value(merged, name, agent, nullable=len(options) < len(node[key]))
        kind = node.get("type")
        if isinstance(kind, list):
            nullable = nullable or "null" in kind
            kind = next((k for k in kind if k != "null"), "null")
        if kind == "object":
            return self.object(node, agent)
        if kind == "array":
            return self.array(node, name, agent)
        if kind == "string":
            return self.string(node, name, agent, nullable=nullable)
        if kind in ("integer", "number"):
            return self.number(node, name, integer=kind == "integer", nullable=nullable)
        if kind == "boolean":
            return True
        return None

    def object(self, node: dict[str, Any], agent: str | None) -> dict[str, Any]:
        props: dict[str, Any] = node.get("properties", {})
        if "agent_id" in props and agent is None:
            agent = self.take_agent()
        return {key: self.value(sub, key, agent, nullable=False) for key, sub in props.items()}

    def array(self, node: dict[str, Any], name: str, agent: str | None) -> list[Any]:
        items = self.resolve(node.get("items", {}))
        top_level_objects = items.get("type") == "object" and agent is None
        want = (len(self.agents) or 3) if top_level_objects else 2
        size = min(max(node.get("minItems", 0), want), node.get("maxItems", _MAX_FAKE_ITEMS))
        return [self.value(items, name, agent, nullable=False) for _ in range(size)]

    def take_agent(self) -> str:
        if not self.agents:
            return "a01"
        agent = self.agents[self.agent_cursor % len(self.agents)]
        self.agent_cursor += 1
        return agent

    def string(self, node: dict[str, Any], name: str, agent: str | None, *, nullable: bool) -> str | None:
        n = next(self.counter)
        key = name.lower()
        choices = _CHOICES.search(node.get("description", ""))
        if choices:
            options = [c.strip() for c in re.split(r"[,|/]", choices.group(1)) if c.strip()]
            return options[n % len(options)]
        if key == "agent_id":
            return agent or self.take_agent()
        if key.startswith("target_agent"):
            others = [a for a in self.agents if a != agent]
            return others[n % len(others)] if others else (None if nullable else "a01")
        if key in ("source", "target") and self.refs:
            return self.refs[n % len(self.refs)]
        if "handle" in key:
            text = f"fake_user_{n}"
        elif key in ("id", "slug") or key.endswith("_id"):
            if nullable:
                return None
            text = f"item-{n}"
            self.refs.append(text)
        elif "name" in key or key == "title":
            text = f"Placeholder {key or 'name'} {n}"
            self.refs.append(text)
        else:
            text = f"Canned {key or 'text'} #{n} from the offline test model ({self.purpose})."
        return self.fit(text, node)

    @staticmethod
    def fit(text: str, node: dict[str, Any]) -> str:
        text = text.ljust(node.get("minLength", 0), "x")
        return text[: node["maxLength"]] if "maxLength" in node else text

    def number(self, node: dict[str, Any], name: str, *, integer: bool, nullable: bool) -> float | int | None:
        n = next(self.counter)
        if integer and "post" in name.lower():
            if self.post_ids:
                return self.post_ids[n % len(self.post_ids)]
            if nullable:
                return None
        low = node.get("minimum", node.get("exclusiveMinimum"))
        high = node.get("maximum", node.get("exclusiveMaximum"))
        if low is not None and high is not None:
            value = low + (high - low) * 0.6
        elif low is not None:
            value = low + 1
        elif high is not None:
            value = min(high - 1, 0.5)
        else:
            value = 0.5
        return round(value) if integer else value


def fake_responder(purpose: str, prompt: str, schema: type[BaseModel] | None) -> str | dict[str, Any]:
    """Canned FakeLLM responder: schema-valid dicts for json(), short text for text()."""
    if schema is None:
        return f"(offline test reply for {purpose}) I would keep an eye on how people react."
    return _FakeBuilder(purpose, prompt, schema).build()


# ---------- small helpers ----------


def parse_event(spec: str, rounds: int) -> GodEvent:
    """Parse `ROUND:TEXT` into a GodEvent; rounds are numbered 1..rounds."""
    head, sep, text = spec.partition(":")
    if not sep or not head.strip().isdigit() or not text.strip():
        raise typer.BadParameter(f"expected ROUND:TEXT, got {spec!r}", param_hint="--event")
    number = int(head)
    if not 1 <= number <= rounds:
        raise typer.BadParameter(f"round {number} is outside 1..{rounds}", param_hint="--event")
    return GodEvent(round=number, content=text.strip())


def expand_seeds(paths: list[Path]) -> list[Path]:
    """Files are kept as given; directories contribute their supported files."""
    files: list[Path] = []
    for path in paths:
        if path.is_dir():
            files.extend(sorted(p for p in path.iterdir() if p.suffix.lower() in SEED_SUFFIXES))
        else:
            files.append(path)
    return files


def load_run(run_dir: Path) -> tuple[SimConfig, World]:
    config_path, db_path = run_dir / "config.json", run_dir / "world.db"
    for needed in (config_path, db_path):
        if not needed.exists():
            fail(f"{needed} not found. Is {run_dir} a Tellfold run directory?")
    config = SimConfig.model_validate_json(config_path.read_text(encoding="utf-8"))
    return config, World(db_path, seed=config.seed)


def load_graph(run_dir: Path) -> KnowledgeGraph:
    path = run_dir / "graph.json"
    if not path.exists():
        return KnowledgeGraph()
    return KnowledgeGraph.model_validate_json(path.read_text(encoding="utf-8"))


def fail(message: str, code: int = 1) -> NoReturn:
    console.print(f"[bold red]error:[/] {message}")
    raise typer.Exit(code)


def fmt_sentiment(value: float | None) -> Text:
    if value is None:
        return Text("-", style="dim")
    style = "green" if value > 0.1 else "red" if value < -0.1 else "yellow"
    return Text(f"{value:+.2f}", style=style)


def round_table(stats: list[RoundStats], total_rounds: int) -> Table:
    done = sum(1 for s in stats if s.round > 0)  # round 0 holds the opening posts
    table = Table(title=f"Rounds ({done}/{total_rounds})", title_justify="left", expand=False)
    for header, justify in [("Round", "right"), ("Active", "right"), ("Posts", "right"),
                            ("Action mix", "left"), ("Sentiment", "right"), ("Calls", "right"),
                            ("LLM s", "right")]:
        table.add_column(header, justify=justify)
    for s in stats:
        mix = ", ".join(f"{k} {v}" for k, v in sorted(s.actions.items(), key=lambda kv: -kv[1]) if v)
        table.add_row(str(s.round), str(s.active_agents), str(s.new_posts), mix or "-",
                      fmt_sentiment(s.sentiment), str(s.llm_calls), f"{s.llm_seconds:.0f}")
    return table


# ---------- live progress view ----------


class RunView:
    """Rich renderable for `tellfold run`: finished stages, the current stage, and the round table."""

    def __init__(self, total_rounds: int):
        self.total_rounds = total_rounds
        self.finished: list[tuple[str, float]] = []
        self.current: str | None = None
        self.started = 0.0
        self.detail = ""
        self.notes: list[str] = []
        self.stats: list[RoundStats] = []

    def begin(self, label: str) -> None:
        """Close the current stage (if any) and start a new one."""
        if self.current == label:
            return
        self.end()
        self.current, self.detail, self.started = label, "", time.monotonic()

    def end(self) -> None:
        if self.current:
            self.finished.append((self.current, time.monotonic() - self.started))
        self.current = None

    def set_detail(self, text: str) -> None:
        self.detail = text

    def note(self, text: str) -> None:
        self.notes = [*self.notes[-3:], text]

    def render(self) -> RenderableType:
        lines: list[RenderableType] = [
            Text.assemble(("  ✓ ", "green"), label, (f"  {secs:.1f}s", "dim"))
            for label, secs in list(self.finished)
        ]
        if self.current:
            detail = f"  {self.detail}" if self.detail else ""
            lines.append(Spinner("dots", Text.assemble(self.current, (detail, "dim")), style="cyan"))
        lines.extend(Text(f"    {n}", style="dim") for n in list(self.notes))
        if self.stats:
            lines.append(round_table(list(self.stats), self.total_rounds))
        return Group(*lines)


def stage_labels(config: SimConfig) -> dict[str, str]:
    return {
        RunStage.READING.value: "Reading seed documents",
        RunStage.GRAPH.value: "Extracting knowledge graph",
        RunStage.PERSONAS.value: f"Creating {config.num_agents} personas",
        RunStage.SEEDING.value: "Seeding follows and opening posts",
        RunStage.ROUNDS.value: f"Simulating {config.rounds} rounds",
        RunStage.REPORT.value: "Writing the report",
    }


def pipeline_listener(view: RunView, config: SimConfig):
    """Adapts run_pipeline's on_event stream to the live view."""
    labels = stage_labels(config)

    def on_event(name: str, data: dict[str, Any]) -> None:
        if name == "stage":
            stage = str(data.get("stage", ""))
            if stage in labels:
                view.begin(labels[stage])
            view.set_detail(str(data.get("detail", "")))
            if "estimated_calls" in data:
                view.note(f"{data.get('chunks')} chunks; about {data['estimated_calls']} model calls this run "
                          "if none need a retry")
            if "entities" in data:
                view.note(f"graph: {data['entities']} entities, {data.get('relations', 0)} relations")
            return
        if name == "round_done":
            stats = RoundStats.model_validate({k: data[k] for k in RoundStats.model_fields if k in data})
            view.stats = [*(s for s in view.stats if s.round != stats.round), stats]
            if stats.round > 0:
                view.set_detail(f"round {stats.round} of {config.rounds} done")
            return
        if name in ("run_done", "run_stopped", "run_failed"):
            view.end()
            return
        if name in ("status", "posts_added"):
            return
        if name == "event_injected":
            view.note(f"round {data.get('round')}: breaking news injected")
            return
        scalars = ", ".join(f"{k}={v}" for k, v in data.items() if isinstance(v, (int, float, str)))[:80]
        message = f"{name.replace('_', ' ')}{': ' + scalars if scalars else ''}"
        if "fail" in name or "error" in name:
            view.note(f"warning: {message}")
        else:
            view.set_detail(message)

    return on_event


def print_summary(report: Report, run_dir: Path) -> None:
    body = Text.assemble(
        (report.prediction, ""), "\n\n", ("confidence: ", "dim"), (report.confidence, "bold"),
    )
    console.print()
    console.print(Panel(body, title=report.title, title_align="left", border_style="cyan"))
    console.print(f"Report: [bold]{run_dir / 'report.html'}[/]  (also report.md, report.json)", soft_wrap=True)
    console.print(f"Next:   tellfold chat {run_dir} --agent a01", soft_wrap=True)
    console.print(f"        tellfold ask {run_dir} \"...\"", soft_wrap=True)


# ---------- commands ----------


def _version(value: bool) -> None:
    if value:
        console.print(f"tellfold {__version__}")
        raise typer.Exit()


@app.callback()
def main(
    version: Annotated[bool, typer.Option("--version", callback=_version, is_eager=True,
                                          help="Show the version and exit.")] = False,
) -> None:
    """Tellfold: simulate how a crowd reacts to your documents, then read the forecast."""


@app.command()
def run(
    seeds: Annotated[list[Path], typer.Argument(exists=True, help="Seed files (.pdf .md .txt) or folders.")],
    question: Annotated[str, typer.Option("--question", "-q", help="What you want predicted.")],
    agents: Annotated[int, typer.Option(min=2, max=200, help="Number of simulated people.")] = 20,
    rounds: Annotated[int, typer.Option(min=1, max=50, help="Simulation rounds.")] = 6,
    batch: Annotated[int, typer.Option(min=1, max=20, help="Agents decided per model call.")] = 6,
    agent_model: Annotated[str, typer.Option(help="Model for per-round agent decisions.")] = "haiku",
    planner_model: Annotated[str, typer.Option(help="Model for graph, personas and report.")] = "sonnet",
    concurrency: Annotated[int, typer.Option(min=1, max=16, help="Parallel Claude CLI calls.")] = 4,
    event: Annotated[list[str] | None, typer.Option(
        "--event", help="Breaking news injected at a round, as ROUND:TEXT. Repeatable.")] = None,
    out: Annotated[Path, typer.Option(help="Folder that holds run directories.")] = Path("runs"),
    language: Annotated[str, typer.Option(help="Language agents post and the report is written in.")] = "English",
    seed: Annotated[int, typer.Option(help="RNG seed for activity sampling and feed tie-breaks.")] = 7,
    think: Annotated[bool, typer.Option(
        "--think", help="Extended thinking for graph, personas and report: more thorough, several times slower.")] = False,
    open_report: Annotated[bool, typer.Option("--open", help="Open report.html when done.")] = False,
    agent_runtime: Annotated[str | None, typer.Option(
        help="Runtime for agent decisions and interviews (see `tellfold runtimes`). Default: settings, "
             "else claude-code.")] = None,
    planner_runtime: Annotated[str | None, typer.Option(
        help="Runtime for graph, personas and report (see `tellfold runtimes`). Default: settings, "
             "else claude-code.")] = None,
    cast_from: Annotated[Path | None, typer.Option(
        "--cast-from", exists=True, file_okay=False,
        help="Reuse the topic map and people of this run (in the --out folder) and compare the two: no graph or "
             "persona calls. Both runs join a study; see `tellfold compare`.")] = None,
    variant: Annotated[str | None, typer.Option(
        help="Label of this run in its study, e.g. \"Draft B\". Default: the next letter.")] = None,
) -> None:
    """Run a full simulation: ingest, graph, personas, rounds, report."""
    files = expand_seeds(seeds)
    if not files:
        fail("no .pdf, .md, .markdown or .txt files found in the given seeds")
    agent_runtime, planner_runtime = resolve_runtimes(agent_runtime, planner_runtime)
    config = SimConfig(
        question=question, seed_files=[str(f) for f in files], num_agents=agents, rounds=rounds,
        batch_size=batch, agent_model=agent_model, planner_model=planner_model, concurrency=concurrency,
        language=language, events=[parse_event(e, rounds) for e in event or []], seed=seed, think=think,
        agent_runtime=agent_runtime, planner_runtime=planner_runtime,
    )
    study = None
    if cast_from is not None:
        if cast_from.resolve().parent != out.resolve():
            fail(f"--cast-from must be a run in the --out folder ({out})")
        try:
            source = SimConfig.model_validate_json((cast_from / "config.json").read_text(encoding="utf-8"))
            cast = load_cast(cast_from)
            study, label = plan_variant(out, cast_from.name, source, variant=variant, now=now_iso())
        except (OSError, ValueError) as e:
            fail(f"cannot reuse {cast_from}: {e}")
        except (RunError, StudyError) as e:
            fail(str(e))
        config = config.model_copy(update={"cast_from": cast_from.name, "study_id": study.id, "variant": label,
                                           "num_agents": len(cast.personas)})
    elif variant:
        fail("--variant needs --cast-from: a study compares runs of one crowd")
    run_dir = create_run_dir(out, question)
    if study is not None:
        join_study(out, study, run_dir.name, config.variant or "")
    files = copy_seeds(run_dir, files)
    config = config.model_copy(update={"seed_files": [str(f) for f in files]})
    write_json(run_dir / "config.json", config)
    console.print(f"[bold]Tellfold[/] run [cyan]{run_dir}[/]" + ("  [yellow](fake LLM)[/]" if fake_mode() else ""),
                  soft_wrap=True)

    view = RunView(rounds)
    try:
        llm = make_llm(run_dir, concurrency, think=think, config=config)
    except LLMError as e:
        fail(str(e))
    try:
        with Live(get_renderable=view.render, console=console, refresh_per_second=8):
            report = anyio.run(partial(run_pipeline, llm, run_dir, config, files,
                                       on_event=pipeline_listener(view, config)))
    except KeyboardInterrupt:
        mark_stopped(run_dir)
        console.print(f"\n[yellow]Interrupted.[/] Everything written so far is in {run_dir}.\n"
                      f"If the simulation had started, rebuild the report with: tellfold report {run_dir}",
                      soft_wrap=True)
        raise typer.Exit(EXIT_INTERRUPTED)
    except (LLMError, RunError) as e:
        console.line()
        fail(f"{e}\nPartial results are in {run_dir}. Check `tellfold doctor` if the CLI itself failed.")

    if report is None:
        console.print(f"[yellow]Stopped[/] before any round finished. Partial results are in {run_dir}.",
                      soft_wrap=True)
        raise typer.Exit(EXIT_INTERRUPTED)
    print_summary(report, run_dir)
    if study is not None:
        console.print(f"Study:  {study.id}. Compare the versions with: tellfold compare {run_dir}", soft_wrap=True)
    if open_report:
        webbrowser.open((run_dir / "report.html").resolve().as_uri())


@app.command()
def compare(
    run_dir: Annotated[Path, typer.Argument(exists=True, file_okay=False, help="A run that belongs to a study.")],
    model: Annotated[str | None, typer.Option(help="Override the planner model.")] = None,
) -> None:
    """Compare the runs of a study (made with --cast-from) and write studies/<id>/compare.json."""
    config, world = load_run(run_dir)
    world.close()
    runs_dir = run_dir.resolve().parent
    study = study_of(runs_dir, run_dir.resolve().name, config)
    if study is None:
        fail(f"{run_dir} is not part of a study. Make one with: tellfold run SEED -q ... --cast-from {run_dir}")
    members = [Member(letter=letter(i), run_id=r.id, variant=r.variant, run_dir=runs_dir / r.id)
               for i, r in enumerate(study.runs) if (runs_dir / r.id / "report.json").is_file()]
    if len(members) < 2:
        fail("a comparison needs at least two runs of the study with a report")
    folder = study_dir(runs_dir, study.id)
    llm = make_llm(run_dir, 1, config=config)
    try:
        with console.status("Comparing the versions..."):
            result = anyio.run(partial(build_compare, llm, study.id, members, question=config.question,
                                       model=model or config.planner_model, language=config.language))
    except KeyboardInterrupt:
        console.print("\n[yellow]Interrupted.[/] Nothing was written.")
        raise typer.Exit(EXIT_INTERRUPTED)
    except LLMError as e:
        fail(str(e))
    folder.mkdir(parents=True, exist_ok=True)
    write_json(folder / COMPARE_FILE, result)
    verdict = result.report.better_variant or "a tie"
    console.print(Panel(Text.assemble((result.report.why, ""), "\n\n", ("better: ", "dim"), (verdict, "bold")),
                        title=result.report.headline, title_align="left", border_style="cyan"))
    console.print(f"Comparison: [bold]{folder / COMPARE_FILE}[/]", soft_wrap=True)


@app.command()
def check(
    run_dir: Annotated[Path, typer.Argument(exists=True, file_okay=False, help="A finished run with a report.")],
    runs: Annotated[int, typer.Option(min=2, max=5, help="Runs in all, this one included.")] = 3,
    agent_model: Annotated[str | None, typer.Option(help="Agent model for the new runs (default: the run's).")] = None,
) -> None:
    """Check it: run RUN_DIR again with the same people and text but other seeds, then say what held."""
    config, world = load_run(run_dir)
    world.close()
    if not (run_dir / "report.json").is_file():
        fail(f"{run_dir} has no report yet. Build it first with: tellfold report {run_dir}")
    out = run_dir.resolve().parent
    seeds = sorted(p for p in (run_dir / "seeds").glob("*") if p.is_file() and not p.name.startswith("."))
    if not seeds:
        fail(f"{run_dir} has no saved seed documents to run again")
    study = Study(id=new_study_id(), kind="repeat", created_at=now_iso(), planned=runs,
                  runs=[StudyRun(id=run_dir.resolve().name, variant="Run 1")])
    save_study(out, study)
    console.print(f"[bold]Check it[/]: {runs - 1} more run{'s' if runs > 2 else ''} of {run_dir.name}, one after "
                  "another" + ("  [yellow](fake LLM)[/]" if fake_mode() else ""), soft_wrap=True)
    for i in range(1, runs):
        new_dir = create_run_dir(out, config.question)
        files = copy_seeds(new_dir, seeds)
        new = repeat_config(config, run_dir.resolve().name, i, seed_files=files, study_id=study.id,
                            agent_model=agent_model)
        variant = new.variant or ""
        study = join_study(out, study, new_dir.name, variant)
        write_json(new_dir / "config.json", new)
        try:
            llm = make_llm(new_dir, new.concurrency, think=new.think, config=new)
            with console.status(f"{variant} of {runs} (seed {new.seed})..."):
                done = anyio.run(partial(run_pipeline, llm, new_dir, new, files))
        except KeyboardInterrupt:
            mark_stopped(new_dir)
            console.print(f"\n[yellow]Interrupted.[/] The check stopped at {variant}.")
            raise typer.Exit(EXIT_INTERRUPTED)
        except (LLMError, RunError) as e:
            fail(f"{variant} failed: {e}")
        if done is None:
            fail(f"{variant} stopped before a round finished")
        console.print(f"{variant}: {done.title}", soft_wrap=True)
    members = [Member(letter=letter(i), run_id=r.id, variant=r.variant, run_dir=out / r.id)
               for i, r in enumerate(study.runs)]
    try:
        with console.status("Checking what held..."):
            agreement = anyio.run(partial(build_agreement, make_llm(run_dir, 1, config=config), study.id, members,
                                          question=config.question, model=config.planner_model,
                                          language=config.language))
    except LLMError as e:
        fail(str(e))
    folder = study_dir(out, study.id)
    write_json(folder / AGREEMENT_FILE, agreement)
    body = Text.assemble((agreement.summary, ""), "\n\n", ("held: ", "dim"), ("; ".join(agreement.held) or "-", ""),
                         "\n", ("split: ", "dim"), ("; ".join(agreement.split) or "-", ""))
    console.print(Panel(body, title=f"{agreement.verdict.capitalize()} across {agreement.runs} runs",
                        title_align="left", border_style="cyan"))
    console.print(f"Check: [bold]{folder / AGREEMENT_FILE}[/]", soft_wrap=True)


@app.command()
def branch(
    run_dir: Annotated[Path, typer.Argument(exists=True, file_okay=False, help="The run to branch.")],
    at: Annotated[int, typer.Option("--at", min=0, help="Keep rounds 0 to AT exactly as they happened.")],
    event: Annotated[list[str] | None, typer.Option(
        "--event", help="Something that happens after AT, as ROUND:TEXT. Repeatable.")] = None,
    drop_events: Annotated[bool, typer.Option(
        "--drop-events", help="Drop the run's own events after AT (by default they still happen).")] = False,
    open_report: Annotated[bool, typer.Option("--open", help="Open report.html when done.")] = False,
) -> None:
    """What if...? Copy a run up to round AT, change what happens next, and continue it as a new run."""
    config, world = load_run(run_dir)
    try:
        happened = world.events()  # includes events added while the run was going, which config.json lacks
    finally:
        world.close()
    new_events = [parse_event(e, config.rounds) for e in event or []]
    seen = {(e.round, e.content) for e in happened}
    planned = happened + [e for e in config.events if (e.round, e.content) not in seen]  # not reached yet
    later = [] if drop_events else [e for e in planned if e.round > at]
    out = run_dir.resolve().parent
    try:
        study, label = plan_variant(out, run_dir.resolve().name, config, variant=f"Branch at round {at}",
                                    now=now_iso(), source_label="Original")
    except StudyError as e:
        fail(str(e))
    new_dir = create_run_dir(out, config.question)
    try:
        new = prepare_branch(run_dir.resolve(), new_dir, config, at, later + new_events, study_id=study.id,
                             variant=label)
    except RunError as e:
        shutil.rmtree(new_dir, ignore_errors=True)
        fail(str(e))
    join_study(out, study, new_dir.name, label)
    console.print(f"[bold]Tellfold[/] branch [cyan]{new_dir}[/] of {run_dir.name} after round {at}"
                  + ("  [yellow](fake LLM)[/]" if fake_mode() else ""), soft_wrap=True)
    view = RunView(new.rounds)
    seeds = [Path(p) for p in new.seed_files if Path(p).is_file()]
    try:
        llm = make_llm(new_dir, new.concurrency, think=new.think, config=new)
        with Live(get_renderable=view.render, console=console, refresh_per_second=8):
            built = anyio.run(partial(run_pipeline, llm, new_dir, new, seeds, on_event=pipeline_listener(view, new)))
    except KeyboardInterrupt:
        mark_stopped(new_dir)
        console.print(f"\n[yellow]Interrupted.[/] Everything written so far is in {new_dir}.", soft_wrap=True)
        raise typer.Exit(EXIT_INTERRUPTED)
    except (LLMError, RunError) as e:
        console.line()
        fail(f"{e}\nPartial results are in {new_dir}.")
    if built is None:
        fail("the branch stopped before a round finished")
    print_summary(built, new_dir)
    console.print(f"Study:  {study.id}. Compare it with the original: tellfold compare {new_dir}", soft_wrap=True)
    if open_report:
        webbrowser.open((new_dir / "report.html").resolve().as_uri())


@app.command()
def rewrite(
    run_dir: Annotated[Path, typer.Argument(exists=True, file_okay=False, help="A run with a report.")],
    model: Annotated[str | None, typer.Option(help="Override the planner model.")] = None,
) -> None:
    """Suggest a better version of the run's text (rewrite.json). Test it with: tellfold run --cast-from RUN_DIR."""
    config, world = load_run(run_dir)
    world.close()
    report_path = run_dir / "report.json"
    if not report_path.exists():
        fail(f"{report_path} not found. Build it first with: tellfold report {run_dir}")
    built = Report.model_validate_json(report_path.read_text(encoding="utf-8"))
    llm = make_llm(run_dir, 1, config=config)
    try:
        with console.status("Writing a better version..."):
            suggestion = anyio.run(partial(build_rewrite, llm, run_dir, config, built,
                                           model=model or config.planner_model))
    except KeyboardInterrupt:
        console.print("\n[yellow]Interrupted.[/] Nothing was written.")
        raise typer.Exit(EXIT_INTERRUPTED)
    except RewriteError as e:
        fail(str(e))
    except LLMError as e:
        fail(str(e))
    write_json(run_dir / REWRITE_FILE, suggestion)
    revised = run_dir / ("rewrite" + (Path(suggestion.source_name).suffix or ".md"))
    revised.write_text(suggestion.revised_text.rstrip() + "\n", encoding="utf-8")
    others = sorted(p for p in (run_dir / "seeds").glob("*")
                    if p.is_file() and not p.name.startswith(".") and p.name != suggestion.source_name)
    console.print(Panel(suggestion.revised_text, title=f"Suggested {suggestion.source_name}", title_align="left",
                        border_style="cyan"))
    for change in suggestion.changes:
        cites = ", ".join(f"#{pid}" for pid in change.cites)
        console.print(f"[bold]{change.what}[/] {change.why}" + (f" [dim]({cites})[/]" if cites else ""), soft_wrap=True)
    for note in suggestion.kept_on_purpose:
        console.print(f"[dim]kept:[/] {note}", soft_wrap=True)
    files = " ".join(shlex.quote(str(p)) for p in [revised, *others])
    console.print(f"Saved: [bold]{revised}[/] (and {REWRITE_FILE}). Test it against the same people with:\n"
                  f"  tellfold run {files} -q {shlex.quote(config.question)} --cast-from {shlex.quote(str(run_dir))}",
                  soft_wrap=True)


@app.command()
def report(
    run_dir: Annotated[Path, typer.Argument(exists=True, file_okay=False, help="A run directory.")],
    model: Annotated[str | None, typer.Option(help="Override the planner model.")] = None,
    open_report: Annotated[bool, typer.Option("--open", help="Open report.html when done.")] = False,
) -> None:
    """Rebuild report.json / .html / .md from an existing (possibly interrupted) run."""
    config, world = load_run(run_dir)
    llm = make_llm(run_dir, config.concurrency, think=config.think)
    try:
        with console.status("Writing the report..."):
            built = anyio.run(partial(build_report, llm, world, load_graph(run_dir), config,
                                      model=model or config.planner_model))
    except KeyboardInterrupt:
        console.print("\n[yellow]Interrupted.[/] The report was not rewritten.")
        raise typer.Exit(EXIT_INTERRUPTED)
    except LLMError as e:
        fail(str(e))
    html_path = write_report(run_dir, built, world, config)
    print_summary(built, run_dir)
    if open_report:
        webbrowser.open(html_path.resolve().as_uri())


@app.command()
def chat(
    run_dir: Annotated[Path, typer.Argument(exists=True, file_okay=False, help="A run directory.")],
    agent: Annotated[str, typer.Option("--agent", "-a", help="Agent id (a03) or handle to interview.")],
    model: Annotated[str | None, typer.Option(help="Override the model (default: the run's agent model).")] = None,
) -> None:
    """Interview one simulated agent, in character. Type 'exit' to leave."""
    config, world = load_run(run_dir)
    persona = find_persona(world, agent)
    console.print(Panel(f"[bold]@{persona.handle}[/] {persona.name}\n{persona.bio}",
                        title=f"Interviewing {persona.id}", title_align="left", border_style="cyan"))
    llm = make_llm(run_dir, 1)
    history: list[tuple[str, str]] = []
    while True:
        try:
            question = console.input("[bold cyan]you>[/] ").strip()
        except (EOFError, KeyboardInterrupt):
            break
        if question.lower() in {"exit", "quit", ":q"}:
            break
        if not question:
            continue
        try:
            answer = anyio.run(partial(interview, llm, world, persona.id, question,
                                       model=model or config.agent_model, history=list(history)))
        except KeyboardInterrupt:
            break
        except LLMError as e:
            console.print(f"[red]{e}[/]")
            continue
        history.append((question, answer))
        console.print(f"[bold magenta]@{persona.handle}>[/] {answer}\n")


def find_persona(world: World, agent: str) -> Persona:
    """Look an agent up by id (a03) or handle (@maria_dev)."""
    personas = world.personas()
    wanted = agent.removeprefix("@").lower()
    for persona in personas:
        if wanted in (persona.id.lower(), persona.handle.lower()):
            return persona
    fail(f"unknown agent {agent!r}. Known: {', '.join(p.id for p in personas) or 'none'}")


@app.command()
def ask(
    run_dir: Annotated[Path, typer.Argument(exists=True, file_okay=False, help="A run directory.")],
    question: Annotated[str, typer.Argument(help="Your question about the outcome.")],
    model: Annotated[str | None, typer.Option(help="Override the planner model.")] = None,
) -> None:
    """Ask the report agent a one-shot question about the simulation."""
    config, world = load_run(run_dir)
    report_path = run_dir / "report.json"
    if not report_path.exists():
        fail(f"{report_path} not found. Build it first with: tellfold report {run_dir}")
    built = Report.model_validate_json(report_path.read_text(encoding="utf-8"))
    llm = make_llm(run_dir, 1)
    try:
        with console.status("Thinking..."):
            answer = anyio.run(partial(ask_report, llm, world, built, question,
                                       model=model or config.planner_model))
    except KeyboardInterrupt:
        raise typer.Exit(EXIT_INTERRUPTED)
    except LLMError as e:
        fail(str(e))
    console.print(answer)


@app.command(name="agents")
def list_agents(
    run_dir: Annotated[Path, typer.Argument(exists=True, file_okay=False, help="A run directory.")],
) -> None:
    """Show the simulated personas with their reach and activity."""
    _, world = load_run(run_dir)
    post_counts = Counter(p.author_id for p in world.posts())
    table = Table(title=f"Agents in {run_dir.name}", title_justify="left")
    for header in ("ID", "Handle", "Name", "Followers", "Posts", "Activity", "Influence", "Starting stance"):
        table.add_column(header, overflow="fold")
    for p in world.personas():
        table.add_row(p.id, f"@{p.handle}", p.name, str(len(world.followers(p.id))), str(post_counts[p.id]),
                      f"{p.activity:.2f}", f"{p.influence:.2f}", p.stance[:80])
    console.print(table)


@app.command()
def ui(
    runs: Annotated[Path, typer.Option(help="Folder that holds run directories.")] = Path("runs"),
    port: Annotated[int, typer.Option(min=1, max=65535, help="Port to serve Tellfold Studio on (127.0.0.1 only).")] = 8765,
    open_browser: Annotated[bool, typer.Option("--open/--no-open", help="Open Studio in the browser.")] = True,
) -> None:
    """Start Tellfold Studio, the local web UI for running, watching and replaying simulations."""
    if not port_free(port):
        fail(f"port {port} on 127.0.0.1 is already in use. Pick another with --port.")
    try:
        import uvicorn

        from tellfold.server.app import create_app
    except ImportError as e:
        fail(f"Tellfold Studio could not load: {e}. Run `uv sync`.")
    runs.mkdir(parents=True, exist_ok=True)

    def llm_factory(run_dir: Path | None, concurrency: int, think: bool = False) -> LLM:
        return make_llm(run_dir, concurrency, think)

    application = create_app(runs.resolve(), llm_factory)
    url = f"http://127.0.0.1:{port}/"
    console.print(f"[bold]Tellfold Studio[/] on [cyan]{url}[/]  runs: {runs.resolve()}"
                  + ("  [yellow](fake LLM)[/]" if fake_mode() else ""), soft_wrap=True)
    console.print("[dim]Press Ctrl-C to quit.[/]")
    if open_browser:
        timer = threading.Timer(1.0, webbrowser.open, args=(url,))
        timer.daemon = True
        timer.start()
    uvicorn.run(application, host="127.0.0.1", port=port, log_level="warning",
                timeout_graceful_shutdown=3)


def port_free(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        try:
            sock.bind(("127.0.0.1", port))
        except OSError:
            return False
    return True


@app.command()
def doctor() -> None:
    """Check the Claude Code CLI, the SDK, and that a tiny haiku call works."""
    rows: list[tuple[str, bool, str]] = []
    cli_path = shutil.which("claude")
    rows.append(("Claude Code CLI on PATH", cli_path is not None,
                 f"{cli_path} ({cli_version(cli_path)})" if cli_path
                 else "not found: install Claude Code and make sure `claude` is on PATH"))
    sdk_ok, sdk_detail = sdk_status()
    rows.append(("claude-agent-sdk import", sdk_ok, sdk_detail))
    if fake_mode() or (cli_path and sdk_ok):
        rows.append(ping_model())
    else:
        rows.append(("Logged in (haiku call)", False, "skipped: fix the checks above first"))
    rows.append(("Python", sys.version_info >= (3, 11), sys.version.split()[0]))

    table = Table(title="tellfold doctor", title_justify="left")
    for header in ("Check", "Status", "Detail"):
        table.add_column(header, overflow="fold")
    for name, ok, detail in rows:
        table.add_row(name, Text("ok", style="green") if ok else Text("FAIL", style="bold red"), detail)
    console.print(table)
    if not all(ok for _, ok, _ in rows):
        raise typer.Exit(1)


def cli_version(path: str) -> str:
    try:
        done = subprocess.run([path, "--version"], capture_output=True, text=True, timeout=20, check=False)
    except (OSError, subprocess.TimeoutExpired) as e:
        return f"version check failed: {e}"
    return (done.stdout or done.stderr).strip() or "unknown version"


def sdk_status() -> tuple[bool, str]:
    try:
        sdk = importlib.import_module("claude_agent_sdk")
    except ImportError as e:
        return False, f"{e}. Run `uv sync`."
    return True, f"version {getattr(sdk, '__version__', 'unknown')}"


def ping_model() -> tuple[str, bool, str]:
    label = "Logged in (haiku call)" + (" [fake LLM]" if fake_mode() else "")
    llm = make_llm(None, 1)
    started = time.monotonic()
    try:
        reply = anyio.run(partial(llm.text, "Reply with the single word: ok",
                                  system="You are a connectivity check. Answer with one word.",
                                  model="haiku", purpose="doctor.ping"))
    except Exception as e:  # noqa: BLE001 - doctor reports any failure instead of crashing
        return label, False, f"{e}. Run `claude` once and log in with /login."
    return label, bool(reply.strip()), f"replied {reply.strip()[:40]!r} in {time.monotonic() - started:.1f}s"


# ---------- agent runtimes ----------


def resolve_runtimes(agent: str | None, planner: str | None) -> tuple[str, str]:
    """Explicit ids, else the settings defaults; fails with a helpful message for an unknown id."""
    from tellfold.runtimes import known_ids
    from tellfold.settings import load_settings

    settings = load_settings()
    known = known_ids(settings)
    chosen = (agent or settings.agent_runtime, planner or settings.planner_runtime)
    for flag, value in zip(("--agent-runtime", "--planner-runtime"), chosen):
        if value not in known:
            fail(f"unknown runtime {value!r} for {flag}. Known: {', '.join(known)}")
    return chosen


@app.command(name="runtimes")
def list_runtimes(
    check_auth: Annotated[bool, typer.Option("--check-auth", help="Also check logins (slower; may call the "
                                                                  "tools or their APIs).")] = False,
    as_json: Annotated[bool, typer.Option("--json", help="Print JSON instead of a table.")] = False,
) -> None:
    """Show the LLM runtimes detected on this machine (CLIs, Ollama, OpenAI-compatible servers, APIs)."""
    import json

    from tellfold.runtimes import detect_all
    from tellfold.settings import load_settings

    infos = detect_all(load_settings(), check_auth=check_auth)
    if as_json:
        sys.stdout.write(json.dumps([i.model_dump() for i in infos], indent=2) + "\n")
        return
    table = Table(title="Agent runtimes", title_justify="left")
    for header in ("ID", "Installed", "Version", "Auth", "Tested", "Agent / planner model"):
        table.add_column(header, overflow="fold")
    for info in infos:
        auth_style = {"ok": "green", "missing": "red"}.get(info.auth, "dim")
        defaults = " / ".join(v or "-" for v in info.default_models.values())
        table.add_row(info.id, Text("yes", style="green") if info.installed else Text("no", style="dim"),
                      info.version or "-", Text(info.auth, style=auth_style), "yes" if info.tested else "no",
                      defaults or "-")
    console.print(table)
    missing = [i for i in infos if i.installed and i.auth == "missing" and i.auth_hint]
    for info in missing:
        console.print(f"[yellow]{info.id}[/]: {info.auth_hint}", soft_wrap=True)
    if not check_auth:
        console.print("[dim]Add --check-auth to check logins.[/]")


def _stderr_log_config(level: str) -> dict[str, Any]:
    level = level.upper()
    return {
        "version": 1,
        "disable_existing_loggers": False,
        "formatters": {"plain": {"format": "%(asctime)s %(levelname)s %(name)s: %(message)s"}},
        "handlers": {"stderr": {"class": "logging.StreamHandler", "stream": "ext://sys.stderr",
                                "formatter": "plain"}},
        "root": {"handlers": ["stderr"], "level": level},
        "loggers": {name: {"handlers": ["stderr"], "level": level, "propagate": False}
                    for name in ("uvicorn", "uvicorn.error", "uvicorn.access")},
    }


@app.command()
def serve(
    token: Annotated[str, typer.Option(envvar=["TELLFOLD_SERVE_TOKEN", "SHOAL_SERVE_TOKEN"], help="Bearer token every /api request must "
                                       "carry. Prefer the TELLFOLD_SERVE_TOKEN env var (argv is visible in ps).")],
    port: Annotated[int, typer.Option(min=0, max=65535, help="Port on 127.0.0.1; 0 picks a free one.")] = 0,
    runs: Annotated[Path | None, typer.Option(help="Folder that holds run directories "
                                              "(default: settings runs_dir, else ./runs).")] = None,
    examples: Annotated[Path | None, typer.Option(help="Folder of read-only example runs to list next to the "
                                                  "runs (the Mac app passes its bundled examples).")] = None,
    log_level: Annotated[str, typer.Option(help="debug, info, warning or error (logs go to stderr).")] = "warning",
    watch_stdin: Annotated[bool, typer.Option("--watch-stdin/--no-watch-stdin",
                                              help="Exit when stdin closes (the Mac app keeps it open).")] = True,
) -> None:
    """Headless API server for the Mac app: prints {"port": N} on stdout once listening, then logs to stderr."""
    import asyncio
    import json

    err = Console(stderr=True)
    if len(token) < 16:
        err.print("[bold red]error:[/] --token must be at least 16 characters")
        raise typer.Exit(2)
    if log_level.lower() not in {"debug", "info", "warning", "error"}:
        err.print("[bold red]error:[/] --log-level must be debug, info, warning or error")
        raise typer.Exit(2)
    try:
        import uvicorn

        from tellfold.server.app import create_app
        from tellfold.server.auth import TokenAuthMiddleware
        from tellfold.settings import load_settings
    except ImportError as e:
        err.print(f"[bold red]error:[/] the server could not load: {e}. Run `uv sync`.")
        raise typer.Exit(1)

    runs_dir = (runs or Path(load_settings().runs_dir or "runs")).expanduser().resolve()
    runs_dir.mkdir(parents=True, exist_ok=True)

    def llm_factory(run_dir: Path | None, concurrency: int, think: bool = False) -> LLM:
        return make_llm(run_dir, concurrency, think)

    examples_dir = examples.expanduser().resolve() if examples else None
    if examples_dir is not None and not examples_dir.is_dir():
        err.print(f"[yellow]warning:[/] --examples {examples_dir} is not a folder; no examples will be listed")
        examples_dir = None
    application = create_app(runs_dir, llm_factory, examples_dir=examples_dir)
    application.add_middleware(TokenAuthMiddleware, token=token)
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        sock.bind(("127.0.0.1", port))
    except OSError as e:
        err.print(f"[bold red]error:[/] cannot bind 127.0.0.1:{port}: {e}")
        raise typer.Exit(1)
    sock.listen(128)
    sock.setblocking(False)
    bound_port = sock.getsockname()[1]
    server = uvicorn.Server(uvicorn.Config(application, log_config=_stderr_log_config(log_level),
                                           access_log=log_level.lower() == "debug", timeout_graceful_shutdown=3))
    err.print(f"tellfold serve: 127.0.0.1:{bound_port}  runs: {runs_dir}" + ("  (fake LLM)" if fake_mode() else ""),
              soft_wrap=True, highlight=False)

    async def main_task() -> None:
        loop = asyncio.get_running_loop()
        task = asyncio.create_task(server.serve(sockets=[sock]))
        while not server.started:
            if task.done():
                await task
                return
            await asyncio.sleep(0.02)
        sys.stdout.write(json.dumps({"port": bound_port}) + "\n")
        sys.stdout.flush()
        if watch_stdin:
            def wait_for_eof() -> None:
                try:
                    while sys.stdin.buffer.read(65536):
                        pass
                except (OSError, ValueError, AttributeError):
                    pass
                loop.call_soon_threadsafe(setattr, server, "should_exit", True)
                _exit_later(_SERVE_EOF_EXIT_SECONDS)

            threading.Thread(target=wait_for_eof, name="tellfold-stdin-watch", daemon=True).start()
        await task
        # A request thread stuck in a blocking call (e.g. a file open waiting on a macOS privacy prompt) would
        # otherwise keep asyncio.run() waiting for the thread pool and leave an orphaned engine behind the app.
        _exit_later(_SERVE_SHUTDOWN_EXIT_SECONDS)

    try:
        asyncio.run(main_task())
    except KeyboardInterrupt:
        pass
    finally:
        sock.close()


def legacy_main() -> None:
    """The deprecated `shoal` command from before the rename: a notice on stderr, then the same CLI."""
    Console(stderr=True).print("[yellow]`shoal` is now `tellfold`. The old name still works for now; please switch.[/]")
    app(prog_name="tellfold")
