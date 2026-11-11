#!/usr/bin/env python3
"""Tellfold accuracy benchmark: run every case in bench/cases through the `tellfold` CLI and grade the reports.

    python bench/run.py --dry-run              # free: the CLI's offline model and a word-overlap judge
    python bench/run.py --estimate             # model calls and API-equivalent cost of a full pass
    python bench/run.py --validate             # check the case files only
    python bench/run.py                        # a real pass (uses your AI tool's plan)

A case is a folder with `input.md` (what was known on the day, the only seed), `outcome.md` (what happened, with
sources; never shown to the simulation or the judge) and `rubric.json` (direction, objections, backdown, groups).
Every report is graded by one judge call (`bench/judge_prompt.md`) that never sees the true direction or backdown.
Results go to `bench/results/<date>-<label>.json` (dry runs to `bench/runs/results/`) with a Markdown summary next
to it. Standard library only.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import os
import re
import shlex
import shutil
import sqlite3
import subprocess
import sys
from pathlib import Path

BENCH = Path(__file__).resolve().parent
REPO = BENCH.parent
CASES_DIR = BENCH / "cases"
RESULTS_DIR = BENCH / "results"
RUNS_DIR = BENCH / "runs"  # ignored by git (`runs/` in .gitignore)
DRY_RESULTS_DIR = RUNS_DIR / "results"  # dry-run numbers mean nothing, so they stay out of bench/results

DIRECTIONS = ("negative", "mixed", "positive")
BACKDOWNS = ("none", "partial", "full")
CATEGORIES = (
    "pricing", "product-change", "rebrand", "policy", "local-government", "school", "public-service", "workplace",
)

# Fixed settings for every case: the app's defaults for a new simulation.
SETTINGS = {
    "agents": 20,
    "rounds": 6,
    "batch": 6,
    "agent_model": "haiku",
    "planner_model": "sonnet",
    "concurrency": 4,
    "language": "English",
    "runtime": "claude-code",
}
FIRST_SEED = 7

# A run's last-round sentiment (-1..1) read as a direction, for the secondary, judge-free direction score.
SENTIMENT_BAND = 0.2

JUDGE_SCHEMA = {
    "type": "object",
    "properties": {
        "predicted_direction": {"type": "string", "enum": list(DIRECTIONS)},
        "direction_evidence": {"type": "string"},
        "predicted_backdown": {"type": "string", "enum": [*BACKDOWNS, "unclear"]},
        "backdown_evidence": {"type": "string"},
        "objections": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "verdict": {"type": "string", "enum": ["covered", "partial", "missed"]},
                    "evidence": {"type": "string"},
                },
                "required": ["id", "verdict", "evidence"],
            },
        },
        "groups": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {"group": {"type": "string"}, "named": {"type": "boolean"}},
                "required": ["group", "named"],
            },
        },
        "extra_objections": {"type": "array", "items": {"type": "string"}},
    },
    "required": ["predicted_direction", "predicted_backdown", "objections", "groups"],
}

ZERO_SHOT_SCHEMA = {
    "type": "object",
    "properties": {
        "summary": {"type": "string"},
        "prediction": {"type": "string"},
        "confidence": {"type": "string", "enum": ["low", "medium", "high"]},
        "camps": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {"name": {"type": "string"}, "summary": {"type": "string"}},
                "required": ["name", "summary"],
            },
        },
        "risks": {"type": "array", "items": {"type": "string"}},
    },
    "required": ["summary", "prediction", "confidence", "camps", "risks"],
}

# Words that suggest input.md describes the reaction instead of the announcement. Warnings, not errors.
LEAK_WORDS = (
    "backlash", "outrage", "outcry", "u-turn", "reversed", "reversal", "backtrack", "uproar", "furious", "criticised",
    "criticized", "petition", "boycott", "walked back", "climbdown", "controversial",
)
# Words about the benchmark itself. A seed holds only the announcement; case metadata lives in rubric.json. Errors.
META_WORDS = ("benchmark", "accuracy test", "control case", "training cutoff", "known to the model")

# ---------- cost estimate ----------
# Average characters per call by purpose, measured from five real 20-agent, 6-round runs (Claude Code, haiku agents,
# sonnet planner, 7-11 Nov 2026; `calls.jsonl` records prompt and output lengths only). Seeds in this benchmark are
# 200-500 words, shorter than those runs' seeds, so graph and report prompts are rounded down a little.
CALL_CHARS = {
    "graph.extract": (5_000, 8_000),
    "personas.batch": (10_000, 8_000),
    "engine.seed": (10_500, 2_100),
    "engine.round": (27_000, 3_300),
    "engine.sentiment": (2_000, 160),
    "report.write": (21_000, 10_500),
    "judge": (16_000, 2_000),
    "zero_shot": (4_000, 5_000),
}
CHARS_PER_TOKEN = 4
# System prompt and JSON schema sent with every call, which `calls.jsonl` does not count.
OVERHEAD_TOKENS = 1_500
# The report call keeps extended thinking on; hidden thinking tokens are billed as output.
REPORT_THINKING_TOKENS = 8_000
# USD per million tokens (input, output), API list prices for prompts under 100k tokens, Nov 2026. A Claude Code
# subscription is not billed per token; this is the API-equivalent cost, to compare against plan limits.
PRICES = {"haiku": (0.10, 0.50), "sonnet": (2.00, 10.00), "opus": (5.00, 25.00)}
CHUNK_SIZE, CHUNK_OVERLAP = 1800, 200


def estimate_calls(agents: int, rounds: int, batch: int, chunks: int) -> dict[str, int]:
    """Model calls by purpose for one run, the same formula as `tellfold-store/src/estimate.rs`."""
    active = max(1, math.ceil(agents * 0.4))
    return {
        "graph.extract": math.ceil(min(chunks, 64) / 4),
        "personas.batch": math.ceil(agents / 10),
        "engine.seed": math.ceil(round(0.3 * agents) / batch),
        "engine.round": rounds * math.ceil(active / batch),
        "engine.sentiment": rounds + 1,
        "report.write": 1,
    }


def chunk_count(text: str) -> int:
    return max(1, math.ceil((len(text) - CHUNK_OVERLAP) / (CHUNK_SIZE - CHUNK_OVERLAP)))


def call_cost(purpose: str, model: str, calls: int) -> tuple[int, int, float]:
    chars_in, chars_out = CALL_CHARS[purpose]
    tokens_in = calls * (chars_in // CHARS_PER_TOKEN + OVERHEAD_TOKENS)
    tokens_out = calls * (chars_out // CHARS_PER_TOKEN + (REPORT_THINKING_TOKENS if purpose == "report.write" else 0))
    price_in, price_out = price_for(model)
    return tokens_in, tokens_out, (tokens_in * price_in + tokens_out * price_out) / 1e6


def price_for(model: str) -> tuple[float, float]:
    """List price for a model alias or full id ("sonnet", "claude-sonnet-5-5"); an unknown model is an error."""
    for family, price in PRICES.items():
        if family in model.lower():
            return price
    raise SystemExit(f"no price for model {model!r}: use one of {', '.join(PRICES)} or a full id containing one")


def estimate(cases: list[dict], settings: dict, repeats: int, zero_shot: bool, judge_model: str) -> dict:
    agent_model, planner_model = settings["agent_model"], settings["planner_model"]
    rows: dict[str, list] = {}

    def add(purpose: str, model: str, calls: int) -> None:
        tokens_in, tokens_out, usd = call_cost(purpose, model, calls)
        row = rows.setdefault(purpose, [model, 0, 0, 0, 0.0])
        row[1] += calls
        row[2] += tokens_in
        row[3] += tokens_out
        row[4] += usd

    for case in cases:
        chunks = chunk_count(case["input"].read_text(encoding="utf-8"))
        calls = estimate_calls(settings["agents"], settings["rounds"], settings["batch"], chunks)
        for _ in range(repeats):
            for purpose, n in calls.items():
                add(purpose, agent_model if purpose.startswith("engine.") else planner_model, n)
            add("judge", judge_model, 1)
        if zero_shot:
            add("zero_shot", planner_model, 1)
            add("judge", judge_model, 1)
    total = [sum(r[i] for r in rows.values()) for i in (1, 2, 3, 4)]
    return {
        "cases": len(cases),
        "repeats": repeats,
        "zero_shot": zero_shot,
        "rows": {k: {"model": v[0], "calls": v[1], "tokens_in": v[2], "tokens_out": v[3], "usd": round(v[4], 3)}
                 for k, v in rows.items()},
        "total": {"calls": total[0], "tokens_in": total[1], "tokens_out": total[2], "usd": round(total[3], 2)},
    }


def print_estimate(est: dict) -> None:
    print(f"{est['cases']} cases x {est['repeats']} run(s){' + zero-shot baseline' if est['zero_shot'] else ''}")
    print(f"{'purpose':<18}{'model':<8}{'calls':>7}{'tokens in':>12}{'tokens out':>12}{'USD':>9}")
    for purpose, r in est["rows"].items():
        print(f"{purpose:<18}{r['model']:<8}{r['calls']:>7}{r['tokens_in']:>12,}{r['tokens_out']:>12,}{r['usd']:>9.2f}")
    t = est["total"]
    print(f"{'total':<26}{t['calls']:>7}{t['tokens_in']:>12,}{t['tokens_out']:>12,}{t['usd']:>9.2f}")
    print("API-equivalent list prices; on a Claude Code plan this counts against usage limits instead.")


# ---------- cases ----------


def load_cases(only: list[str] | None = None, folder_root: Path = CASES_DIR) -> list[dict]:
    cases = []
    for folder in sorted(p for p in folder_root.iterdir() if p.is_dir()):
        if only and folder.name not in only:
            continue
        rubric_path = folder / "rubric.json"
        rubric = json.loads(rubric_path.read_text(encoding="utf-8")) if rubric_path.exists() else {}
        cases.append({"slug": folder.name, "dir": folder, "input": folder / "input.md", "rubric": rubric})
    if only:
        missing = sorted(set(only) - {c["slug"] for c in cases})
        if missing:
            raise SystemExit(f"unknown case(s): {', '.join(missing)}")
    return cases


def _check_rubric_fields(r: dict, slug: str) -> list[str]:
    errors = []
    if r.get("slug") != slug:
        errors.append(f"rubric slug {r.get('slug')!r} is not the folder name")
    for key in ("title", "question", "announced_on", "model_cutoff"):
        if not isinstance(r.get(key), str) or not r.get(key):
            errors.append(f"rubric.{key} missing")
    if r.get("category") not in CATEGORIES:
        errors.append(f"rubric.category {r.get('category')!r} not one of {CATEGORIES}")
    if r.get("direction") not in DIRECTIONS:
        errors.append(f"rubric.direction {r.get('direction')!r} not one of {DIRECTIONS}")
    return errors


def _check_dates(r: dict) -> list[str]:
    """known_to_model must agree with the announcement date and the model's cutoff month."""
    try:
        announced = dt.date.fromisoformat(r.get("announced_on") or "")
        cutoff = dt.date.fromisoformat((r.get("model_cutoff") or "") + "-01")
    except ValueError as e:
        return [f"bad date: {e}"]
    after = announced >= (cutoff.replace(day=28) + dt.timedelta(days=4)).replace(day=1)
    if r.get("known_to_model") is not (not after):
        return [f"rubric.known_to_model should be {not after} for {announced} and cutoff {cutoff:%Y-%m}"]
    return []


