//! The accuracy benchmark harness: case files, scoring, the estimate, the judge through a fake Claude Code CLI and
//! resuming. The whole dry run on the real CLI is in `tellfold-cli/tests/bench.rs`, which has the binary.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use tellfold_bench::cases::{load_cases, validate_case, Case};
use tellfold_bench::estimate::{estimate, estimate_calls, price_for, PRICES};
use tellfold_bench::judge::{extract_json, judge_checklist, ClaudeModel, Judge, LlmJudge, Model, Usage};
use tellfold_bench::output::results_path;
use tellfold_bench::run::run_zero_shot;
use tellfold_bench::score::{crowd_rules_of, score};
use tellfold_bench::{parse_args, Settings};

fn fixture_cases() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixture_rubric() -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixture_cases().join("corner-cafe-prices/rubric.json")).unwrap())
        .unwrap()
}

fn the_case() -> Case {
    let mut cases = load_cases(None, &fixture_cases()).unwrap();
    assert_eq!(cases.len(), 1);
    cases.remove(0)
}

fn judge_prompt() -> PathBuf {
    tellfold_bench::bench_dir().join("judge_prompt.md")
}

/// A case folder in `root` with this seed, two sources and this rubric.
fn write_case(root: &Path, slug: &str, input: &str, rubric: &Value) -> Case {
    let folder = root.join(slug);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("input.md"), input).unwrap();
    std::fs::write(folder.join("outcome.md"), "https://a.example https://b.example").unwrap();
    std::fs::write(folder.join("rubric.json"), rubric.to_string()).unwrap();
    load_cases(None, root).unwrap().remove(0)
}

#[test]
fn every_case_in_the_repo_is_valid() {
    let cases = load_cases(None, &tellfold_bench::cases_dir()).unwrap();
    assert!(!cases.is_empty(), "bench/cases is empty");
    for case in &cases {
        let (errors, _) = validate_case(case);
        assert!(errors.is_empty(), "{}: {errors:?}", case.slug);
    }
}

#[test]
fn validate_catches_a_bad_rubric() {
    let dir = tempfile::tempdir().unwrap();
    let mut rubric = fixture_rubric();
    rubric["slug"] = json!("bad-case");
    rubric["direction"] = json!("angry");
    rubric["objections"] = json!(rubric["objections"].as_array().unwrap()[..2]);
    rubric["known_to_model"] = json!(true);
    let case = write_case(dir.path(), "bad-case", &"word ".repeat(300), &rubric);
    let (errors, _) = validate_case(&case);
    assert!(errors.iter().any(|e| e.contains("direction")), "{errors:?}");
    assert!(errors.iter().any(|e| e.contains("3-5")), "{errors:?}");
    assert!(errors.iter().any(|e| e.contains("known_to_model")), "{errors:?}");
}

#[test]
fn score_compares_the_grade_with_the_rubric() {
    let rubric = fixture_rubric();
    let grade = json!({
        "predicted_direction": "negative",
        "predicted_backdown": "none",
        "objections": [
            {"id": "o1", "verdict": "covered", "evidence": "x"},
            {"id": "o2", "verdict": "partial", "evidence": "y"},
        ],  // o3 left out: counts as missed
        "groups": [{"group": "regular customers", "named": true}, {"group": "students", "named": false}],
    });
    let s = score(&rubric, &grade).unwrap();
    assert_eq!((s["direction_match"].clone(), s["direction_distance"].clone()), (json!(false), json!(1)));
    assert_eq!(s["objection_recall"], json!(0.5));
    assert_eq!(s["objection_verdicts"], json!(["covered", "partial", "missed"]));
    assert_eq!((s["backdown_match"].clone(), s["backdown_kind_match"].clone()), (json!(true), json!(true)));
    assert_eq!(s["group_recall"], json!(0.5));
    let mut unclear = grade.clone();
    unclear["predicted_backdown"] = json!("unclear");
    assert_eq!(score(&rubric, &unclear).unwrap()["backdown_match"], json!(false));
}

