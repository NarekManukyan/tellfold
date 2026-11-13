//! Tellfold accuracy benchmark: runs every case in `bench/cases` through the `tellfold` CLI and grades the reports.
//!
//! ```text
//! cargo run -p tellfold-bench -- --dry-run    # free: the CLI's offline model and a word-overlap judge
//! cargo run -p tellfold-bench -- --estimate   # model calls and API-equivalent cost of a full pass
//! cargo run -p tellfold-bench -- --validate   # check the case files only
//! cargo run -p tellfold-bench                 # a real pass (uses your AI tool's plan)
//! ```
//!
//! A case is a folder with `input.md` (what was known on the day, the only seed), `outcome.md` (what happened, with
//! sources; never shown to the simulation or the judge) and `rubric.json` (direction, objections, backdown, groups).
//! Every report is graded by one judge call (`bench/judge_prompt.md`) that never sees the true direction or backdown.
//! Results go to `bench/results/<date>-<label>.json` (dry runs to `bench/runs/results/`) with a Markdown summary next
//! to it.

pub mod cases;
pub mod estimate;
pub mod judge;
pub mod output;
pub mod process;
pub mod run;
pub mod score;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use cases::{load_cases, validate_all};
use judge::{ClaudeModel, DryJudge, DryModel, Judge, LlmJudge, Model};
use output::{markdown_path, merge_results, result_text, results_path, write_markdown};
use run::{run_case, run_zero_shot, Cli};
use score::{baselines, count_failures, crowd_rules_warned, planner_vs_crowd, summarise};

pub const DIRECTIONS: &[&str] = &["negative", "mixed", "positive"];
pub const BACKDOWNS: &[&str] = &["none", "partial", "full"];
pub const CATEGORIES: &[&str] =
    &["pricing", "product-change", "rebrand", "policy", "local-government", "school", "public-service", "workplace"];
pub const FIRST_SEED: i64 = 7;
/// A run's last-round sentiment (-1..1) read as a direction, for the secondary, judge-free direction score.
pub const SENTIMENT_BAND: f64 = 0.2;

/// The repository this harness was built from: `bench/` and the default tellfold build are found there.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."))
}

pub fn bench_dir() -> PathBuf {
    repo().join("bench")
}

pub fn cases_dir() -> PathBuf {
    bench_dir().join("cases")
}

pub fn results_dir() -> PathBuf {
    bench_dir().join("results")
}

/// Simulation folders, ignored by git (`runs/` in .gitignore).
pub fn runs_dir() -> PathBuf {
    bench_dir().join("runs")
}

/// Dry-run numbers mean nothing, so they stay out of `bench/results`.
pub fn dry_results_dir() -> PathBuf {
    runs_dir().join("results")
}

/// Fixed settings for every case: the app's defaults for a new simulation.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub agents: i64,
    pub rounds: i64,
    pub batch: i64,
    pub agent_model: String,
    pub planner_model: String,
    pub concurrency: i64,
    pub language: String,
    pub runtime: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            agents: 20,
            rounds: 6,
            batch: 6,
            agent_model: "haiku".into(),
            planner_model: "sonnet".into(),
            concurrency: 4,
            language: "English".into(),
            runtime: "claude-code".into(),
        }
    }
}

impl Settings {
    pub fn to_json(&self) -> Value {
        json!({
            "agents": self.agents, "rounds": self.rounds, "batch": self.batch, "agent_model": self.agent_model,
            "planner_model": self.planner_model, "concurrency": self.concurrency, "language": self.language,
            "runtime": self.runtime,
        })
    }
}

/// Python's truth value of a JSON value: null, false, 0, "" and empty lists and objects are false.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

pub fn sentiment_direction(value: Option<f64>) -> Option<&'static str> {
    let value = value?;
    Some(if value < -SENTIMENT_BAND {
        "negative"
    } else if value > SENTIMENT_BAND {
        "positive"
    } else {
        "mixed"
    })
}

pub const USAGE: &str = "\
usage: tellfold-bench [-h] [--dry-run] [--estimate] [--validate] [--cases CASES] [--merge MERGE]
                      [--repeats REPEATS] [--zero-shot] [--tellfold TELLFOLD] [--claude CLAUDE]
                      [--judge-model JUDGE_MODEL] [--agent-model AGENT_MODEL] [--planner-model PLANNER_MODEL]
                      [--label LABEL] [--cases-dir CASES_DIR] [--results-dir RESULTS_DIR] [--runs-dir RUNS_DIR]
                      [--resume RESUME]