def _check_objections(r: dict) -> list[str]:
    errors = []
    objections = r.get("objections") or []
    if not 3 <= len(objections) <= 5:
        errors.append(f"{len(objections)} objections; the rubric needs 3-5")
    ids = [o.get("id") for o in objections]
    if len(set(ids)) != len(ids) or not all(ids):
        errors.append("objection ids must be present and unique")
    for o in objections:
        if not o.get("text"):
            errors.append(f"objection {o.get('id')} has no text")
        if not str(o.get("source", "")).startswith("http"):
            errors.append(f"objection {o.get('id')} has no source URL")
    return errors


def _check_backdown(r: dict) -> list[str]:
    b = r.get("backdown") or {}
    if b.get("kind") not in BACKDOWNS:
        return [f"backdown.kind {b.get('kind')!r} not one of {BACKDOWNS}"]
    if b.get("happened") is not (b["kind"] != "none"):
        return ["backdown.happened must be true exactly when kind is partial or full"]
    if b["kind"] != "none" and not str(b.get("source") or "").startswith("http"):
        return ["a backdown needs a source URL"]
    return []


def _check_input(path: Path) -> tuple[list[str], list[str]]:
    """The seed must be the announcement only: nothing about the benchmark, ideally nothing about the reaction."""
    if not path.exists():
        return [], []
    lowered = path.read_text(encoding="utf-8").lower()
    errors, warnings = [], []
    word_count = len(lowered.split())
    if not 120 <= word_count <= 700:
        warnings.append(f"input.md has {word_count} words (aim for 200-500)")
    meta = [w for w in META_WORDS if w in lowered]
    if meta:
        errors.append(f"input.md mentions {', '.join(meta)}: keep case metadata in rubric.json")
    leaks = [w for w in LEAK_WORDS if re.search(rf"\b{re.escape(w)}\b", lowered)]
    if leaks:
        warnings.append(f"input.md mentions {', '.join(leaks)}: check it does not describe the reaction")
    return errors, warnings


