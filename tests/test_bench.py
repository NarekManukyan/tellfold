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
    # crowd rules 2 and later add the stakes plan and a fact check a round, both on the planner model
    ruled = bench.estimate_calls(20, 6, 6, 1, crowd_rules=3)
    assert ruled == {**calls, "personas.stakes": 1, "engine.factcheck": 7}
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
    assert result["usage"]["simulation_calls"] == 0  # the Python CLI's offline model writes no calls.jsonl
    assert "Dry run" in out.with_suffix(".md").read_text()


def test_dry_run_results_stay_out_of_bench_results():
    args = bench.parse_args(["--dry-run"])
    assert args.results_dir == bench.DRY_RESULTS_DIR
    assert bench.parse_args([]).results_dir == bench.RESULTS_DIR


FAKE_CLAUDE = """#!/usr/bin/env python3
import json, sys
from pathlib import Path
log = Path(__file__).with_name("calls.jsonl")
calls = log.read_text().splitlines() if log.exists() else []
with log.open("a") as fh:
    fh.write(json.dumps({"argv": sys.argv[1:], "stdin": sys.stdin.read()}) + "\\n")
if not calls and Path(__file__).with_name("fail-first").exists():
    print("[]")  # not an object: the harness must retry
    sys.exit(0)
print(json.dumps({"structured_output": %s, "usage": {"input_tokens": 10, "output_tokens": 5},
                  "total_cost_usd": 0.01}))
"""


def fake_claude(tmp_path, reply: dict, fail_first: bool = False) -> Path:
    script = tmp_path / "claude"
    script.write_text(FAKE_CLAUDE % repr(reply))
    script.chmod(0o755)
    if fail_first:
        (tmp_path / "fail-first").touch()
    return script


def read_calls(tmp_path) -> list[dict]:
    return [json.loads(line) for line in (tmp_path / "calls.jsonl").read_text().splitlines()]


def test_llm_judge_grades_blind(tmp_path):
    """The real grading path with a fake CLI: what the judge is shown, the per-case schema, and the parsed reply."""
    (case,) = bench.load_cases(folder_root=FIXTURE_CASES)
    rubric = case["rubric"]
    reply = {
        "predicted_direction": "negative", "direction_evidence": "q", "predicted_backdown": "partial",
        "backdown_evidence": "q", "extra_objections": [],
        "objections": [{"id": "O1", "verdict": "covered", "evidence": "q"}],
        "groups": [{"group": "Regular  customers", "named": True}, {"group": "teachers", "named": True}],
    }
    judge = bench.LlmJudge(bench.ClaudeModel(str(fake_claude(tmp_path, reply)), "sonnet"))
    report = {"title": "T", "summary": "Regulars grumble about the coffee price.", "prediction": "P",
              "camps": [{"name": "Regulars", "summary": "s", "members": ["a1"]}], "citations": ["x"]}
    grade = judge.grade(case, report, -0.5)
    assert grade == reply
    (call,) = read_calls(tmp_path)
    shown = call["stdin"]
    # the judge never sees the answer it is grading against
    assert '"direction"' not in shown and "backdown" not in shown.split("REPORT")[0].lower()
    assert rubric["direction"] not in shown.split("REPORT")[0].split("CHECKLIST")[1]
    assert "{{" not in shown and "{{" not in call["argv"][call["argv"].index("--system-prompt") + 1]
    # the seed's own headline, never the rubric title written with hindsight
    assert "A note from Linden & Rye about our prices" in shown and rubric["title"] not in shown
    assert "Closing on Tuesdays is inconvenient for regulars" in shown and "students" in shown
    assert "Regulars grumble about the coffee price." in shown and "citations" not in shown and "a1" not in shown
    argv = call["argv"]
    system = argv[argv.index("--system-prompt") + 1]
    assert system.startswith("You are a strict, literal grader")
    schema = json.loads(argv[argv.index("--json-schema") + 1])
    item = schema["properties"]["objections"]["items"]["properties"]
    assert item["id"]["enum"] == ["o1", "o2", "o3"]
    assert schema["properties"]["groups"]["items"]["properties"]["group"]["enum"] == rubric["pushback_groups"]
    assert argv[argv.index("--model") + 1] == "sonnet"
    assert judge.usage == {"calls": 1, "input_tokens": 10, "output_tokens": 5, "cost_usd": 0.01}
    # ids and groups match ignoring case and spacing; what matches nothing is reported
    s = bench.score(rubric, grade)
    assert s["objection_verdicts"] == ["covered", "missed", "missed"]
    assert s["group_recall"] == 0.5
    assert s["unmatched"] == ["teachers"]


def test_judge_checklist_has_no_answer():
    (case,) = bench.load_cases(folder_root=FIXTURE_CASES)
    shown = bench.judge_checklist(case)
    assert set(shown) == {"announcement", "question", "objections", "pushback_groups"}
    assert all(set(o) == {"id", "text"} for o in shown["objections"])


def test_a_case_without_groups_shows_no_trace_of_them(tmp_path):
    """An empty group list would tell the blind judge that nobody pushed back, so groups vanish from the prompt."""
    (case,) = bench.load_cases(folder_root=FIXTURE_CASES)
    case = {**case, "rubric": {**case["rubric"], "pushback_groups": []}}
    reply = {"predicted_direction": "positive", "predicted_backdown": "none",
             "objections": [{"id": "o1", "verdict": "missed", "evidence": ""}]}
    judge = bench.LlmJudge(bench.ClaudeModel(str(fake_claude(tmp_path, reply)), "sonnet"))
    grade = judge.grade(case, {"summary": "S"}, None)
    (call,) = read_calls(tmp_path)
    argv = call["argv"]
    system = argv[argv.index("--system-prompt") + 1]
    schema = json.loads(argv[argv.index("--json-schema") + 1])
    # the question template ("which groups will push back") and "a different group" in the objection rule are the same
    # for every case; only the group checks must vanish
    for text in (system, call["stdin"].replace(case["rubric"]["question"], "")):
        assert "groups" not in text.lower() and "pushback group" not in text.lower() and "{{" not in text
    assert "groups" not in schema["properties"] and "groups" not in schema["required"]
    assert bench.score(case["rubric"], grade)["group_recall"] is None


