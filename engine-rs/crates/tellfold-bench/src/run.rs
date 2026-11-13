//! Running and grading one case: the simulation through the tellfold CLI (resumable, waiting out a dropped network),
//! the judge, and the zero-shot baseline.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use tellfold_models::json::{py_dumps_pretty, py_str};

use crate::cases::Case;
use crate::judge::{zero_shot_schema, Judge, Model};
use crate::output::pct;
use crate::process::{network_error, run_cli, tail, wait_for_network, CliEnd, NETWORK_TRIES};
use crate::score::{mix_direction, score};
use crate::{sentiment_direction, truthy, Settings, FIRST_SEED};

/// How long one simulation, and one rebuilt report, may take before it counts as hung (a dropped network once left
/// five runs waiting on the CLI overnight).
pub const RUN_TIMEOUT: Duration = Duration::from_secs(3 * 3600);
pub const REPORT_TIMEOUT: Duration = Duration::from_secs(45 * 60);

/// What the tellfold CLI is run with: the command, its environment and its working directory (the repository, so
/// every path it is given is absolute).
pub struct Cli {
    pub cmd: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
}

impl Cli {
    fn run(&self, args: &[String], log: &Path, timeout: Duration) -> Result<CliEnd, String> {
        let argv: Vec<String> = self.cmd.iter().chain(args).cloned().collect();
        run_cli(&argv, log, &self.env, &self.cwd, timeout)
    }
}

