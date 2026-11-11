"""Record what the Python `tellfold` command prints and writes for a session of commands on the canned model.

Run from the repository root:  uv run python engine-rs/golden/make_cli_runs.py

Writes golden/fixtures/cli_runs/session.json. The Rust test (crates/tellfold-cli/tests/cli_runs.rs) runs the same
steps with the Rust binary in a fresh folder and compares exit codes, stdout, stderr and the files written, after the
same normalisation: the folder becomes `<cwd>`, run and study ids become `{run:N}` / `{study:N}`, stage timings, timestamps and pids are masked.
"""

from __future__ import annotations

import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = Path(__file__).resolve().parent / "fixtures" / "cli_runs"
EXAMPLE = ROOT / "examples" / "mossbook_pricing.md"
QUESTION = "Will existing buyers accept the subscription?"
RUN = ["run", "seed.md", "-q", QUESTION, "--agents", "4", "--rounds", "2", "--batch", "2", "--out", "runs"]

# (args, stdin, extra env). `{run:N}` is the N-th run folder made so far (by step, then by name).
STEPS: list[tuple[list[str], str | None, dict[str, str]]] = [
    ([*RUN, "--event", "2:A rival app says it will stay a one-time purchase"], None, {}),
    (["report", "runs/{run:0}"], None, {}),
    (["agents", "runs/{run:0}"], None, {}),
    (["ask", "runs/{run:0}", "Who changed their mind?"], None, {}),
    (["chat", "runs/{run:0}", "--agent", "a01"], "Why?\n\nexit\n", {}),
    (["chat", "runs/{run:0}", "-a", "@nobody"], None, {}),
    (["agents", "empty"], None, {}),
    ([*RUN, "--event", "5:too late"], None, {}),
    ([*RUN, "--event", "nonsense"], None, {}),
    ([*RUN, "--variant", "B"], None, {}),
    ([*RUN, "--agent-runtime", "nope"], None, {}),
    (["run", "empty", "-q", QUESTION, "--out", "runs"], None, {}),
    ([*RUN, "--cast-from", "runs/{run:0}", "--variant", "Shorter"], None, {}),
    (["compare", "runs/{run:1}"], None, {}),
    (["compare", "runs/{run:0}"], None, {}),
    (["run", "seed.md", "-q", QUESTION, "--out", "elsewhere", "--cast-from", "runs/{run:0}"], None, {}),
    (["check", "runs/{run:0}", "--runs", "2"], None, {}),
    (["branch", "runs/{run:0}", "--at", "1", "--event", "2:The owner apologises"], None, {}),
    (["branch", "runs/{run:0}", "--at", "2"], None, {}),
    (["rewrite", "runs/{run:0}"], None, {}),
    (["run", "seeds-dir", "-q", "Folder seeds?", "--agents", "2", "--rounds", "1", "--out", "runs"], None, {}),
    (["doctor"], None, {"PATH": "/nonexistent"}),
    (["--version"], None, {}),
    (["agents"], None, {}),
    (["agents", "runs/{run:0}", "extra"], None, {}),
    (["agents", "runs/{run:0}", "x\ty", "z"], None, {}),
    (["nope"], None, {}),
    (["runn"], None, {}),
    (["--", "-x"], None, {}),
    (["--"], None, {}),
    (["--bogus", "run"], None, {}),
    (["--versio"], None, {}),
    (["--help=1"], None, {}),
    (["run"], None, {}),
    (["run", "seed.md"], None, {}),
    (["run", "seed.md", "-q"], None, {}),
    (["run", "nope.md", "-q", "x"], None, {}),
    (["run", "seed.md", "-q", "x", "--agents", "1"], None, {}),
    (["run", "seed.md", "-q", "x", "--agents", "abc"], None, {}),
    (["run", "seed.md", "-q", "x", "--seed", "1.5"], None, {}),
    (["run", "seed.md", "-q", "x", "--agnts", "3"], None, {}),
    (["run", "seed.md", "-q", "x", "--think=1"], None, {}),
    (["run", "seed.md", "-q", "x", "-z"], None, {}),
    (["compare", "seed.md"], None, {}),
    (["compare", "missing"], None, {}),
    (["branch", "runs/{run:0}"], None, {}),
    (["branch", "runs/{run:0}", "--at", "-1"], None, {}),
    (["ask", "runs/{run:0}"], None, {}),
    (["check", "runs/{run:0}", "--runs", "9"], None, {}),
    (["ui", "--port", "0"], None, {}),
    (["serve"], None, {}),
    (["runtimes", "--json=1"], None, {}),
    (["run", "--help", "--bogus"], None, {}),
    (["run", "--bogus", "--help"], None, {}),
    (["run", "seed.md", "--help"], None, {}),
    (["agents", "--help"], None, {}),
]

# files compared after the session, per run folder and per study folder
RUN_FILES = ["config.json", "status.json", "graph.json", "personas.json", "report.json", "report.md",
             "rewrite.json", "rewrite.md"]
STUDY_FILES = ["study.json", "compare.json", "agreement.json"]

FORCED = {"TERMINAL_WIDTH", "GITHUB_ACTIONS", "FORCE_COLOR", "PY_COLORS", "_TYPER_FORCE_DISABLE_TERMINAL",
          "UNICODE_VERSION", "TTY_COMPATIBLE", "TTY_INTERACTIVE", "JUPYTER_COLUMNS"}