#[test]
fn estimate_matches_the_engine_formula() {
    // 20 agents, 6 rounds, batch 6, one chunk: the Rust estimate is 20 agent calls and 4 planner calls
    let calls = estimate_calls(20, 6, 6, 1, 0);
    let agent: i64 = calls.iter().filter(|(p, _)| p.starts_with("engine.")).map(|(_, n)| n).sum();
    let planner: i64 = calls.iter().filter(|(p, _)| !p.starts_with("engine.")).map(|(_, n)| n).sum();
    assert_eq!((agent, planner), (20, 4));
    // crowd rules 2 and later add the stakes plan and a fact check a round, both on the planner model
    let ruled = estimate_calls(20, 6, 6, 1, 3);
    let mut expected = calls.clone();
    expected.extend([("personas.stakes", 1), ("engine.factcheck", 7)]);
    assert_eq!(ruled, expected);
    let est = estimate(&[the_case()], &Settings::default(), 2, true, "sonnet").unwrap();
    assert_eq!(est["rows"]["judge"]["calls"], json!(3));
    assert!(est["total"]["usd"].as_f64().unwrap() > 0.0);
}

#[test]
fn the_estimate_is_the_engines_own_for_every_crowd_rules_version() {
    // the same run through tellfold-store's estimate_call_split, which the Mac app mirrors
    for crowd_rules in 0..=tellfold_models::CROWD_RULES {
        for (agents, rounds, batch, chunks) in [(20, 6, 6, 1), (7, 3, 2, 9), (50, 10, 6, 80), (1, 1, 1, 1)] {
            let mut config = tellfold_models::SimConfig::new("q", Vec::new());
            config.num_agents = agents;
            config.rounds = rounds;
            config.batch_size = batch;
            config.crowd_rules = crowd_rules;
            let (agent, planner) = tellfold_store::estimate::estimate_call_split(&config, chunks);
            let calls = estimate_calls(agents, rounds, batch, chunks, crowd_rules);
            let on_agents = |p: &str| p.starts_with("engine.") && p != "engine.factcheck";
            let bench_agent: i64 = calls.iter().filter(|(p, _)| on_agents(p)).map(|(_, n)| n).sum();
            let bench_planner: i64 = calls.iter().filter(|(p, _)| !on_agents(p)).map(|(_, n)| n).sum();
            assert_eq!(
                (bench_agent, bench_planner),
                (agent, planner),
                "{crowd_rules} {agents} {rounds} {batch} {chunks}"
            );
        }
    }
}