def validate_case(case: dict) -> tuple[list[str], list[str]]:
    """(errors, warnings) for one case folder."""
    folder, r = case["dir"], case["rubric"]
    errors = [f"missing {name}" for name in ("input.md", "outcome.md", "rubric.json") if not (folder / name).exists()]
    if not r:
        return errors, []
    errors += _check_rubric_fields(r, case["slug"]) + _check_dates(r) + _check_objections(r) + _check_backdown(r)
    warnings = [] if r.get("pushback_groups") else ["no pushback_groups"]
    input_errors, input_warnings = _check_input(case["input"])
    errors += input_errors
    warnings += input_warnings
    outcome = folder / "outcome.md"
    if outcome.exists() and len(re.findall(r"https?://", outcome.read_text(encoding="utf-8"))) < 2:
        errors.append("outcome.md cites fewer than two sources")
    return errors, warnings


def validate_all(cases: list[dict]) -> bool:
    ok = True
    for case in cases:
        errors, warnings = validate_case(case)
        for e in errors:
            print(f"error   {case['slug']}: {e}")
        for w in warnings:
            print(f"warning {case['slug']}: {w}")
        ok = ok and not errors
    print(f"{len(cases)} cases, {'all valid' if ok else 'some invalid'}")
    return ok


# ---------- prompts and the judge ----------


def split_prompt(path: Path) -> tuple[str, str]:
    """The `## System` and `## User` sections of a prompt file."""
    text = path.read_text(encoding="utf-8")
    # headings only: the files' introductions mention "## System" and "## User" in running text too
    parts = re.split(r"^## (System|User)[ \t]*$", text, flags=re.MULTILINE)
    sections = dict(zip(parts[1::2], (s.strip() for s in parts[2::2])))
    if set(sections) != {"System", "User"} or len(parts) != 5:
        raise ValueError(f"{path.name} needs exactly one '## System' and one '## User' heading")
    return sections["System"], sections["User"]


def extract_json(text: str) -> dict:
    start = text.find("{")
    if start < 0:
        raise ValueError("no JSON object in the reply")
    obj, _ = json.JSONDecoder().raw_decode(text[start:])
    return obj