Tellfold accuracy benchmark: run every case in bench/cases through the `tellfold` CLI and grade the reports.

    tellfold-bench --dry-run              # free: the CLI's offline model and a word-overlap judge
    tellfold-bench --estimate             # model calls and API-equivalent cost of a full pass
    tellfold-bench --validate             # check the case files only
    tellfold-bench                        # a real pass (uses your AI tool's plan)

options:
  -h, --help            show this help message and exit
  --dry-run             offline model and word-overlap judge; costs nothing
  --estimate            print the calls and cost of this pass, run nothing
  --validate            check the case files, run nothing
  --cases CASES         comma-separated slugs (default: every case)
  --merge MERGE         comma-separated result JSONs of one pass run in parts (--cases): write them as one result,
                        run nothing
  --repeats REPEATS     runs per case, seeds 7, 8, 9...
  --zero-shot           also grade one direct planner-model answer per case
  --tellfold TELLFOLD   command that runs the CLI, e.g. \"engine-rs/target/debug/tellfold\" (default: the
                        engine-rs release build, else tellfold on PATH)
  --claude CLAUDE       Claude Code CLI for the judge and zero-shot calls (default: claude)
  --judge-model JUDGE_MODEL
                        (default: sonnet)
  --agent-model AGENT_MODEL
                        (default: haiku)
  --planner-model PLANNER_MODEL
                        (default: sonnet)
  --label LABEL         results file label (default: <planner>-<agent>, or dry-run)
  --cases-dir CASES_DIR
                        (default: bench/cases)
  --results-dir RESULTS_DIR
                        default: bench/results, or bench/runs/results for a dry run
  --runs-dir RUNS_DIR   (default: bench/runs)
  --resume RESUME       the simulation folder of an interrupted pass (<runs-dir>/<date>-<label>): keep its
                        finished runs, rebuild failed reports, run only what is missing
";

/// The parsed command line. Every path is absolute: the CLI runs with the repository as its working directory.
#[derive(Clone, Debug, PartialEq)]
pub struct Args {
    pub dry_run: bool,
    pub estimate: bool,
    pub validate: bool,
    pub cases: Option<String>,
    pub merge: Option<String>,
    pub repeats: i64,
    pub zero_shot: bool,
    pub tellfold: Option<String>,
    pub claude: String,
    pub judge_model: String,
    pub agent_model: String,
    pub planner_model: String,
    pub label: Option<String>,
    pub cases_dir: PathBuf,
    pub results_dir: PathBuf,
    pub runs_dir: PathBuf,
    pub resume: Option<PathBuf>,
}

/// `Path.resolve()`: absolute, with symlinks resolved as far as the path exists.
pub fn resolve(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    if let Ok(real) = absolute.canonicalize() {
        return real;
    }
    // resolve the longest existing prefix and keep the rest as written
    let mut existing = absolute.clone();
    let mut rest = Vec::new();
    while !existing.exists() {
        match (existing.file_name().map(|n| n.to_os_string()), existing.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent.to_path_buf();
            }
            _ => return absolute,
        }
    }
    let mut out = existing.canonicalize().unwrap_or(existing);
    for name in rest.into_iter().rev() {
        out.push(name);
    }
    out
}

/// Parses the command line (without the program name). `Err(None)` asks for the help text, `Err(Some(message))` is
/// a usage error.
pub fn parse_args(argv: &[String]) -> Result<Args, Option<String>> {
    let defaults = Settings::default();
    let mut args = Args {
        dry_run: false,
        estimate: false,
        validate: false,
        cases: None,
        merge: None,
        repeats: 1,
        zero_shot: false,
        tellfold: None,
        claude: "claude".into(),
        judge_model: "sonnet".into(),
        agent_model: defaults.agent_model,
        planner_model: defaults.planner_model,
        label: None,
        cases_dir: cases_dir(),
        results_dir: PathBuf::new(),
        runs_dir: runs_dir(),
        resume: None,
    };
    let mut results_dir = None;
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        let mut value = || -> Result<String, Option<String>> {
            inline
                .clone()
                .or_else(|| it.next().cloned())
                .ok_or_else(|| Some(format!("argument {name}: expected one argument")))
        };
        match name {
            "-h" | "--help" => return Err(None),
            "--dry-run" => args.dry_run = true,
            "--estimate" => args.estimate = true,
            "--validate" => args.validate = true,
            "--zero-shot" => args.zero_shot = true,
            "--cases" => args.cases = Some(value()?),
            "--merge" => args.merge = Some(value()?),
            "--repeats" => {
                let v = value()?;
                args.repeats = v.parse().map_err(|_| Some(format!("argument --repeats: invalid int value: '{v}'")))?;
            }
            "--tellfold" => args.tellfold = Some(value()?),
            "--claude" => args.claude = value()?,
            "--judge-model" => args.judge_model = value()?,
            "--agent-model" => args.agent_model = value()?,
            "--planner-model" => args.planner_model = value()?,
            "--label" => args.label = Some(value()?),
            "--cases-dir" => args.cases_dir = PathBuf::from(value()?),
            "--results-dir" => results_dir = Some(PathBuf::from(value()?)),
            "--runs-dir" => args.runs_dir = PathBuf::from(value()?),
            "--resume" => args.resume = Some(PathBuf::from(value()?)),
            other => return Err(Some(format!("unrecognized arguments: {other}"))),
        }
    }
    // the CLI runs with the repository as its working directory, so every path it is given must be absolute
    args.cases_dir = resolve(&args.cases_dir);
    args.runs_dir = resolve(&args.runs_dir);
    let results = results_dir.unwrap_or_else(|| if args.dry_run { dry_results_dir() } else { self::results_dir() });
    args.results_dir = resolve(&results);
    args.resume = args.resume.as_deref().map(resolve);
    Ok(args)
}