#[test]
fn extract_json_skips_text_around_the_object() {
    assert_eq!(extract_json(r#"Here you go: {"a": {"b": 1}} trailing"#).unwrap(), json!({"a": {"b": 1}}));
    assert!(extract_json("no json").is_err());
}

#[test]
fn dry_run_results_stay_out_of_bench_results() {
    let args = parse_args(&["--dry-run".into()]).unwrap();
    assert_eq!(args.results_dir, tellfold_bench::resolve(&tellfold_bench::dry_results_dir()));
    assert_eq!(parse_args(&[]).unwrap().results_dir, tellfold_bench::resolve(&tellfold_bench::results_dir()));
}

#[test]
fn every_path_is_made_absolute_including_resume() {
    let args = parse_args(&["--resume".into(), "bench/runs/x".into(), "--runs-dir=rel".into()]).unwrap();
    let resume = args.resume.unwrap();
    assert!(resume.is_absolute() && resume.ends_with("bench/runs/x"), "{}", resume.display());
    assert!(args.runs_dir.is_absolute() && args.runs_dir.ends_with("rel"));
    assert!(args.cases_dir.is_absolute());
}

// ---------- the judge through a fake Claude Code CLI ----------

/// A stand-in for `claude`: logs every call's arguments (NUL-separated) and stdin as `call-<n>.argv` and
/// `call-<n>.stdin`, then prints `reply.json` as the structured output. With `fail-first`, the first call prints a
/// JSON array instead, which the harness must retry.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
dir=$(dirname "$0")
n=$(ls "$dir" | grep -c '^call-.*\.argv$')
printf '%s\0' "$@" > "$dir/call-$n.argv"
cat > "$dir/call-$n.stdin"
if [ "$n" = 0 ] && [ -e "$dir/fail-first" ]; then
    echo '[]'
    exit 0
fi
printf '{"structured_output": %s, "usage": {"input_tokens": 10, "output_tokens": 5}, "total_cost_usd": 0.01}\n' \
    "$(cat "$dir/reply.json")"
"#;

fn script(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn fake_claude(dir: &Path, reply: &Value, fail_first: bool) -> String {
    std::fs::write(dir.join("reply.json"), reply.to_string()).unwrap();
    if fail_first {
        std::fs::write(dir.join("fail-first"), "").unwrap();
    }
    script(dir, "claude", FAKE_CLAUDE).display().to_string()
}

struct Call {
    argv: Vec<String>,
    stdin: String,
}

impl Call {
    fn flag(&self, name: &str) -> &str {
        let at = self.argv.iter().position(|a| a == name).unwrap_or_else(|| panic!("no {name}"));
        &self.argv[at + 1]
    }
}

fn read_calls(dir: &Path) -> Vec<Call> {
    let mut calls = BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if let Some(n) = name.strip_prefix("call-").and_then(|n| n.strip_suffix(".argv")) {
            let argv = std::fs::read_to_string(&path).unwrap();
            let argv: Vec<String> = argv.split_terminator('\0').map(String::from).collect();
            let stdin = std::fs::read_to_string(dir.join(format!("call-{n}.stdin"))).unwrap();
            calls.insert(n.parse::<usize>().unwrap(), Call { argv, stdin });
        }
    }
    calls.into_values().collect()
}

#[test]
fn llm_judge_grades_blind() {
    let dir = tempfile::tempdir().unwrap();
    let case = the_case();
    let rubric = &case.rubric;
    let reply = json!({
        "predicted_direction": "negative", "direction_evidence": "q", "predicted_backdown": "partial",
        "backdown_evidence": "q", "extra_objections": [],
        "objections": [{"id": "O1", "verdict": "covered", "evidence": "q"}],
        "groups": [{"group": "Regular  customers", "named": true}, {"group": "teachers", "named": true}],
    });
    let mut judge =
        LlmJudge::new(ClaudeModel::new(&fake_claude(dir.path(), &reply, false), "sonnet"), &judge_prompt()).unwrap();
    let report = json!({"title": "T", "summary": "Regulars grumble about the coffee price.", "prediction": "P",
                        "camps": [{"name": "Regulars", "summary": "s", "members": ["a1"]}], "citations": ["x"]});
    let grade = judge.grade(&case, &report, Some(-0.5)).unwrap();
    assert_eq!(grade, reply);
    let calls = read_calls(dir.path());
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    let shown = &call.stdin;
    let before_report = shown.split("REPORT").next().unwrap();
    // the judge never sees the answer it is grading against
    assert!(!shown.contains("\"direction\"") && !before_report.to_lowercase().contains("backdown"));
    let checklist = before_report.split("CHECKLIST").nth(1).unwrap();
    assert!(!checklist.contains(rubric["direction"].as_str().unwrap()));
    assert!(!shown.contains("{{") && !call.flag("--system-prompt").contains("{{"));
    // the seed's own headline, never the rubric title written with hindsight
    assert!(shown.contains("A note from Linden & Rye about our prices"));
    assert!(!shown.contains(rubric["title"].as_str().unwrap()));
    assert!(shown.contains("Closing on Tuesdays is inconvenient for regulars") && shown.contains("students"));
    assert!(shown.contains("Regulars grumble about the coffee price."));
    assert!(!shown.contains("citations") && !shown.contains("a1"));
    assert!(call.flag("--system-prompt").starts_with("You are a strict, literal grader"));
    let schema: Value = serde_json::from_str(call.flag("--json-schema")).unwrap();
    assert_eq!(schema["properties"]["objections"]["items"]["properties"]["id"]["enum"], json!(["o1", "o2", "o3"]));
    assert_eq!(schema["properties"]["groups"]["items"]["properties"]["group"]["enum"], rubric["pushback_groups"]);
    assert_eq!(call.flag("--model"), "sonnet");
    assert_eq!(judge.usage(), Usage { calls: 1, input_tokens: 10, output_tokens: 5, cost_usd: 0.01 });
    // ids and groups match ignoring case and spacing; what matches nothing is reported
    let s = score(rubric, &grade).unwrap();
    assert_eq!(s["objection_verdicts"], json!(["covered", "missed", "missed"]));
    assert_eq!(s["group_recall"], json!(0.5));
    assert_eq!(s["unmatched"], json!(["teachers"]));
}

#[test]
fn judge_checklist_has_no_answer() {
    let shown = judge_checklist(&the_case());
    let keys: Vec<&String> = shown.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["announcement", "question", "objections", "pushback_groups"]);
    for o in shown["objections"].as_array().unwrap() {
        let keys: Vec<&String> = o.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["id", "text"]);
    }
}