class ClaudeModel:
    """One structured call through the local Claude Code CLI, with the same isolation flags the engine uses."""

    def __init__(self, cli: str, model: str, timeout_s: float = 600):
        self.cli, self.model, self.timeout_s = cli, model, timeout_s
        self.usage = {"calls": 0, "input_tokens": 0, "output_tokens": 0, "cost_usd": 0.0}

    def call(self, system: str, user: str, schema: dict) -> dict:
        argv = [
            self.cli, "-p", "--output-format", "json", "--system-prompt", system, "--tools", "", "--max-turns", "3",
            "--model", self.model, "--strict-mcp-config", "--setting-sources=", "--no-session-persistence",
            "--json-schema", json.dumps(schema),
        ]
        last_error = ""
        for _ in range(2):
            try:
                proc = subprocess.run(argv, input=user, capture_output=True, text=True, timeout=self.timeout_s)
            except subprocess.TimeoutExpired:
                last_error = f"no reply within {self.timeout_s:.0f}s"
                continue
            try:
                reply = json.loads(proc.stdout)
                if not isinstance(reply, dict):
                    raise ValueError(f"reply is a {type(reply).__name__}, not an object")
                if reply.get("is_error"):
                    raise ValueError(str(reply.get("result"))[:300])
                self._count(reply)
                data = reply.get("structured_output") or extract_json(reply.get("result") or "")
                if not isinstance(data, dict):
                    raise ValueError(f"structured output is a {type(data).__name__}, not an object")
                return data
            except ValueError as e:  # json.JSONDecodeError is a ValueError
                last_error = f"{e}; stderr: {proc.stderr[-300:]}"
        raise RuntimeError(f"claude call failed ({self.model}): {last_error}")

    def _count(self, reply: dict) -> None:
        usage = reply.get("usage") or {}
        self.usage["calls"] += 1
        self.usage["input_tokens"] += sum(
            int(usage.get(k) or 0) for k in ("input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens")
        )
        self.usage["output_tokens"] += int(usage.get("output_tokens") or 0)
        self.usage["cost_usd"] += float(reply.get("total_cost_usd") or 0)


STOPWORDS = set(
    "the and for that this with from have will they their them are was were been being into about than then there "
    "what when which while would could should also only more most some such very just over under because people".split()
)


def words(text: str) -> set[str]:
    return {w for w in re.findall(r"[a-z][a-z'-]{3,}", text.lower()) if w not in STOPWORDS}


class DryJudge:
    """Offline stand-in for the judge in `--dry-run`: word overlap, no model. Its grades mean nothing; it only lets
    the harness run end to end for free."""

    def __init__(self) -> None:
        self.usage = {"calls": 0, "input_tokens": 0, "output_tokens": 0, "cost_usd": 0.0}

    def grade(self, rubric: dict, report: dict, final_sentiment: float | None) -> dict:
        self.usage["calls"] += 1
        text = words(json.dumps(report))
        objections = []
        for o in rubric["objections"]:
            want = words(o["text"])
            share = len(want & text) / max(1, len(want))
            verdict = "covered" if share >= 0.6 else "partial" if share >= 0.3 else "missed"
            objections.append({"id": o["id"], "verdict": verdict, "evidence": ""})
        groups = [{"group": g, "named": bool(words(g) & text)} for g in rubric.get("pushback_groups", [])]
        return {
            "predicted_direction": sentiment_direction(final_sentiment) or "mixed",
            "direction_evidence": "(dry run: read from the last round's sentiment)",
            "predicted_backdown": "unclear",
            "backdown_evidence": "",
            "objections": objections,
            "groups": groups,
            "extra_objections": [],
        }


class DryModel:
    """Offline stand-in for the zero-shot call in `--dry-run`: a fixed, schema-valid answer."""

    def __init__(self) -> None:
        self.usage = {"calls": 0, "input_tokens": 0, "output_tokens": 0, "cost_usd": 0.0}

    def call(self, system: str, user: str, schema: dict) -> dict:
        self.usage["calls"] += 1
        return {
            "summary": "(offline test reply) Reactions will be mixed.",
            "prediction": "(offline test reply) Most people accept it after some grumbling.",
            "confidence": "low",
            "camps": [{"name": "Accept", "summary": "Fine with it."}, {"name": "Object", "summary": "Against it."}],
            "risks": ["(offline test reply) Some people object to the price."],
        }


class LlmJudge:
    def __init__(self, model: ClaudeModel):
        self.model = model
        self.system, self.template = split_prompt(BENCH / "judge_prompt.md")

    @property
    def usage(self) -> dict:
        return self.model.usage

    def grade(self, rubric: dict, report: dict, final_sentiment: float | None) -> dict:
        user = self.template.replace("{{CHECKLIST}}", json.dumps(judge_checklist(rubric), indent=2,
                                                                  ensure_ascii=False)).replace(
            "{{REPORT}}", json.dumps(report_for_judge(report), indent=2, ensure_ascii=False)
        )
        return self.model.call(self.system, user, judge_schema(rubric))


def judge_checklist(rubric: dict) -> dict:
    """What the judge may see of a rubric. Never the true direction or backdown: the judge classifies what the report
    predicts blind, and `score` compares that with the rubric."""
    return {
        "title": rubric["title"],
        "question": rubric["question"],
        "objections": [{"id": o["id"], "text": o["text"]} for o in rubric["objections"]],
        "pushback_groups": list(rubric.get("pushback_groups", [])),
    }


