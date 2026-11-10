"""Studies: groups of runs that are compared with each other, kept in `<runs>/studies/<id>/`.

`study.json` (a `Study`) lists the member runs and their labels. A run made from another one ("Try another
version…", `--cast-from`) joins the source's study, or starts one with the source labelled "Draft A". The source
can be a read-only example: it joins through `study.json` only, never by changing its own files.
"""

from __future__ import annotations

import os
import re
import secrets
import string
import tempfile
from datetime import datetime
from pathlib import Path

from pydantic import ValidationError

from tellfold.models import SimConfig, Study, StudyRun

STUDIES_DIR = "studies"
STUDY_FILE = "study.json"
STUDY_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,99}$")
LETTERS = string.ascii_uppercase


class StudyError(Exception):
    """The study cannot be read or joined for a reason the user can fix (bad id, unknown study)."""


def letter(index: int) -> str:
    """The citation prefix of the study's index-th run: A, B, ... Z."""
    return LETTERS[index] if 0 <= index < len(LETTERS) else f"R{index + 1}"


def default_variant(index: int) -> str:
    return f"Draft {letter(index)}"


def new_study_id(now: datetime | None = None) -> str:
    return f"{(now or datetime.now()).strftime('%Y%m%d-%H%M%S')}-{secrets.token_hex(2)}"


def study_dir(runs_dir: Path, study_id: str) -> Path:
    if not STUDY_ID.match(study_id) or ".." in study_id:
        raise StudyError("invalid study id")
    return runs_dir / STUDIES_DIR / study_id


def load_study(runs_dir: Path, study_id: str) -> Study | None:
    try:
        path = study_dir(runs_dir, study_id) / STUDY_FILE
        return Study.model_validate_json(path.read_text(encoding="utf-8"))
    except (OSError, ValueError, ValidationError, StudyError):
        return None


def save_study(runs_dir: Path, study: Study) -> None:
    """Atomic write (temp file + os.replace), like status.json."""
    folder = study_dir(runs_dir, study.id)
    folder.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=".study-", suffix=".tmp", dir=folder)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write(study.model_dump_json(indent=2) + "\n")
        os.replace(tmp, folder / STUDY_FILE)
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise


def list_studies(runs_dir: Path) -> list[Study]:
    root = runs_dir / STUDIES_DIR
    if not root.is_dir():
        return []
    studies = [load_study(runs_dir, p.name) for p in sorted(root.iterdir()) if p.is_dir()]
    return [s for s in studies if s is not None]


def study_of(runs_dir: Path, run_id: str, config: SimConfig) -> Study | None:
    """The study a run belongs to: its own `study_id`, else the first study that lists it (a source run)."""
    if config.study_id and (study := load_study(runs_dir, config.study_id)) is not None:
        return study
    return next((s for s in list_studies(runs_dir) if any(r.id == run_id for r in s.runs)), None)


def plan_variant(runs_dir: Path, source_id: str, source: SimConfig, *, study_id: str | None = None,
                 variant: str | None = None, now: str = "") -> tuple[Study, str]:
    """The study a new run made from `source_id` joins, with the source already in it, and the new run's label.

    The study is `study_id`, else the source's own study, else a new one. Nothing is written: call `join_study`
    once the new run exists."""
    study_id = study_id or source.study_id
    study = load_study(runs_dir, study_id) if study_id else None
    if study_id and study is None:
        study_dir(runs_dir, study_id)  # validates the id
        if study_id != source.study_id:
            raise StudyError(f"there is no study {study_id}")
    study = study or Study(id=study_id or new_study_id(), created_at=now)
    if not any(r.id == source_id for r in study.runs):
        study.runs.append(StudyRun(id=source_id, variant=(source.variant or "").strip()
                                   or default_variant(len(study.runs))))
    label = (variant or "").strip()[:60] or default_variant(len(study.runs))
    return study, label


def join_study(runs_dir: Path, study: Study, run_id: str, variant: str) -> Study:
    """Add `run_id` to `study` (or relabel it) and save the study."""
    runs = [StudyRun(id=r.id, variant=variant) if r.id == run_id else r for r in study.runs]
    if not any(r.id == run_id for r in runs):
        runs.append(StudyRun(id=run_id, variant=variant))
    joined = study.model_copy(update={"runs": runs})
    save_study(runs_dir, joined)
    return joined