#[test]
fn a_case_without_groups_shows_no_trace_of_them() {
    // an empty group list would tell the blind judge that nobody pushed back, so groups vanish from the prompt
    let dir = tempfile::tempdir().unwrap();
    let mut case = the_case();
    case.rubric["pushback_groups"] = json!([]);
    let reply = json!({"predicted_direction": "positive", "predicted_backdown": "none",
                       "objections": [{"id": "o1", "verdict": "missed", "evidence": ""}]});
    let mut judge =
        LlmJudge::new(ClaudeModel::new(&fake_claude(dir.path(), &reply, false), "sonnet"), &judge_prompt()).unwrap();
    let grade = judge.grade(&case, &json!({"summary": "S"}), None).unwrap();
    let calls = read_calls(dir.path());
    let call = &calls[0];
    let schema: Value = serde_json::from_str(call.flag("--json-schema")).unwrap();
    // the question template ("which groups will push back") and "a different group" in the objection rule are the
    // same for every case; only the group checks must vanish
    let user = call.stdin.replace(case.rubric["question"].as_str().unwrap(), "");
    for text in [call.flag("--system-prompt"), user.as_str()] {
        let lower = text.to_lowercase();
        assert!(!lower.contains("groups") && !lower.contains("pushback group") && !text.contains("{{"), "{text}");
    }
    assert!(schema["properties"].get("groups").is_none());
    assert!(!schema["required"].as_array().unwrap().contains(&json!("groups")));
    assert_eq!(score(&case.rubric, &grade).unwrap()["group_recall"], Value::Null);
}

#[test]
fn claude_model_retries_a_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let cli = script(
        dir.path(),
        "claude",
        "#!/bin/sh\nmark=\"$(dirname \"$0\")/called\"\nif [ ! -e \"$mark\" ]; then touch \"$mark\"; sleep 5; fi\n\
         echo '{\"structured_output\": {\"ok\": 1}}'\n",
    );
    let mut model = ClaudeModel::with_timeout(&cli.display().to_string(), "haiku", Duration::from_secs(1));
    let schema = json!({"type": "object", "required": ["ok"]});
    assert_eq!(model.call("sys", "user", &schema).unwrap(), json!({"ok": 1}));
}

#[test]
fn claude_model_retries_a_reply_missing_a_required_key() {
    let dir = tempfile::tempdir().unwrap();
    let mut model = ClaudeModel::new(&fake_claude(dir.path(), &json!({"other": 1}), false), "haiku");
    let err = model.call("sys", "user", &json!({"type": "object", "required": ["ok"]})).unwrap_err();
    assert!(err.contains("no ok"), "{err}");
    assert_eq!(read_calls(dir.path()).len(), 2);
}

#[test]
fn claude_model_retries_a_bad_reply() {
    let dir = tempfile::tempdir().unwrap();
    let mut model = ClaudeModel::new(&fake_claude(dir.path(), &json!({"ok": 1}), true), "haiku");
    assert_eq!(model.call("sys", "user", &json!({"type": "object"})).unwrap(), json!({"ok": 1}));
    assert_eq!(read_calls(dir.path()).len(), 2);
}

#[test]
fn a_reply_that_mentions_a_network_is_kept() {
    // judge and zero-shot answers about bus, rail or social networks used to be taken for a dropped connection and
    // asked again up to six times
    let dir = tempfile::tempdir().unwrap();
    let reply = json!({"ok": "riders on the bus network ECONNRESET"});
    let mut model = ClaudeModel::new(&fake_claude(dir.path(), &reply, false), "haiku");
    let got = model.call("sys", "user", &json!({"type": "object", "required": ["ok"]})).unwrap();
    assert!(got["ok"].as_str().unwrap().starts_with("riders"));
    assert_eq!(read_calls(dir.path()).len(), 1);
}