def judge_schema(rubric: dict) -> dict:
    """JUDGE_SCHEMA with this case's objection ids and group names as enums, so the reply can be matched exactly."""
    schema = json.loads(json.dumps(JUDGE_SCHEMA))
    props = schema["properties"]
    props["objections"]["items"]["properties"]["id"]["enum"] = [o["id"] for o in rubric["objections"]]
    groups = rubric.get("pushback_groups", [])
    if groups:
        props["groups"]["items"]["properties"]["group"]["enum"] = list(groups)
    else:
        props["groups"]["maxItems"] = 0
    return schema


def report_for_judge(report: dict) -> dict:
    """The parts of a report that carry its forecast; ids, citations audit and persona lists are left out."""
    keep = ("title", "summary", "prediction", "confidence", "reasons", "risks", "caveats")
    out = {k: report[k] for k in keep if k in report}
    out["camps"] = [{"name": c.get("name"), "summary": c.get("summary")} for c in report.get("camps", [])]
    out["sections"] = [{"title": s.get("title"), "body": s.get("body_markdown")} for s in report.get("sections", [])]
    out["recommendations"] = [
        {k: v for k, v in r.items() if k in ("action", "title", "why", "expected_effect")}
        for r in report.get("recommendations", [])
    ]
    return out


# ---------- running a case ----------


def sentiment_direction(value: float | None) -> str | None:
    if value is None:
        return None
    return "negative" if value < -SENTIMENT_BAND else "positive" if value > SENTIMENT_BAND else "mixed"


def last_sentiment(run_dir: Path) -> tuple[int | None, float | None]:
    """(last round, its mean sentiment). The sentiment is None when that round has no score: an earlier round's
    score is never used in its place."""
    db = run_dir / "world.db"
    if not db.exists():
        return None, None
    con = sqlite3.connect(db)
    try:
        row = con.execute("SELECT round, data FROM round_stats ORDER BY round DESC LIMIT 1").fetchone()
    finally:
        con.close()
    if row is None:
        return None, None
    return row[0], json.loads(row[1]).get("sentiment")


def run_tellfold(cmd: list[str], case: dict, out: Path, settings: dict, seed: int, env: dict) -> Path:
    """Runs one simulation into a fresh `out` folder and returns the run directory."""
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    argv = [
        *cmd, "run", str(case["input"]), "-q", case["rubric"]["question"],
        "--agents", str(settings["agents"]), "--rounds", str(settings["rounds"]), "--batch", str(settings["batch"]),
        "--agent-model", settings["agent_model"], "--planner-model", settings["planner_model"],
        "--concurrency", str(settings["concurrency"]), "--language", settings["language"], "--seed", str(seed),
        "--agent-runtime", settings["runtime"], "--planner-runtime", settings["runtime"], "--out", str(out),
    ]
    log = out / "tellfold.log"
    with log.open("w", encoding="utf-8") as fh:
        proc = subprocess.run(argv, stdout=fh, stderr=subprocess.STDOUT, env=env, cwd=REPO)
    runs = [p for p in out.iterdir() if p.is_dir()]
    if proc.returncode != 0 or len(runs) != 1:
        tail = log.read_text(encoding="utf-8", errors="replace")[-600:]
        raise RuntimeError(f"tellfold exited {proc.returncode} with {len(runs)} run folder(s): {tail}")
    return runs[0]


def count_calls(run_dir: Path) -> dict:
    calls, failed = 0, 0
    path = run_dir / "calls.jsonl"
    if path.exists():
        for line in path.read_text(encoding="utf-8").splitlines():
            if line.strip():
                calls += 1
                failed += not json.loads(line).get("ok", True)
    return {"calls": calls, "failed_calls": failed}


# ---------- scoring ----------

VERDICT_POINTS = {"covered": 1.0, "partial": 0.5, "missed": 0.0}


def _key(value) -> str:
    return " ".join(str(value or "").lower().split())


def score(rubric: dict, grade: dict) -> dict:
    """Compares the judge's blind classification with the rubric. Ids and group names are matched ignoring case and
    spacing; anything the judge returned that matches nothing in the rubric is listed in `unmatched`."""
    by_id = {_key(o.get("id")): o for o in grade.get("objections", [])}
    verdicts = [by_id.get(_key(o["id"]), {}).get("verdict", "missed") for o in rubric["objections"]]
    recall = sum(VERDICT_POINTS.get(v, 0.0) for v in verdicts) / len(verdicts)
    wanted = rubric.get("pushback_groups", [])
    named = {_key(g.get("group")): bool(g.get("named")) for g in grade.get("groups", [])}
    group_recall = sum(named.get(_key(g), False) for g in wanted) / len(wanted) if wanted else None
    known = {_key(o["id"]) for o in rubric["objections"]} | {_key(g) for g in wanted}
    unmatched = [k for k in [*by_id, *named] if k not in known]
    predicted = grade.get("predicted_direction")
    actual = rubric["direction"]
    distance = abs(DIRECTIONS.index(predicted) - DIRECTIONS.index(actual)) if predicted in DIRECTIONS else None
    predicted_change = grade.get("predicted_backdown") in ("partial", "full")
    actual_change = rubric["backdown"]["kind"] in ("partial", "full")
    return {
        "direction_match": predicted == actual,
        "direction_distance": distance,
        "objection_recall": round(recall, 3),
        "objection_verdicts": verdicts,
        "backdown_predicted": grade.get("predicted_backdown"),
        # a report that does not say what the organisation will do gets no credit either way
        "backdown_match": grade.get("predicted_backdown") != "unclear" and predicted_change == actual_change,
        "backdown_kind_match": grade.get("predicted_backdown") == rubric["backdown"]["kind"],
        "group_recall": None if group_recall is None else round(group_recall, 3),
        "unmatched": unmatched,
    }


