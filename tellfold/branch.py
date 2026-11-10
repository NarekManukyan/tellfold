"""Branch a run from any round: copy the run, keep rounds 0..r, change what happens next, and continue.

The copy shares rounds 0 to r with its source exactly: the same people, posts, reactions, follows and the agents'
memories as they were at the end of round r (kept per round in `memory_history`). New events replace the source's
events after round r, and the engine continues from round r + 1 (`SimConfig.branched_from` / `branch_round`).
"""

from __future__ import annotations

import shutil
import sqlite3
from pathlib import Path

from tellfold.models import GodEvent, SimConfig
from tellfold.runner import SEEDS_DIR, RunError, write_json
from tellfold.world import World


def branch_rounds(source_dir: Path, *, immutable: bool = False) -> tuple[int, list[int]]:
    """(last round, rounds a branch can start from): every round whose memories were kept, plus round 0 and the
    last round, which older runs without a history can always use. A branch needs at least one round to run."""
    world = World(source_dir / "world.db", read_only=immutable)
    try:
        last, kept = world.last_round(), world.memory_rounds()
    finally:
        world.close()
    return last, sorted({0, last, *kept})


def prepare_branch(source_dir: Path, run_dir: Path, source: SimConfig, round: int, events: list[GodEvent], *,
                   source_id: str | None = None, immutable: bool = False, study_id: str | None = None,
                   variant: str | None = None) -> SimConfig:
    """Fill `run_dir` (already created) with a copy of `source_dir` cut after `round`, and return its config.

    `events` are the branch's events after `round` (they replace the source's); events up to `round` stay as they
    happened. `source_id` is the source's run id when it is not its folder name (an example's `example-<folder>`).
    Raises RunError when the run cannot be branched there."""
    db = source_dir / "world.db"
    if not db.is_file():
        raise RunError(f"{source_dir.name} has no simulated world to branch")
    last, allowed = branch_rounds(source_dir, immutable=immutable)
    if round not in allowed:
        raise RunError(f"{source_dir.name} can be branched from round {', '.join(map(str, allowed))} "
                       f"(it did not keep its agents' memories for round {round})")
    if round >= source.rounds:
        raise RunError(f"round {round} is the last round; a branch needs at least one round left to run")
    for e in events:
        if not round < e.round <= source.rounds:
            raise RunError(f"event round {e.round} is outside {round + 1}..{source.rounds}")

    for name in ("graph.json", "personas.json"):
        if (source_dir / name).is_file():
            shutil.copyfile(source_dir / name, run_dir / name)
    if (source_dir / SEEDS_DIR).is_dir():
        shutil.copytree(source_dir / SEEDS_DIR, run_dir / SEEDS_DIR, dirs_exist_ok=True)
    _copy_db(db, run_dir / "world.db", immutable=immutable)
    world = World(run_dir / "world.db", seed=source.seed)
    try:
        world.truncate_after(round)
    except ValueError as e:
        raise RunError(str(e)) from e
    finally:
        world.close()

    kept = [e for e in source.events if e.round <= round]
    config = source.model_copy(update={
        "seed_files": [str(p) for p in sorted((run_dir / SEEDS_DIR).glob("*"))] if (run_dir / SEEDS_DIR).is_dir()
        else source.seed_files,
        "events": kept + sorted(events, key=lambda e: e.round),
        "branched_from": source_id or source_dir.name, "branch_round": round, "cast_from": source_id or source_dir.name,
        "study_id": study_id, "variant": variant,
    })
    write_json(run_dir / "config.json", config)
    return config


def _copy_db(source: Path, target: Path, *, immutable: bool) -> None:
    """A consistent copy through SQLite's backup API (it includes what is still in the source's WAL file)."""
    mode = "ro&immutable=1" if immutable else "ro"
    src = sqlite3.connect(f"{source.resolve().as_uri()}?mode={mode}", uri=True)
    dst = sqlite3.connect(str(target))
    try:
        src.backup(dst)
    finally:
        dst.close()
        src.close()