def test_claude_model_retries_a_timeout(tmp_path):
    script = tmp_path / "claude"
    script.write_text(
        "#!/usr/bin/env python3\nimport json, sys, time\nfrom pathlib import Path\n"
        "mark = Path(__file__).with_name('called')\n"
        "if not mark.exists():\n    mark.touch()\n    time.sleep(5)\n"
        "print(json.dumps({'structured_output': {'ok': 1}}))\n"
    )
    script.chmod(0o755)
    model = bench.ClaudeModel(str(script), "haiku", timeout_s=1)
    assert model.call("sys", "user", {"type": "object", "required": ["ok"]}) == {"ok": 1}


def test_claude_model_retries_a_reply_missing_a_required_key(tmp_path):
    model = bench.ClaudeModel(str(fake_claude(tmp_path, {"other": 1})), "haiku")
    with pytest.raises(RuntimeError, match="no ok"):
        model.call("sys", "user", {"type": "object", "required": ["ok"]})
    assert len(read_calls(tmp_path)) == 2


def test_claude_model_retries_a_bad_reply(tmp_path):
    model = bench.ClaudeModel(str(fake_claude(tmp_path, {"ok": 1}, fail_first=True)), "haiku")
    assert model.call("sys", "user", {"type": "object"}) == {"ok": 1}
    assert len(read_calls(tmp_path)) == 2


def test_results_path_keeps_dotted_labels(tmp_path):
    first = bench.results_path(tmp_path, "sonnet-5.5", "2026-11-11")
    assert first.name == "2026-11-11-sonnet-5.5.json"
    first.write_text("{}")
    assert bench.results_path(tmp_path, "sonnet-5.5", "2026-11-11").name == "2026-11-11-sonnet-5.5-2.json"


def test_validate_reports_null_dates_and_benchmark_words(tmp_path):
    folder = tmp_path / "corner-cafe-prices"
    folder.mkdir()
    (folder / "input.md").write_text("Written for the accuracy test. " + "word " * 300)
    (folder / "outcome.md").write_text("https://a.example https://b.example")
    rubric = json.loads((FIXTURE_CASES / "corner-cafe-prices" / "rubric.json").read_text())
    for bad in ({"announced_on": None, "model_cutoff": None}, {"announced_on": 20260801, "model_cutoff": 2026}):
        (folder / "rubric.json").write_text(json.dumps({**rubric, **bad}))
        errors, _ = bench.validate_case(bench.load_cases(folder_root=tmp_path)[0])
        assert any("bad date" in e for e in errors), bad
        assert any("accuracy test" in e for e in errors)


def test_the_word_benchmark_only_warns(tmp_path):
    folder = tmp_path / "corner-cafe-prices"
    folder.mkdir()
    (folder / "input.md").write_text("The bank cuts its benchmark rate to 3%. " + "word " * 300)
    (folder / "outcome.md").write_text("https://a.example https://b.example")
    (folder / "rubric.json").write_text((FIXTURE_CASES / "corner-cafe-prices" / "rubric.json").read_text())
    errors, warnings = bench.validate_case(bench.load_cases(folder_root=tmp_path)[0])
    assert errors == []
    assert any("benchmark" in w for w in warnings)


def test_unknown_model_has_no_price():
    assert bench.price_for("claude-haiku-5-5") == bench.PRICES["haiku"]
    with pytest.raises(SystemExit):
        bench.price_for("gpt-9")


def test_a_reply_that_mentions_a_network_is_kept(tmp_path):
    # judge and zero-shot answers about bus, rail or social networks used to be taken for a dropped connection and
    # asked again up to six times
    model = bench.ClaudeModel(str(fake_claude(tmp_path, {"ok": "riders on the bus network ECONNRESET"})), "haiku")
    assert model.call("sys", "user", {"type": "object", "required": ["ok"]})["ok"].startswith("riders")
    assert len(read_calls(tmp_path)) == 1


def test_a_saved_zero_shot_answer_is_kept_only_when_resuming(tmp_path):
    saved = tmp_path / "zero-shot.json"
    saved.write_text(json.dumps({"answer": "old"}))

    class Model:
        calls = 0

        def call(self, system, user, schema):
            Model.calls += 1
            return {"summary": "new"}

    class Judge:
        def grade(self, case, answer, final):
            return {}

    case = bench.load_cases(folder_root=FIXTURE_CASES)[0]
    assert bench.run_zero_shot(case, ("sys", "{{SEED}} {{QUESTION}}"), Model(), Judge(), saved, reuse=True) == {
        "answer": "old"
    }
    assert Model.calls == 0
    bench.run_zero_shot(case, ("sys", "{{SEED}} {{QUESTION}}"), Model(), Judge(), saved)
    assert Model.calls == 1
    assert json.loads(saved.read_text())["answer"] == {"summary": "new"}


def test_a_pass_records_its_crowd_rules_and_warns_when_they_differ(capsys):
    assert bench.crowd_rules_of([{"runs": [{"crowd_rules": 3}, {"crowd_rules": 3}]}]) == [3]
    assert bench.crowd_rules_of([{"runs": [{"crowd_rules": 2}]}, {"runs": [{"crowd_rules": 3}, {}]}]) == [2, 3]
    assert "more than one crowd rules version" in capsys.readouterr().err