def mean(values: list) -> float | None:
    values = [v for v in values if v is not None]
    return round(sum(values) / len(values), 3) if values else None


def graded_entries(case: dict, key: str) -> list[dict]:
    """A case's simulation runs (`runs`, a list) or its zero-shot answer (`zero_shot`, one entry), when graded."""
    entries = case.get(key) or []
    if isinstance(entries, dict):
        entries = [entries]
    return [e for e in entries if e.get("scores")]


def summarise(cases: list[dict], key: str = "runs") -> dict:
    """Averages over every graded run (or zero-shot answer), overall and split by whether the model could know."""

    def block(selected: list[dict]) -> dict:
        scores = [e["scores"] for c in selected for e in graded_entries(c, key)]
        return {
            "graded": len(scores),
            "direction_accuracy": mean([float(s["direction_match"]) for s in scores]),
            "objection_recall": mean([s["objection_recall"] for s in scores]),
            "backdown_accuracy": mean([float(s["backdown_match"]) for s in scores]),
            "group_recall": mean([s["group_recall"] for s in scores]),
            "sentiment_direction_accuracy": mean(
                [float(e["sentiment_direction"] == c["rubric"]["direction"]) for c in selected
                 for e in graded_entries(c, key) if e.get("sentiment_direction")]
            ),
        }

    return {
        "all": block(cases),
        "after_cutoff": block([c for c in cases if not c["rubric"].get("known_to_model")]),
        "known_to_model": block([c for c in cases if c["rubric"].get("known_to_model")]),
    }


def baselines(cases: list[dict]) -> dict:
    """What a forecaster that ignores the announcement would score: the most common answer every time."""
    n = len(cases)
    if not n:
        return {}
    directions = [c["rubric"]["direction"] for c in cases]
    majority = max(DIRECTIONS, key=directions.count)
    changed = sum(c["rubric"]["backdown"]["kind"] != "none" for c in cases)
    return {
        "always_negative_direction": round(directions.count("negative") / n, 3),
        "majority_direction": majority,
        "majority_direction_accuracy": round(directions.count(majority) / n, 3),
        "always_no_backdown": round((n - changed) / n, 3),
        "always_backdown": round(changed / n, 3),
    }


# ---------- output ----------


def pct(value: float | None) -> str:
    return "–" if value is None else f"{value * 100:.0f}%"


def write_markdown(path: Path, result: dict) -> None:
    s, b = result["summary"], result["baselines"]
    lines = [
        f"# Tellfold accuracy: {result['label']}",
        "",
        f"Date {result['date']} · commit `{result['git_commit']}` · {len(result['cases'])} cases × "
        f"{result['repeats']} run(s) · agents {result['settings']['agent_model']}, planner "
        f"{result['settings']['planner_model']}, judge {result['judge']['model']}",
        "",
    ]
    if result["dry_run"]:
        lines += [
            "**Dry run.** The simulation used the CLI's offline model and the judge was a word-overlap stand-in. "
            "These numbers test the harness and say nothing about accuracy.",
            "",
        ]
    lines += [
        "| | graded | direction | objection recall | backdown | groups named | direction from sentiment |",
        "|---|---|---|---|---|---|---|",
    ]
    for name, label in (("all", "all cases"), ("after_cutoff", "after the model's cutoff"),
                        ("known_to_model", "known to the model")):
        x = s[name]
        lines.append(
            f"| {label} | {x['graded']} | {pct(x['direction_accuracy'])} | {pct(x['objection_recall'])} | "
            f"{pct(x['backdown_accuracy'])} | {pct(x['group_recall'])} | {pct(x['sentiment_direction_accuracy'])} |"
        )
    if result.get("zero_shot_summary"):
        x = result["zero_shot_summary"]["all"]
        lines.append(
            f"| zero-shot baseline | {x['graded']} | {pct(x['direction_accuracy'])} | {pct(x['objection_recall'])} | "
            f"{pct(x['backdown_accuracy'])} | {pct(x['group_recall'])} | – |"
        )
    if b:
        lines += [
            "",
            "Ignoring the announcement: always \"negative\" gets the direction right "
            f"{pct(b['always_negative_direction'])} of the time; always \"no backdown\" gets the backdown right {pct(b['always_no_backdown'])}.",
        ]
    failed_runs, failed_zero = count_failures(result["cases"])
    if failed_runs or failed_zero:
        lines += ["", f"**Failures:** {failed_runs} simulation run(s) and {failed_zero} zero-shot answer(s) failed; "
                      "the case table shows each error. Failed entries are left out of the averages."]
    lines += ["", "## Cases", "",
              "| case | actual | predicted | objections | backdown (actual / predicted) | zero-shot | error |",
              "|---|---|---|---|---|---|---|"]
    for c in result["cases"]:
        r = c["rubric"]
        zero = _zero_shot_cell(c.get("zero_shot"))
        for run in c["runs"]:
            sc = run.get("scores") or {}
            pred = (run.get("grade") or {}).get("predicted_direction", "–")
            recall = pct(sc.get("objection_recall")) if sc else "–"
            mark = "" if not sc else (" ✓" if sc["direction_match"] else " ✗")
            lines.append(
                f"| {c['slug']} (seed {run['seed']}) | {r['direction']} | {pred}{mark} | {recall} | "
                f"{r['backdown']['kind']} / {sc.get('backdown_predicted', '–')} | {zero} | "
                f"{run.get('error', '')[:80]} |"
            )
    lines += ["", f"Usage: {json.dumps(result['usage'])}", ""]
    path.write_text("\n".join(lines), encoding="utf-8")