TIMING = re.compile(r"\b\d+\.\ds\b")
PID = re.compile(r'"pid": \d+')
STAMP = re.compile(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:\d\d)?")


def run_dirs(cwd: Path) -> list[str]:
    runs = cwd / "runs"
    return sorted(p.name for p in runs.iterdir() if p.is_dir() and p.name != "studies") if runs.is_dir() else []


def study_ids(cwd: Path, runs: list[str]) -> list[str]:
    """Study ids in the order the runs joined them (the ids end in random hex, so their names say nothing)."""
    ordered: list[str] = []
    for name in runs:
        config = json.loads((cwd / "runs" / name / "config.json").read_text(encoding="utf-8"))
        if config.get("study_id") and config["study_id"] not in ordered:
            ordered.append(config["study_id"])
    folder = cwd / "runs" / "studies"
    rest = sorted(p.name for p in folder.iterdir() if p.is_dir() and p.name not in ordered) if folder.is_dir() else []
    return ordered + rest


def normalise(text: str, cwd: Path, runs: list[str], studies: list[str]) -> str:
    for folder in sorted({str(cwd.resolve()), str(cwd)}, key=len, reverse=True):
        text = text.replace(folder, "<cwd>")
    # longest first: a run made in the same second as another is named like it with "-2" added
    names = [(name, f"{{run:{i}}}") for i, name in enumerate(runs)]
    names += [(name, f"{{study:{i}}}") for i, name in enumerate(studies)]
    for name, placeholder in sorted(names, key=lambda n: -len(n[0])):
        text = text.replace(name, placeholder)
    # doctor's Python row: the patch release differs between machines
    text = text.replace(f"│ {platform.python_version()} ", f"│ {'3.13.x':<{len(platform.python_version())}} ")
    return PID.sub('"pid": 0', STAMP.sub("<time>", TIMING.sub("<t>s", text)))


def setup(cwd: Path) -> None:
    shutil.copy(EXAMPLE, cwd / "seed.md")
    (cwd / "empty").mkdir()
    seeds = cwd / "seeds-dir"
    seeds.mkdir()
    (seeds / "b.md").write_text("Second note: the price goes up next month.\n", encoding="utf-8")
    (seeds / "a.txt").write_text("First note: buyers liked the one-time price.\n", encoding="utf-8")
    (seeds / "skip.png").write_bytes(b"not a seed")


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="tellfold-cli-golden-") as tmp:
        cwd = Path(tmp) / "work"
        cwd.mkdir()
        setup(cwd)
        # typer forces colour under GITHUB_ACTIONS / FORCE_COLOR: the fixtures are for a plain, colourless console
        env = {k: v for k, v in os.environ.items() if not k.startswith(("TELLFOLD_", "SHOAL_")) and k not in FORCED}
        env.update(TELLFOLD_FAKE_LLM="1", TELLFOLD_HOME=str(Path(tmp) / "home"), COLUMNS="100", LINES="40",
                   PYTHONIOENCODING="utf-8")
        made: list[str] = []
        steps = []
        for args, stdin, extra in STEPS:
            actual = [re.sub(r"\{run:(\d+)\}", lambda m: made[int(m.group(1))], a) for a in args]
            done = subprocess.run([sys.executable, "-m", "tellfold", *actual], cwd=cwd, input=stdin,
                                  capture_output=True, text=True, env={**env, **extra}, check=False)
            made += [name for name in run_dirs(cwd) if name not in made]
            steps.append({"args": args, "stdin": stdin, "env": extra, "exit": done.returncode,
                          "stdout": done.stdout, "stderr": done.stderr})
        runs = made  # in the order the steps made them
        assert sorted(runs) == run_dirs(cwd)
        studies = study_ids(cwd, runs)
        for step in steps:
            step["stdout"] = normalise(step["stdout"], cwd, runs, studies)
            step["stderr"] = normalise(step["stderr"], cwd, runs, studies)
        files: dict[str, str] = {}
        for i, name in enumerate(runs):
            for file in RUN_FILES:
                path = cwd / "runs" / name / file
                if path.is_file():
                    files[f"runs/{{run:{i}}}/{file}"] = normalise(path.read_text(encoding="utf-8"), cwd, runs, studies)
            seeds = cwd / "runs" / name / "seeds"
            files[f"runs/{{run:{i}}}/seeds/"] = "\n".join(sorted(p.name for p in seeds.iterdir())) if seeds.is_dir() else ""
        for i, sid in enumerate(studies):
            for file in STUDY_FILES:
                path = cwd / "runs" / "studies" / sid / file
                if path.is_file():
                    files[f"runs/studies/{{study:{i}}}/{file}"] = normalise(path.read_text(encoding="utf-8"), cwd,
                                                                           runs, studies)
        files["elsewhere/"] = "\n".join(sorted(p.name for p in (cwd / "elsewhere").iterdir())) \
            if (cwd / "elsewhere").is_dir() else "<missing>"
    OUT.mkdir(parents=True, exist_ok=True)
    session = {"question": QUESTION, "runs": len(runs), "studies": len(studies), "steps": steps, "files": files}
    (OUT / "session.json").write_text(json.dumps(session, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {OUT / 'session.json'}: {len(steps)} steps, {len(runs)} runs, {len(studies)} studies")


if __name__ == "__main__":
    main()
