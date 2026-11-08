"""Turn a finished run into a built-in example: a self-contained, read-only copy the Mac app ships in its bundle.

The copy keeps what the app reads (config, graph, personas, world, report, status, calls log, seeds) and drops
what it must not ship or cannot open read-only: SQLite's WAL side files (the database is switched to a rollback
journal so `mode=ro&immutable=1` can open it from a signed bundle), the owning process id, and absolute seed paths
from the machine that made the run (in `config.json` and the footer of `report.html`).
"""

from __future__ import annotations

import json
import shutil
import sqlite3
from pathlib import Path

from tellfold.models import Report, RunState, RunStatus, SimConfig
from tellfold.report import render_html
from tellfold.runner import SEEDS_DIR, write_json, write_status
from tellfold.world import World

KEEP = ("config.json", "graph.json", "personas.json", "world.db", "calls.jsonl", "status.json",
        "report.json", "report.md", "report.html")
PROMPT_KEYS = ("prompt", "system", "response", "reply", "content")


class ExampleError(Exception):
    pass


def pack_example(run_dir: Path, dest: Path) -> Path:
    """Copy finished run `run_dir` to the new folder `dest`, ready to ship. Returns `dest`."""
    status = RunStatus.model_validate_json((run_dir / "status.json").read_text(encoding="utf-8"))
    if status.state != RunState.DONE or not (run_dir / "report.json").is_file():
        raise ExampleError(f"{run_dir} is not a finished run with a report")
    if dest.exists():
        raise ExampleError(f"{dest} already exists")
    dest.mkdir(parents=True)
    for name in KEEP:
        if (run_dir / name).is_file():
            shutil.copy2(run_dir / name, dest / name)
    _check_calls(dest / "calls.jsonl")
    seeds = dest / SEEDS_DIR
    shutil.copytree(run_dir / SEEDS_DIR, seeds)

    src = sqlite3.connect(f"{(run_dir / 'world.db').resolve().as_uri()}?mode=ro", uri=True)
    try:
        (dest / "world.db").unlink()
        out = sqlite3.connect(dest / "world.db")
        src.backup(out)
        out.execute("PRAGMA journal_mode=DELETE")
        out.execute("VACUUM")
        out.close()
    finally:
        src.close()

    config = SimConfig.model_validate_json((dest / "config.json").read_text(encoding="utf-8"))
    names = sorted(f.name for f in seeds.iterdir() if f.is_file() and not f.name.startswith("."))
    config = config.model_copy(update={"seed_files": [f"{SEEDS_DIR}/{n}" for n in names]})
    write_json(dest / "config.json", config)
    # report.html names the seed files: render it again so it shows the relative paths
    report = Report.model_validate_json((dest / "report.json").read_text(encoding="utf-8"))
    world = World(dest / "world.db", seed=config.seed, read_only=True)
    try:
        (dest / "report.html").write_text(render_html(report, world, config), encoding="utf-8")
    finally:
        world.close()
    write_status(dest, status.model_copy(update={"pid": None, "detail": ""}))
    return dest


def _check_calls(path: Path) -> None:
    """The calls log must hold timings and outcomes only, never prompt or answer text."""
    if not path.is_file():
        return
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.strip() and any(key in json.loads(line) for key in PROMPT_KEYS):
            raise ExampleError(f"{path} holds prompt or answer text; it cannot ship")