#[test]
fn a_failed_call_that_mentions_a_network_is_not_taken_for_a_dropped_one_when_it_exits_zero() {
    // the NETWORK_ERRORS check only runs on a non-zero exit: a clean exit with a bad reply is retried once, not
    // waited on
    let dir = tempfile::tempdir().unwrap();
    let cli = script(
        dir.path(),
        "claude",
        "#!/bin/sh\ndir=$(dirname \"$0\")\necho x >> \"$dir/count\"\necho 'ECONNRESET: socket hang up'\n",
    );
    let mut model = ClaudeModel::new(&cli.display().to_string(), "haiku");
    assert!(model.call("sys", "user", &json!({"type": "object"})).is_err());
    let count = std::fs::read_to_string(dir.path().join("count")).unwrap();
    assert_eq!(count.lines().count(), 2);
}

#[test]
fn results_path_keeps_dotted_labels() {
    let dir = tempfile::tempdir().unwrap();
    let first = results_path(dir.path(), "sonnet-5.5", "2026-11-11");
    assert_eq!(first.file_name().unwrap(), "2026-11-11-sonnet-5.5.json");
    std::fs::write(&first, "{}").unwrap();
    assert_eq!(
        results_path(dir.path(), "sonnet-5.5", "2026-11-11").file_name().unwrap(),
        "2026-11-11-sonnet-5.5-2.json"
    );
    assert_eq!(tellfold_bench::output::markdown_path(&first).file_name().unwrap(), "2026-11-11-sonnet-5.5.md");
}

#[test]
fn validate_reports_null_dates_and_benchmark_words() {
    let dir = tempfile::tempdir().unwrap();
    let input = format!("Written for the accuracy test. {}", "word ".repeat(300));
    for bad in
        [json!({"announced_on": null, "model_cutoff": null}), json!({"announced_on": 20260801, "model_cutoff": 2026})]
    {
        let mut rubric = fixture_rubric();
        for (k, v) in bad.as_object().unwrap() {
            rubric[k] = v.clone();
        }
        let case = write_case(dir.path(), "corner-cafe-prices", &input, &rubric);
        let (errors, _) = validate_case(&case);
        assert!(errors.iter().any(|e| e.contains("bad date")), "{bad}: {errors:?}");
        assert!(errors.iter().any(|e| e.contains("accuracy test")), "{errors:?}");
    }
}

#[test]
fn the_word_benchmark_only_warns() {
    let dir = tempfile::tempdir().unwrap();
    let input = format!("The bank cuts its benchmark rate to 3%. {}", "word ".repeat(300));
    let case = write_case(dir.path(), "corner-cafe-prices", &input, &fixture_rubric());
    let (errors, warnings) = validate_case(&case);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(warnings.iter().any(|w| w.contains("benchmark")), "{warnings:?}");
}

#[test]
fn unknown_model_has_no_price() {
    assert_eq!(price_for("claude-haiku-5-5").unwrap(), PRICES.iter().find(|(f, _)| *f == "haiku").unwrap().1);
    assert!(price_for("gpt-9").is_err());
}

struct CountingModel(usize);

impl Model for CountingModel {
    fn call(&mut self, _: &str, _: &str, _: &Value) -> Result<Value, String> {
        self.0 += 1;
        Ok(json!({"summary": "new"}))
    }
    fn usage(&self) -> Usage {
        Usage::default()
    }
}

struct EmptyJudge;

impl Judge for EmptyJudge {
    fn grade(&mut self, _: &Case, _: &Value, _: Option<f64>) -> Result<Value, String> {
        Ok(json!({}))
    }
    fn usage(&self) -> Usage {
        Usage::default()
    }
}