def _zero_shot_cell(zs: dict | None) -> str:
    """One case's zero-shot answer for the case table: direction, recall and backdown, or its error."""
    if not zs:
        return ""
    if zs.get("error"):
        return f"failed: {zs['error'][:60]}"
    sc = zs["scores"]
    return (f"{zs['grade']['predicted_direction']}{' ✓' if sc['direction_match'] else ' ✗'}, "
            f"{pct(sc['objection_recall'])}, {sc['backdown_predicted']}")


def results_path(folder: Path, label: str, date: str) -> Path:
    """`<date>-<label>.json`, or `<date>-<label>-2.json` and up when that exists. Labels may contain dots."""
    path, n = folder / f"{date}-{label}.json", 2
    while path.exists():
        path, n = folder / f"{date}-{label}-{n}.json", n + 1
    return path


def git_commit() -> str:
    try:
        return subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True, cwd=REPO,
                              check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def default_tellfold() -> list[str]:
    built = REPO / "engine-rs" / "target" / "release" / "tellfold"
    if built.exists():
        return [str(built)]
    found = shutil.which("tellfold")
    if found:
        return [found]
    raise SystemExit("no tellfold binary: build engine-rs (cargo build --release -p tellfold-cli) or pass --tellfold")


# ---------- main ----------


def make_graders(args: argparse.Namespace, settings: dict) -> tuple[dict, object, object | None, str]:
    """(environment for the CLI, judge, zero-shot model or None, results label) for a dry or a live pass."""
    env = dict(os.environ)
    if args.dry_run:
        env["TELLFOLD_FAKE_LLM"] = "1"
        return env, DryJudge(), DryModel() if args.zero_shot else None, args.label or "dry-run"
    env.pop("TELLFOLD_FAKE_LLM", None)
    judge = LlmJudge(ClaudeModel(args.claude, args.judge_model))
    zero = ClaudeModel(args.claude, settings["planner_model"]) if args.zero_shot else None
    return env, judge, zero, args.label or f"{settings['planner_model']}-{settings['agent_model']}"


def run_once(cmd: list[str], case: dict, out: Path, settings: dict, seed: int, env: dict, judge) -> dict:
    """One simulation of one case, graded. Raises on any failure."""
    rubric = case["rubric"]
    run_dir = run_tellfold(cmd, case, out, settings, seed, env)
    report = json.loads((run_dir / "report.json").read_text(encoding="utf-8"))
    final_round, final = last_sentiment(run_dir)
    run = {
        "run_dir": str(run_dir.relative_to(REPO) if run_dir.is_relative_to(REPO) else run_dir),
        **count_calls(run_dir),
        "report_confidence": report.get("confidence"),
        "final_round": final_round,
        "final_sentiment": final,
        "sentiment_direction": sentiment_direction(final),
    }
    if final_round is not None and final is None:
        run["sentiment_note"] = f"round {final_round}, the last, has no sentiment score"
    run["grade"] = judge.grade(rubric, report, final)
    run["scores"] = score(rubric, run["grade"])
    return run


def run_case(case: dict, cmd: list[str], runs_root: Path, settings: dict, repeats: int, env: dict, judge) -> dict:
    """Every seed of one case. A failed run is recorded with its error and the pass goes on."""
    rubric = case["rubric"]
    entry = {"slug": case["slug"], "rubric": rubric, "runs": []}
    for seed in range(FIRST_SEED, FIRST_SEED + repeats):
        print(f"[{case['slug']}] seed {seed}: simulating...", flush=True)
        try:
            run = {"seed": seed, **run_once(cmd, case, runs_root / case["slug"] / f"seed-{seed}", settings, seed,
                                            env, judge)}
        # one failed case must not stop a three-hour pass; the error goes into the results and the exit code
        except Exception as e:  # noqa: BLE001
            run = {"seed": seed, "error": str(e)}
            print(f"[{case['slug']}] failed: {e}", file=sys.stderr, flush=True)
        else:
            sc = run["scores"]
            print(f"[{case['slug']}] direction {run['grade']['predicted_direction']} (actual "
                  f"{rubric['direction']}), objections {pct(sc['objection_recall'])}, backdown "
                  f"{sc['backdown_predicted']} (actual {rubric['backdown']['kind']})", flush=True)
            if sc["unmatched"]:
                print(f"[{case['slug']}] warning: judge returned unknown ids or groups {sc['unmatched']}",
                      file=sys.stderr, flush=True)
        entry["runs"].append(run)
    return entry