fn subfolders(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    found.sort();
    found
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn read_tail(path: &Path, n: usize) -> String {
    let bytes = std::fs::read(path).unwrap_or_default();
    tail(&String::from_utf8_lossy(&bytes), n).to_string()
}

fn run_status(run_dir: &Path) -> Value {
    read_json(&run_dir.join("status.json")).unwrap_or(json!({}))
}

/// Runs one simulation into `out` and returns the run directory. With `resume`, a finished run already in `out` is
/// kept, and one whose simulation finished but whose report failed only gets its report rebuilt. A run that fails or
/// hangs because the network dropped waits for the network and is picked up the same way.
pub fn run_tellfold(
    cli: &Cli,
    case: &Case,
    out: &Path,
    settings: &Settings,
    seed: i64,
    mut resume: bool,
) -> Result<PathBuf, String> {
    for attempt in 0..=NETWORK_TRIES {
        let runs = subfolders(out);
        if resume && runs.len() == 1 {
            let run_dir = &runs[0];
            let status = run_status(run_dir);
            if status.get("state").and_then(Value::as_str) == Some("done") && run_dir.join("report.json").exists() {
                return Ok(run_dir.clone());
            }
            if status.get("stage").and_then(Value::as_str) == Some("report") {
                println!("[{}] seed {seed}: rebuilding the report...", case.slug);
                let log = out.join("report.log");
                let end = cli.run(&["report".into(), run_dir.display().to_string()], &log, REPORT_TIMEOUT)?;
                if matches!(end, CliEnd::Exited(0)) && run_dir.join("report.json").exists() {
                    return Ok(run_dir.clone());
                }
                let text = read_tail(&log, 2000);
                if let CliEnd::Exited(code) = end {
                    if !network_error(&text) {
                        return Err(format!("tellfold report exited {code}: {}", tail(&text, 600)));
                    }
                }
                if attempt < NETWORK_TRIES {
                    wait_for_network(Duration::from_secs(60));
                }
                continue;
            }
        }
        let (end, text) = match simulate(cli, case, out, settings, seed)? {
            Ok(run_dir) => return Ok(run_dir),
            Err(failed) => failed,
        };
        if let CliEnd::Exited(code) = end {
            if !network_error(&text) {
                return Err(format!("tellfold exited {code} with a broken run folder: {}", tail(&text, 600)));
            }
        }
        let what = if matches!(end, CliEnd::TimedOut) { "hung" } else { "network failure" };
        eprintln!("[{}] seed {seed}: {what} (try {} of {})", case.slug, attempt + 1, NETWORK_TRIES + 1);
        if attempt < NETWORK_TRIES {
            wait_for_network(Duration::from_secs(60));
        }
        resume = true;
    }
    Err(format!("tellfold kept failing on the network after {} tries", NETWORK_TRIES + 1))
}

/// One fresh simulation into `out`: the run directory, or how the CLI ended and the tail of its log.
fn simulate(
    cli: &Cli,
    case: &Case,
    out: &Path,
    settings: &Settings,
    seed: i64,
) -> Result<Result<PathBuf, (CliEnd, String)>, String> {
    if out.exists() {
        std::fs::remove_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let args: Vec<String> = [
        "run".to_string(),
        case.input.display().to_string(),
        "-q".into(),
        py_str(case.r("question")),
        "--agents".into(),
        settings.agents.to_string(),
        "--rounds".into(),
        settings.rounds.to_string(),
        "--batch".into(),
        settings.batch.to_string(),
        "--agent-model".into(),
        settings.agent_model.clone(),
        "--planner-model".into(),
        settings.planner_model.clone(),
        "--concurrency".into(),
        settings.concurrency.to_string(),
        "--language".into(),
        settings.language.clone(),
        "--seed".into(),
        seed.to_string(),
        "--agent-runtime".into(),
        settings.runtime.clone(),
        "--planner-runtime".into(),
        settings.runtime.clone(),
        "--out".into(),
        out.display().to_string(),
    ]
    .into();
    let log = out.join("tellfold.log");
    let end = cli.run(&args, &log, RUN_TIMEOUT)?;
    let runs = subfolders(out);
    if matches!(end, CliEnd::Exited(0)) && runs.len() == 1 {
        return Ok(Ok(runs[0].clone()));
    }
    Ok(Err((end, read_tail(&log, 2000))))
}

/// (last round, its mean sentiment). The sentiment is None when that round has no score: an earlier round's score is
/// never used in its place.
pub fn last_sentiment(run_dir: &Path) -> Result<(Option<i64>, Option<f64>), String> {
    let db = run_dir.join("world.db");
    if !db.exists() {
        return Ok((None, None));
    }
    let con = rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("{}: {e}", db.display()))?;
    let row: Option<(i64, String)> = con
        .query_row("SELECT round, data FROM round_stats ORDER BY round DESC LIMIT 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .map(Some)
        .or_else(|e| if matches!(e, rusqlite::Error::QueryReturnedNoRows) { Ok(None) } else { Err(e) })
        .map_err(|e| format!("{}: {e}", db.display()))?;
    let Some((round, data)) = row else { return Ok((None, None)) };
    let data: Value = serde_json::from_str(&data).map_err(|e| e.to_string())?;
    Ok((Some(round), data.get("sentiment").and_then(Value::as_f64)))
}

/// Model calls the run recorded in `calls.jsonl`, and how many of them failed.
pub fn count_calls(run_dir: &Path) -> Result<(i64, i64), String> {
    let (mut calls, mut failed) = (0, 0);
    if let Ok(text) = std::fs::read_to_string(run_dir.join("calls.jsonl")) {
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            calls += 1;
            let entry: Value = serde_json::from_str(line).map_err(|e| format!("calls.jsonl: {e}"))?;
            failed += i64::from(!entry.get("ok").map_or(true, truthy));
        }
    }
    Ok((calls, failed))
}

fn write_pretty(path: &Path, value: &Value) -> Result<(), String> {
    std::fs::write(path, py_dumps_pretty(value) + "\n").map_err(|e| format!("{}: {e}", path.display()))
}

/// One simulation of one case, graded, filled into `run` step by step. On a failure it returns the error and `run`
/// keeps what was already done, so a judge failure does not lose the simulation, its call counts or its sentiment.
pub fn run_once(
    run: &mut Value,
    cli: &Cli,
    case: &Case,
    out: &Path,
    settings: &Settings,
    judge: &mut dyn Judge,
    resume: bool,
) -> Result<(), String> {
    let seed = run["seed"].as_i64().unwrap_or(FIRST_SEED);
    let run_dir = run_tellfold(cli, case, out, settings, seed, resume)?;
    let shown = run_dir.strip_prefix(&cli.cwd).unwrap_or(&run_dir);
    run["run_dir"] = json!(shown.display().to_string());
    let (calls, failed) = count_calls(&run_dir)?;
    run["calls"] = json!(calls);
    run["failed_calls"] = json!(failed);
    let (final_round, final_sentiment) = last_sentiment(&run_dir)?;
    run["final_round"] = json!(final_round);
    run["final_sentiment"] = json!(final_sentiment);
    run["sentiment_direction"] = json!(sentiment_direction(final_sentiment));
    if let (Some(round), None) = (final_round, final_sentiment) {
        run["sentiment_note"] = json!(format!("round {round}, the last, has no sentiment score"));
    }
    let report = read_json(&run_dir.join("report.json"))?;
    run["report_confidence"] = report.get("confidence").cloned().unwrap_or(Value::Null);
    run["first_reaction"] = report.get("first_reaction").cloned().unwrap_or(Value::Null);
    let config = read_json(&run_dir.join("config.json"))?;
    run["crowd_rules"] = config.get("crowd_rules").cloned().unwrap_or(json!(0));
    let stakes = config.get("stakes").cloned().unwrap_or(Value::Null);
    if truthy(&stakes) {
        let planned = stakes.get("planned").cloned().ok_or("config.stakes has no planned mix")?;
        run["planner_mix"] = planned.clone();
        run["planner_reason"] = stakes.get("reason").cloned().unwrap_or(json!(""));
        run["planner_direction"] = json!(mix_direction(&planned));
    }
    let saved = out.join("grade.json");
    if resume && saved.exists() {
        run["grade"] = read_json(&saved)?;
    } else {
        run["grade"] = judge.grade(case, &report, final_sentiment)?;
        write_pretty(&saved, &run["grade"])?;
    }
    run["scores"] = score(&case.rubric, &run["grade"])?;
    Ok(())
}

pub fn warn_unmatched(slug: &str, scores: &Value, what: &str) {
    let unmatched = scores.get("unmatched").cloned().unwrap_or(json!([]));
    if truthy(&unmatched) {
        eprintln!(
            "[{slug}] warning: the {what} judge returned unknown ids or groups {}",
            tellfold_models::json::py_repr(&unmatched)
        );
    }
}

/// Every seed of one case. A failed run is recorded with its error and the pass goes on.
pub fn run_case(
    case: &Case,
    cli: &Cli,
    runs_root: &Path,
    settings: &Settings,
    repeats: i64,
    judge: &mut dyn Judge,
    resume: bool,
) -> Result<Value, String> {
    let rubric = &case.rubric;
    let mut runs = Vec::new();
    for seed in FIRST_SEED..FIRST_SEED + repeats {
        println!("[{}] seed {seed}: simulating...", case.slug);
        let mut run = json!({"seed": seed});
        let out = runs_root.join(&case.slug).join(format!("seed-{seed}"));
        // one failed case must not stop a three-hour pass; the error goes into the results and the exit code
        match run_once(&mut run, cli, case, &out, settings, judge, resume) {
            Err(e) if crate::process::interrupted() => return Err(e),
            Err(e) => {
                eprintln!("[{}] failed: {e}", case.slug);
                run["error"] = json!(e);
            }
            Ok(()) => {
                let sc = &run["scores"];
                println!(
                    "[{}] direction {} (actual {}), objections {}, backdown {} (actual {})",
                    case.slug,
                    py_str(run["grade"].get("predicted_direction").unwrap_or(&Value::Null)),
                    py_str(&rubric["direction"]),
                    pct(&sc["objection_recall"]),
                    py_str(&sc["backdown_predicted"]),
                    py_str(&rubric["backdown"]["kind"]),
                );
                warn_unmatched(&case.slug, sc, "simulation");
            }
        }
        runs.push(run);
    }
    Ok(json!({"slug": case.slug, "rubric": rubric, "runs": runs}))
}

/// The no-crowd baseline for one case: one direct call with the same seed and question, graded the same way. A new
/// answer is written to `saved`; with `reuse` (a resumed pass), a graded answer already there is kept.
pub fn run_zero_shot(
    case: &Case,
    prompt: &(String, String),
    model: &mut dyn Model,
    judge: &mut dyn Judge,
    saved: Option<&Path>,
    reuse: bool,
) -> Result<Value, String> {
    if let Some(saved) = saved.filter(|s| reuse && s.exists()) {
        return read_json(saved);
    }
    let (system, template) = prompt;
    let seed = std::fs::read_to_string(&case.input).map_err(|e| format!("{}: {e}", case.input.display()))?;
    let user = template.replace("{{SEED}}", &seed).replace("{{QUESTION}}", &py_str(case.r("question")));
    let mut zs = json!({});
    let mut attempt = || -> Result<(), String> {
        zs["answer"] = model.call(system, &user, &zero_shot_schema())?;
        zs["grade"] = judge.grade(case, &zs["answer"], None)?;
        zs["scores"] = score(&case.rubric, &zs["grade"])?;
        warn_unmatched(&case.slug, &zs["scores"], "zero-shot");
        if let Some(saved) = saved {
            if let Some(parent) = saved.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            write_pretty(saved, &zs)?;
        }
        Ok(())
    };
    // same as run_case: a failed baseline call is recorded and counted, not fatal
    if let Err(e) = attempt() {
        if crate::process::interrupted() {
            return Err(e);
        }
        eprintln!("[{}] zero-shot failed: {e}", case.slug);
        zs["error"] = json!(e);
    }
    Ok(zs)
}