#[test]
fn a_saved_zero_shot_answer_is_kept_only_when_resuming() {
    let dir = tempfile::tempdir().unwrap();
    let saved = dir.path().join("zero-shot.json");
    std::fs::write(&saved, r#"{"answer": "old"}"#).unwrap();
    let case = the_case();
    let prompt = ("sys".to_string(), "{{SEED}} {{QUESTION}}".to_string());
    let mut model = CountingModel(0);
    let kept = run_zero_shot(&case, &prompt, &mut model, &mut EmptyJudge, Some(&saved), true).unwrap();
    assert_eq!(kept, json!({"answer": "old"}));
    assert_eq!(model.0, 0);
    run_zero_shot(&case, &prompt, &mut model, &mut EmptyJudge, Some(&saved), false).unwrap();
    assert_eq!(model.0, 1);
    let written: Value = serde_json::from_str(&std::fs::read_to_string(&saved).unwrap()).unwrap();
    assert_eq!(written["answer"], json!({"summary": "new"}));
}

#[test]
fn a_pass_records_its_crowd_rules_and_warns_when_they_differ() {
    let (versions, warning) = crowd_rules_of(&[json!({"runs": [{"crowd_rules": 3}, {"crowd_rules": 3}]})]);
    assert_eq!((versions, warning), (vec![3], None));
    let (versions, warning) =
        crowd_rules_of(&[json!({"runs": [{"crowd_rules": 2}]}), json!({"runs": [{"crowd_rules": 3}, {}]})]);
    assert_eq!(versions, vec![2, 3]);
    assert!(warning.unwrap().contains("more than one crowd rules version"));
}

#[test]
fn shell_split_reads_the_tellfold_command_like_shlex() {
    use tellfold_bench::shell_split;
    assert_eq!(shell_split("uv run tellfold").unwrap(), ["uv", "run", "tellfold"]);
    assert_eq!(shell_split(r#"'/a b/tellfold' --x "c d" e\ f"#).unwrap(), ["/a b/tellfold", "--x", "c d", "e f"]);
    assert!(shell_split("'open").is_err());
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[test]
fn a_hung_cli_is_killed_with_its_whole_process_group() {
    use tellfold_bench::process::{run_cli, CliEnd};
    let dir = tempfile::tempdir().unwrap();
    // the CLI starts a child of its own (as the engine starts model CLIs) and both hang
    let cli = script(
        dir.path(),
        "tellfold",
        "#!/bin/sh\nsleep 600 &\necho $! > \"$1/child\"\necho $$ > \"$1/parent\"\nwait\n",
    );
    let argv = vec![cli.display().to_string(), dir.path().display().to_string()];
    let env: Vec<_> = std::env::vars_os().collect();
    let end = run_cli(&argv, &dir.path().join("log"), &env, dir.path(), Duration::from_secs(2)).unwrap();
    assert!(matches!(end, CliEnd::TimedOut), "{end:?}");
    let pid = |name: &str| std::fs::read_to_string(dir.path().join(name)).unwrap().trim().parse::<i32>().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert!(!alive(pid("parent")), "the CLI is still running");
    assert!(!alive(pid("child")), "the CLI's child is still running");
}

#[test]
fn a_reply_whose_pipes_stay_open_after_the_cli_exits_counts_as_none() {
    // a process the CLI started keeps stdout open: reading it must not outlast the timeout
    let dir = tempfile::tempdir().unwrap();
    let cli = script(dir.path(), "claude", "#!/bin/sh\nsleep 30 &\necho '{}'\n");
    let started = std::time::Instant::now();
    let out = tellfold_bench::process::run_captured(&[cli.display().to_string()], "", Duration::from_secs(1)).unwrap();
    assert!(out.is_none());
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn a_relative_tellfold_command_is_made_absolute() {
    let cmd = tellfold_bench::absolute_command(vec!["engine-rs/target/debug/tellfold".into(), "-v".into()]);
    assert!(Path::new(&cmd[0]).is_absolute() && cmd[0].ends_with("engine-rs/target/debug/tellfold"));
    assert_eq!(cmd[1], "-v");
    assert_eq!(tellfold_bench::absolute_command(vec!["tellfold".into()]), ["tellfold"]);
}

#[test]
fn a_cli_that_exits_reports_its_code_and_log() {
    use tellfold_bench::process::{run_cli, CliEnd};
    let dir = tempfile::tempdir().unwrap();
    let cli = script(dir.path(), "tellfold", "#!/bin/sh\necho out\necho err >&2\nexit 3\n");
    let log = dir.path().join("log");
    let end = run_cli(&[cli.display().to_string()], &log, &[], dir.path(), Duration::from_secs(30)).unwrap();
    assert!(matches!(end, CliEnd::Exited(3)), "{end:?}");
    assert_eq!(std::fs::read_to_string(log).unwrap(), "out\nerr\n");
}