def run_zero_shot(case: dict, prompt: tuple[str, str], model, judge) -> dict:
    """The no-crowd baseline for one case: one direct call with the same seed and question, graded the same way."""
    system, template = prompt
    user = template.replace("{{SEED}}", case["input"].read_text(encoding="utf-8")).replace(
        "{{QUESTION}}", case["rubric"]["question"]
    )
    zs: dict = {}
    try:
        zs["answer"] = model.call(system, user, ZERO_SHOT_SCHEMA)
        zs["grade"] = judge.grade(case["rubric"], zs["answer"], None)
        zs["scores"] = score(case["rubric"], zs["grade"])
    # same as run_case: a failed baseline call is recorded and counted, not fatal
    except Exception as e:  # noqa: BLE001
        zs["error"] = str(e)
        print(f"[{case['slug']}] zero-shot failed: {e}", file=sys.stderr, flush=True)
    return zs


def build_result(args: argparse.Namespace, settings: dict, cmd: list[str], label: str, date: str,
                 results: list[dict], judge, zero_model) -> dict:
    return {
        "benchmark": "tellfold-accuracy",
        "version": 2,
        "label": label,
        "date": date,
        "dry_run": args.dry_run,
        "git_commit": git_commit(),
        "tellfold": cmd,
        "settings": settings,
        "repeats": args.repeats,
        "judge": {"model": "word-overlap (dry run)" if args.dry_run else args.judge_model,
                  "prompt": "bench/judge_prompt.md"},
        "summary": summarise(results),
        "zero_shot_summary": summarise(results, "zero_shot") if zero_model is not None else None,
        "baselines": baselines(results),
        "usage": {
            "simulation_calls": sum(r.get("calls", 0) for c in results for r in c["runs"]),
            "judge": judge.usage,
            "zero_shot": zero_model.usage if zero_model else None,
        },
        "cases": results,
    }


def count_failures(results: list[dict]) -> tuple[int, int]:
    """(failed simulation runs, failed zero-shot answers)."""
    runs = sum(1 for c in results for r in c["runs"] if r.get("error"))
    zero = sum(1 for c in results if (c.get("zero_shot") or {}).get("error"))
    return runs, zero


def parse_args(argv: list[str] | None) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--dry-run", action="store_true", help="offline model and word-overlap judge; costs nothing")
    p.add_argument("--estimate", action="store_true", help="print the calls and cost of this pass, run nothing")
    p.add_argument("--validate", action="store_true", help="check the case files, run nothing")
    p.add_argument("--cases", help="comma-separated slugs (default: every case)")
    p.add_argument("--repeats", type=int, default=1, help="runs per case, seeds 7, 8, 9...")
    p.add_argument("--zero-shot", action="store_true", help="also grade one direct planner-model answer per case")
    p.add_argument("--tellfold", help='command that runs the CLI, e.g. "uv run tellfold" (default: engine-rs build)')
    p.add_argument("--claude", default="claude", help="Claude Code CLI for the judge and zero-shot calls")
    p.add_argument("--judge-model", default="sonnet")
    p.add_argument("--agent-model", default=SETTINGS["agent_model"])
    p.add_argument("--planner-model", default=SETTINGS["planner_model"])
    p.add_argument("--label", help="results file label (default: <planner>-<agent>, or dry-run)")
    p.add_argument("--cases-dir", type=Path, default=CASES_DIR)
    p.add_argument("--results-dir", type=Path, help="default: bench/results, or bench/runs/results for a dry run")
    p.add_argument("--runs-dir", type=Path, default=RUNS_DIR)
    args = p.parse_args(argv)
    # the CLI runs with cwd=REPO, so every path it is given must be absolute
    args.cases_dir = args.cases_dir.resolve()
    args.runs_dir = args.runs_dir.resolve()
    args.results_dir = (args.results_dir or (DRY_RESULTS_DIR if args.dry_run else RESULTS_DIR)).resolve()
    return args


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    only = [s.strip() for s in args.cases.split(",") if s.strip()] if args.cases else None
    cases = load_cases(only, args.cases_dir)
    settings = {**SETTINGS, "agent_model": args.agent_model, "planner_model": args.planner_model}
    if args.validate:
        return 0 if validate_all(cases) else 1
    if args.estimate:
        print_estimate(estimate(cases, settings, args.repeats, args.zero_shot, args.judge_model))
        return 0
    if not validate_all(cases):
        return 1

    cmd = shlex.split(args.tellfold) if args.tellfold else default_tellfold()
    env, judge, zero_model, label = make_graders(args, settings)
    zero_prompt = split_prompt(BENCH / "zero_shot_prompt.md") if zero_model is not None else None
    date = dt.date.today().isoformat()
    args.results_dir.mkdir(parents=True, exist_ok=True)
    out_json = results_path(args.results_dir, label, date)
    runs_root = args.runs_dir / out_json.stem

    results = []
    for case in cases:
        entry = run_case(case, cmd, runs_root, settings, args.repeats, env, judge)
        if zero_model is not None:
            entry["zero_shot"] = run_zero_shot(case, zero_prompt, zero_model, judge)
        results.append(entry)

    result = build_result(args, settings, cmd, label, date, results, judge, zero_model)
    out_json.write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    write_markdown(out_json.with_suffix(".md"), result)
    failed_runs, failed_zero = count_failures(results)
    print(f"wrote {out_json} and {out_json.with_suffix('.md').name}; {failed_runs} failed run(s), "
          f"{failed_zero} failed zero-shot answer(s)")
    return 1 if failed_runs or failed_zero else 0


if __name__ == "__main__":
    sys.exit(main())
