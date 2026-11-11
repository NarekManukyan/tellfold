"""The accuracy benchmark harness (`bench/run.py`): case files, scoring, the estimate and a whole dry run."""

import importlib.util
import json
import sys
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parent.parent
FIXTURE_CASES = REPO / "tests" / "fixtures" / "bench_case"

spec = importlib.util.spec_from_file_location("bench_run", REPO / "bench" / "run.py")
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)


def test_every_case_in_the_repo_is_valid():
    cases = bench.load_cases()
    assert cases, "bench/cases is empty"
    for case in cases:
        errors, _ = bench.validate_case(case)
        assert errors == [], f"{case['slug']}: {errors}"


def test_validate_catches_a_bad_rubric(tmp_path):
    folder = tmp_path / "bad-case"
    folder.mkdir()
    (folder / "input.md").write_text("word " * 300)
    (folder / "outcome.md").write_text("https://a.example https://b.example")
    rubric = json.loads((FIXTURE_CASES / "corner-cafe-prices" / "rubric.json").read_text())
    rubric.update(slug="bad-case", direction="angry", objections=rubric["objections"][:2], known_to_model=True)
    (folder / "rubric.json").write_text(json.dumps(rubric))
    errors, _ = bench.validate_case(bench.load_cases(folder_root=tmp_path)[0])
    assert any("direction" in e for e in errors)
    assert any("3-5" in e for e in errors)
    assert any("known_to_model" in e for e in errors)


def test_score():
    rubric = json.loads((FIXTURE_CASES / "corner-cafe-prices" / "rubric.json").read_text())
    grade = {
        "predicted_direction": "negative",
        "predicted_backdown": "none",
        "objections": [
            {"id": "o1", "verdict": "covered", "evidence": "x"},
            {"id": "o2", "verdict": "partial", "evidence": "y"},
        ],  # o3 left out: counts as missed
        "groups": [{"group": "regular customers", "named": True}, {"group": "students", "named": False}],
    }
    s = bench.score(rubric, grade)
    assert s["direction_match"] is False and s["direction_distance"] == 1
    assert s["objection_recall"] == 0.5
    assert s["objection_verdicts"] == ["covered", "partial", "missed"]
    assert s["backdown_match"] is True and s["backdown_kind_match"] is True
    assert s["group_recall"] == 0.5
    assert bench.score(rubric, {**grade, "predicted_backdown": "unclear"})["backdown_match"] is False


def test_estimate_matches_the_engine_formula():
    # 20 agents, 6 rounds, batch 6, one chunk: the Rust estimate is 20 agent calls and 4 planner calls
    calls = bench.estimate_calls(20, 6, 6, 1)
    agent = sum(n for p, n in calls.items() if p.startswith("engine."))
    planner = sum(n for p, n in calls.items() if not p.startswith("engine."))
    assert (agent, planner) == (20, 4)
    case = bench.load_cases(folder_root=FIXTURE_CASES)
    est = bench.estimate(case, bench.SETTINGS, repeats=2, zero_shot=True, judge_model="sonnet")
    assert est["rows"]["judge"]["calls"] == 3
    assert est["total"]["usd"] > 0


def test_extract_json():
    assert bench.extract_json('Here you go: {"a": {"b": 1}} trailing') == {"a": {"b": 1}}
    with pytest.raises(ValueError):
        bench.extract_json("no json")


def test_dry_run_end_to_end(tmp_path):
    """The whole pass on the Python CLI's offline model: run, grade, results JSON and Markdown."""
    code = bench.main([
        "--dry-run", "--zero-shot", "--repeats", "2",
        "--tellfold", f"{sys.executable} -m tellfold",
        "--cases-dir", str(FIXTURE_CASES),
        "--results-dir", str(tmp_path / "results"),
        "--runs-dir", str(tmp_path / "runs"),
    ])
    assert code == 0
    (out,) = (tmp_path / "results").glob("*-dry-run.json")
    result = json.loads(out.read_text())
    assert result["dry_run"] is True
    runs = result["cases"][0]["runs"]
    assert [r["seed"] for r in runs] == [7, 8]
    assert all("scores" in r and "error" not in r for r in runs)
    assert result["summary"]["all"]["graded"] == 2
    assert result["zero_shot_summary"]["all"]["graded"] == 1
    assert "simulation_calls" in result["usage"]  # the offline model writes no calls.jsonl
    assert "Dry run" in out.with_suffix(".md").read_text()