/// POSIX shell words, as `shlex.split` reads them: quotes and backslashes, no expansion.
pub fn shell_split(text: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => return Err("No closing quotation".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\' | '$' | '`')) => word.push(c),
                            Some('\n') => {}
                            Some(c) => {
                                word.push('\\');
                                word.push(c);
                            }
                            None => return Err("No closing quotation".into()),
                        },
                        Some(c) => word.push(c),
                        None => return Err("No closing quotation".into()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next() {
                    Some('\n') => {}
                    Some(c) => word.push(c),
                    None => return Err("No escaped character".into()),
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

/// The command with a relative program path made absolute from where the harness was started, since the CLI runs
/// with the repository as its working directory. A bare name (`tellfold`) is still looked up on PATH.
pub fn absolute_command(mut cmd: Vec<String>) -> Vec<String> {
    if let Some(program) = cmd.first_mut() {
        if program.contains('/') && Path::new(program.as_str()).is_relative() {
            *program = resolve(Path::new(program.as_str())).display().to_string();
        }
    }
    cmd
}

/// `engine-rs/target/release/tellfold` when it is built, else `tellfold` on PATH.
pub fn default_tellfold() -> Result<Vec<String>, String> {
    let built = repo().join("engine-rs/target/release/tellfold");
    if built.exists() {
        return Ok(vec![built.display().to_string()]);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let found = dir.join("tellfold");
        if found.is_file() {
            return Ok(vec![found.display().to_string()]);
        }
    }
    Err("no tellfold binary: build engine-rs (cargo build --release -p tellfold-cli) or pass --tellfold".into())
}

pub fn git_commit() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(repo())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

/// Runs the harness with these arguments (without the program name) and returns the exit code.
pub fn main(argv: &[String]) -> i32 {
    process::install_signal_handlers();
    match try_main(argv) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("{message}");
            if process::interrupted() {
                130
            } else {
                1
            }
        }
    }
}

fn try_main(argv: &[String]) -> Result<i32, String> {
    let args = match parse_args(argv) {
        Ok(args) => args,
        Err(None) => {
            print!("{USAGE}");
            return Ok(0);
        }
        Err(Some(message)) => {
            eprint!("{}", USAGE.split("\n\n").next().unwrap_or("")); // the usage lines
            eprintln!("\ntellfold-bench: error: {message}");
            return Ok(2);
        }
    };
    let only: Option<Vec<String>> = args
        .cases
        .as_deref()
        .map(|c| c.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect());
    let cases = load_cases(only.as_deref(), &args.cases_dir)?;
    let settings = Settings {
        agent_model: args.agent_model.clone(),
        planner_model: args.planner_model.clone(),
        ..Default::default()
    };
    if args.validate {
        return Ok(if validate_all(&cases) { 0 } else { 1 });
    }
    if let Some(merge) = &args.merge {
        let paths: Vec<PathBuf> =
            merge.split(',').map(str::trim).filter(|s| !s.is_empty()).map(PathBuf::from).collect();
        return merge_results(&paths, &args.results_dir, &args.cases_dir);
    }
    if args.estimate {
        let est = estimate::estimate(&cases, &settings, args.repeats, args.zero_shot, &args.judge_model)?;
        print!("{}", estimate::format_estimate(&est));
        return Ok(0);
    }
    if !validate_all(&cases) {
        return Ok(1);
    }
    let cmd = match &args.tellfold {
        Some(text) => absolute_command(shell_split(text)?),
        None => default_tellfold()?,
    };
    if cmd.is_empty() {
        return Err("--tellfold is empty".into());
    }
    let mut env: Vec<(OsString, OsString)> =
        std::env::vars_os().filter(|(k, _)| k != tellfold_llm_fake_env()).collect();
    let (mut judge, mut zero_model, label): (Box<dyn Judge>, Option<Box<dyn Model>>, String) = if args.dry_run {
        env.push((tellfold_llm_fake_env().into(), "1".into()));
        let zero: Option<Box<dyn Model>> = args.zero_shot.then(|| Box::new(DryModel::default()) as Box<dyn Model>);
        (Box::new(DryJudge::default()), zero, args.label.clone().unwrap_or_else(|| "dry-run".into()))
    } else {
        let judge =
            LlmJudge::new(ClaudeModel::new(&args.claude, &args.judge_model), &bench_dir().join("judge_prompt.md"))?;
        let zero: Option<Box<dyn Model>> =
            args.zero_shot.then(|| Box::new(ClaudeModel::new(&args.claude, &settings.planner_model)) as Box<dyn Model>);
        let label =
            args.label.clone().unwrap_or_else(|| format!("{}-{}", settings.planner_model, settings.agent_model));
        (Box::new(judge), zero, label)
    };
    let zero_prompt =
        if zero_model.is_some() { Some(judge::split_prompt(&bench_dir().join("zero_shot_prompt.md"))?) } else { None };
    let date = chrono::Local::now().date_naive().format("%Y-%m-%d").to_string();
    std::fs::create_dir_all(&args.results_dir).map_err(|e| format!("{}: {e}", args.results_dir.display()))?;
    let out_json = results_path(&args.results_dir, &label, &date);
    let stem = out_json.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let runs_root = args.resume.clone().unwrap_or_else(|| args.runs_dir.join(stem));
    let cli = Cli { cmd: cmd.clone(), env, cwd: repo() };
    let resume = args.resume.is_some();

    let mut results = Vec::new();
    for case in &cases {
        let mut entry = run_case(case, &cli, &runs_root, &settings, args.repeats, judge.as_mut(), resume)?;
        if let (Some(model), Some(prompt)) = (zero_model.as_mut(), zero_prompt.as_ref()) {
            let saved = runs_root.join(&case.slug).join("zero-shot.json");
            entry["zero_shot"] = run_zero_shot(case, prompt, model.as_mut(), judge.as_mut(), Some(&saved), resume)?;
        }
        results.push(entry);
    }

    let result = json!({
        "benchmark": "tellfold-accuracy",
        "version": 2,
        "label": label,
        "date": date,
        "dry_run": args.dry_run,
        "git_commit": git_commit(),
        "tellfold": cmd,
        "settings": settings.to_json(),
        "repeats": args.repeats,
        "crowd_rules": crowd_rules_warned(&results),
        "judge": {"model": if args.dry_run { "word-overlap (dry run)" } else { args.judge_model.as_str() },
                  "prompt": "bench/judge_prompt.md"},
        "summary": summarise(&results, "runs"),
        "zero_shot_summary": if zero_model.is_some() { summarise(&results, "zero_shot") } else { Value::Null },
        "baselines": baselines(&results),
        "planner_vs_crowd": planner_vs_crowd(&results),
        "usage": {
            "simulation_calls": results.iter()
                .flat_map(|c| c["runs"].as_array().cloned().unwrap_or_default())
                .map(|r| r.get("calls").and_then(Value::as_i64).unwrap_or(0))
                .sum::<i64>(),
            "judge": judge.usage().to_json(),
            "zero_shot": zero_model.as_ref().map(|m| m.usage().to_json()),
        },
        "cases": results,
    });
    std::fs::write(&out_json, result_text(&result)).map_err(|e| format!("{}: {e}", out_json.display()))?;
    let md = markdown_path(&out_json);
    write_markdown(&md, &result)?;
    let (failed_runs, failed_zero) = count_failures(result["cases"].as_array().map_or(&[][..], Vec::as_slice));
    println!(
        "wrote {} and {}; {failed_runs} failed run(s), {failed_zero} failed zero-shot answer(s)",
        out_json.display(),
        md.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    );
    Ok(if failed_runs > 0 || failed_zero > 0 { 1 } else { 0 })
}

/// The variable that runs the tellfold CLI on its offline model.
fn tellfold_llm_fake_env() -> &'static str {
    "TELLFOLD_FAKE_LLM"
}
