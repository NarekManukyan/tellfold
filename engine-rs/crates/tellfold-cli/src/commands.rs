//! The commands of `tellfold` (port of the command functions in `tellfold/cli.py`). Each returns `Err(Exit)` to end
//! the process with a code once it has printed why.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tellfold_engine::runner::{mark_stopped, run_pipeline, PipelineError};
use tellfold_llm::Llm;
use tellfold_models::text::{py_prefix, py_strip};
use tellfold_models::{GodEvent, KnowledgeGraph, Persona, Report, SimConfig, Study, StudyKind, StudyRun};
use tellfold_report::chat::{ask_report, interview};
use tellfold_report::report::build_report;
use tellfold_report::rewrite::build_rewrite;
use tellfold_report::{write_report, ReportError, ReportWriter};
use tellfold_runtimes::{load_settings, Auth};
use tellfold_store::files::{copy_seeds, create_run_dir, load_cast, now_iso, write_json, REWRITE_FILE};
use tellfold_store::study::{
    join_study, letter, new_study_id, plan_variant, repeat_config, save_study, study_dir, study_of, AGREEMENT_FILE,
    COMPARE_FILE,
};
use tellfold_store::world::World;
use tellfold_study::agree::build_agreement;
use tellfold_study::branch::{prepare_branch, BranchOptions};
use tellfold_study::compare::{build_compare, Member};

use crate::args::{py_repr, Parsed, UsageError};
use crate::llm::{fake_mode, make_llm, resolve_runtimes};
use crate::paths::{as_uri, py_os_error, py_path, py_resolve};
use crate::term::{Overflow, Table, Text};
use crate::ui::Console;
use crate::view::{on_event, RunView};
use crate::VERSION;

pub const EXIT_INTERRUPTED: i32 = 130;
pub const SEED_SUFFIXES: [&str; 4] = [".pdf", ".md", ".markdown", ".txt"];

/// End the process with this code (what `typer.Exit` and a usage error do).
#[derive(Debug)]
pub enum Exit {
    Code(i32),
    /// A bad parameter value found while the command runs (`typer.BadParameter`): click's usage error, exit 2.
    Usage(UsageError),
    /// Ctrl-C where Python lets the KeyboardInterrupt through: typer prints `Aborted.` on stderr, exit 1.
    Abort,
}

pub type Done = Result<(), Exit>;

/// Prints `error: <message>` and gives exit code 1.
pub fn fail(con: &Console, message: &str) -> Exit {
    con.print(&format!("[bold red]error:[/] {message}"));
    Exit::Code(1)
}

fn fake_note() -> &'static str {
    if fake_mode() {
        "  [yellow](fake LLM)[/]"
    } else {
        ""
    }
}

enum Outcome<T> {
    Done(T),
    Interrupted,
}

/// Runs `future` to its end, or until Ctrl-C (Python's KeyboardInterrupt), when it is dropped.
fn block_on<F: Future>(future: F) -> Outcome<F::Output> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
    runtime.block_on(async {
        tokio::select! {
            value = future => Outcome::Done(value),
            _ = tokio::signal::ctrl_c() => Outcome::Interrupted,
        }
    })
}

fn open_in_browser(path: &Path) {
    let uri = as_uri(&py_resolve(path));
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = std::process::Command::new(opener)
        .arg(uri)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

// ---------- small helpers ----------

/// `ROUND:TEXT` into an event; rounds are numbered 1..rounds.
pub fn parse_event(spec: &str, rounds: i64) -> Result<GodEvent, UsageError> {
    let bad = |message: String| UsageError::new(format!("Invalid value for --event: {message}"));
    let Some((head, text)) = spec.split_once(':') else {
        return Err(bad(format!("expected ROUND:TEXT, got {}", py_repr(spec))));
    };
    let number = match tellfold_engine::pytext::decimal_int(py_strip(head)) {
        Some(n) if !py_strip(text).is_empty() => n,
        _ => return Err(bad(format!("expected ROUND:TEXT, got {}", py_repr(spec)))),
    };
    if !(1..=rounds).contains(&number) {
        return Err(bad(format!("round {number} is outside 1..{rounds}")));
    }
    Ok(GodEvent::new(number, py_strip(text)))
}

/// Files are kept as given; folders contribute their supported files, sorted.
pub fn expand_seeds(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut found: Vec<PathBuf> = std::fs::read_dir(path)
                .map(|entries| {
                    entries
                        .filter_map(|e| e.ok())
                        .map(|e| path.join(e.file_name()))
                        .filter(|p| {
                            let suffix = p.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase()));
                            suffix.is_some_and(|s| SEED_SUFFIXES.contains(&s.as_str()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            found.sort();
            files.extend(found);
        } else {
            files.push(path.clone());
        }
    }
    files
}

fn read_config(path: &Path) -> Result<SimConfig, String> {
    let text = std::fs::read_to_string(path).map_err(|e| py_os_error(&e, path))?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

pub fn load_run(con: &Console, run_dir: &Path) -> Result<(SimConfig, World), Exit> {
    let (config_path, db_path) = (run_dir.join("config.json"), run_dir.join("world.db"));
    for needed in [&config_path, &db_path] {
        if !needed.exists() {
            return Err(fail(
                con,
                &format!("{} not found. Is {} a Tellfold run directory?", needed.display(), run_dir.display()),
            ));
        }
    }
    let config = read_config(&config_path).map_err(|e| fail(con, &e))?;
    let world = World::open(&db_path, config.seed).map_err(|e| fail(con, &e.to_string()))?;
    Ok((config, world))
}

fn load_graph(run_dir: &Path) -> Result<KnowledgeGraph, String> {
    let path = run_dir.join("graph.json");
    if !path.exists() {
        return Ok(KnowledgeGraph::default());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| py_os_error(&e, &path))?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn read_report(con: &Console, run_dir: &Path) -> Result<Report, Exit> {
    let path = run_dir.join("report.json");
    if !path.exists() {
        return Err(fail(
            con,
            &format!("{} not found. Build it first with: tellfold report {}", path.display(), run_dir.display()),
        ));
    }
    let text = std::fs::read_to_string(&path).map_err(|e| fail(con, &py_os_error(&e, &path)))?;
    serde_json::from_str(&text).map_err(|e| fail(con, &e.to_string()))
}

fn run_name(run_dir: &Path) -> String {
    run_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

pub fn print_summary(con: &Console, report: &Report, run_dir: &Path) {
    let mut body = Text::plain(&report.prediction);
    body.push("\n\n", "");
    body.push("confidence: ", "dim");
    body.push(tellfold_report::report::confidence_value(report.confidence), "bold");
    con.line();
    con.panel(body, &report.title);
    let dir = run_dir.display();
    con.print_soft(&format!(
        "Report: [bold]{}[/]  (also report.md, report.json)",
        run_dir.join("report.html").display()
    ));
    con.print_soft(&format!("Next:   tellfold chat {dir} --agent a01"));
    con.print_soft(&format!("        tellfold ask {dir} \"...\""));
}

/// The pipeline under the live view; `Ok(None)` when it stopped before any round finished.
fn run_live(
    con: &Console,
    llm: &dyn Llm,
    run_dir: &Path,
    config: &SimConfig,
    seeds: &[PathBuf],
) -> Outcome<Result<Option<Report>, PipelineError>> {
    let view = Arc::new(Mutex::new(RunView::new(config.rounds)));
    let render_view = view.clone();
    let mut live = con.live(Arc::new(move || render_view.lock().expect("not poisoned").render()));
    let listener = |name: &str, data: &serde_json::Value| on_event(&view, config, name, data);
    let outcome = block_on(run_pipeline(llm, run_dir, config, seeds, Some(&listener), None, None, &ReportWriter));
    live.stop();
    outcome
}

// ---------- commands ----------

pub fn run(con: &Console, p: &Parsed) -> Done {
    let seeds: Vec<PathBuf> = p.list("seeds").iter().map(|s| py_path(s)).collect();
    let question = p.str("question").unwrap_or_default();
    let rounds = p.int("rounds");
    let out = py_path(&p.str("out").unwrap_or_else(|| "runs".into()));
    let think = p.flag("think");
    let cast_from = p.str("cast_from").map(|s| py_path(&s));
    let variant = p.str("variant");

    let files = expand_seeds(&seeds);
    if files.is_empty() {
        return Err(fail(con, "no .pdf, .md, .markdown or .txt files found in the given seeds"));
    }
    let (agent_runtime, planner_runtime) =
        resolve_runtimes(p.str("agent_runtime").as_deref(), p.str("planner_runtime").as_deref())
            .map_err(|e| fail(con, &e))?;
    let events =
        p.list("event").iter().map(|e| parse_event(e, rounds)).collect::<Result<Vec<_>, _>>().map_err(Exit::Usage)?;
    let mut config = SimConfig {
        num_agents: p.int("agents"),
        rounds,
        batch_size: p.int("batch"),
        agent_model: p.str("agent_model").unwrap_or_default(),
        planner_model: p.str("planner_model").unwrap_or_default(),
        concurrency: p.int("concurrency"),
        language: p.str("language").unwrap_or_default(),
        events,
        seed: p.int("seed"),
        think,
        agent_runtime,
        planner_runtime,
        ..SimConfig::new(&question, files.iter().map(|f| f.to_string_lossy().into_owned()).collect())
    };
    let mut study: Option<Study> = None;
    if let Some(cast_from) = &cast_from {
        if py_resolve(cast_from).parent() != Some(py_resolve(&out).as_path()) {
            return Err(fail(con, &format!("--cast-from must be a run in the --out folder ({})", out.display())));
        }
        let source = read_config(&cast_from.join("config.json"))
            .map_err(|e| fail(con, &format!("cannot reuse {}: {e}", cast_from.display())))?;
        let cast = load_cast(cast_from).map_err(|e| fail(con, &e.to_string()))?;
        let (planned, label) =
            plan_variant(&out, &run_name(cast_from), &source, None, variant.as_deref(), &now_iso(), None)
                .map_err(|e| fail(con, &e.to_string()))?;
        config.cast_from = Some(run_name(cast_from));
        config.study_id = Some(planned.id.clone());
        config.variant = Some(label);
        config.num_agents = cast.personas.len() as i64;
        study = Some(planned);
    } else if variant.as_deref().is_some_and(|v| !v.is_empty()) {
        return Err(fail(con, "--variant needs --cast-from: a study compares runs of one crowd"));
    }
    let run_dir = create_run_dir(&out, &question).map_err(|e| fail(con, &py_os_error(&e, &out)))?;
    if let Some(study) = &study {
        join_study(&out, study, &run_name(&run_dir), config.variant.as_deref().unwrap_or(""))
            .map_err(|e| fail(con, &e.to_string()))?;
    }
    let files = copy_seeds(&run_dir, &files).map_err(|e| fail(con, &e.to_string()))?;
    config.seed_files = files.iter().map(|f| f.to_string_lossy().into_owned()).collect();
    write_json(&run_dir.join("config.json"), &config).map_err(|e| fail(con, &e.to_string()))?;
    con.print_soft(&format!("[bold]Tellfold[/] run [cyan]{}[/]{}", run_dir.display(), fake_note()));

    let llm = make_llm(Some(&run_dir), config.concurrency, think, Some(&config)).map_err(|e| fail(con, &e.0))?;
    let report = match run_live(con, llm.as_ref(), &run_dir, &config, &files) {
        Outcome::Interrupted => {
            let _ = mark_stopped(&run_dir);
            let dir = run_dir.display();
            con.print_soft(&format!(
                "\n[yellow]Interrupted.[/] Everything written so far is in {dir}.\nIf the simulation had started, \
                 rebuild the report with: tellfold report {dir}"
            ));
            return Err(Exit::Code(EXIT_INTERRUPTED));
        }
        Outcome::Done(Err(e)) => {
            con.line();
            return Err(fail(
                con,
                &format!(
                    "{e}\nPartial results are in {}. Check `tellfold doctor` if the CLI itself failed.",
                    run_dir.display()
                ),
            ));
        }
        Outcome::Done(Ok(None)) => {
            con.print_soft(&format!(
                "[yellow]Stopped[/] before any round finished. Partial results are in {}.",
                run_dir.display()
            ));
            return Err(Exit::Code(EXIT_INTERRUPTED));
        }
        Outcome::Done(Ok(Some(report))) => report,
    };
    print_summary(con, &report, &run_dir);
    if let Some(study) = &study {
        con.print_soft(&format!(
            "Study:  {}. Compare the versions with: tellfold compare {}",
            study.id,
            run_dir.display()
        ));
    }
    if p.flag("open_report") {
        open_in_browser(&run_dir.join("report.html"));
    }
    Ok(())
}

fn study_members(runs_dir: &Path, study: &Study, with_report: bool) -> Vec<Member> {
    study
        .runs
        .iter()
        .enumerate()
        .filter(|(_, r)| !with_report || runs_dir.join(&r.id).join("report.json").is_file())
        .map(|(i, r)| Member::new(&letter(i as i64), &r.id, &r.variant, &runs_dir.join(&r.id)))
        .collect()
}

pub fn compare(con: &Console, p: &Parsed) -> Done {
    let run_dir = py_path(&p.str("run_dir").unwrap_or_default());
    let (config, world) = load_run(con, &run_dir)?;
    let _ = world.close();
    let resolved = py_resolve(&run_dir);
    let runs_dir = resolved.parent().map(Path::to_path_buf).unwrap_or_default();
    let Some(study) = study_of(&runs_dir, &run_name(&resolved), &config) else {
        let dir = run_dir.display();
        return Err(fail(
            con,
            &format!("{dir} is not part of a study. Make one with: tellfold run SEED -q ... --cast-from {dir}"),
        ));
    };
    let members = study_members(&runs_dir, &study, true);
    if members.len() < 2 {
        return Err(fail(con, "a comparison needs at least two runs of the study with a report"));
    }
    let folder = study_dir(&runs_dir, &study.id).map_err(|e| fail(con, &e.to_string()))?;
    let llm = make_llm(Some(&run_dir), 1, false, Some(&config)).map_err(|e| fail(con, &e.0))?;
    let model = p.str("model").filter(|m| !m.is_empty()).unwrap_or_else(|| config.planner_model.clone());
    let status = con.status("Comparing the versions...");
    let outcome =
        block_on(build_compare(llm.as_ref(), &study.id, &members, &config.question, &model, &config.language, None));
    drop(status);
    let result = match outcome {
        Outcome::Interrupted => {
            con.print("\n[yellow]Interrupted.[/] Nothing was written.");
            return Err(Exit::Code(EXIT_INTERRUPTED));
        }
        Outcome::Done(Err(e)) => return Err(fail(con, &e.to_string())),
        Outcome::Done(Ok(result)) => result,
    };
    std::fs::create_dir_all(&folder).map_err(|e| fail(con, &e.to_string()))?;
    write_json(&folder.join(COMPARE_FILE), &result).map_err(|e| fail(con, &e.to_string()))?;
    let verdict = result.report.better_variant.clone().filter(|v| !v.is_empty()).unwrap_or_else(|| "a tie".into());
    let mut body = Text::plain(&result.report.why);
    body.push("\n\n", "");
    body.push("better: ", "dim");
    body.push(&verdict, "bold");
    con.panel(body, &result.report.headline);
    con.print_soft(&format!("Comparison: [bold]{}[/]", folder.join(COMPARE_FILE).display()));
    Ok(())
}

pub fn check(con: &Console, p: &Parsed) -> Done {
    let run_dir = py_path(&p.str("run_dir").unwrap_or_default());
    let runs = p.int("runs");
    let agent_model = p.str("agent_model");
    let (config, world) = load_run(con, &run_dir)?;
    let _ = world.close();
    if !run_dir.join("report.json").is_file() {
        let dir = run_dir.display();
        return Err(fail(con, &format!("{dir} has no report yet. Build it first with: tellfold report {dir}")));
    }
    let resolved = py_resolve(&run_dir);
    let out = resolved.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut seeds: Vec<PathBuf> = std::fs::read_dir(run_dir.join("seeds"))
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| run_dir.join("seeds").join(e.file_name()))
                .filter(|p| p.is_file() && !run_name(p).starts_with('.'))
                .collect()
        })
        .unwrap_or_default();
    seeds.sort();
    if seeds.is_empty() {
        return Err(fail(con, &format!("{} has no saved seed documents to run again", run_dir.display())));
    }
    let mut study = Study {
        kind: StudyKind::Repeat,
        planned: runs,
        runs: vec![StudyRun { id: run_name(&resolved), variant: "Run 1".into() }],
        ..Study::new(&new_study_id(None), &now_iso())
    };
    save_study(&out, &study).map_err(|e| fail(con, &e.to_string()))?;
    con.print_soft(&format!(
        "[bold]Check it[/]: {} more run{} of {}, one after another{}",
        runs - 1,
        if runs > 2 { "s" } else { "" },
        run_name(&run_dir),
        fake_note()
    ));
    for i in 1..runs {
        let new_dir = create_run_dir(&out, &config.question).map_err(|e| fail(con, &py_os_error(&e, &out)))?;
        let files = copy_seeds(&new_dir, &seeds).map_err(|e| fail(con, &e.to_string()))?;
        let new = repeat_config(&config, &run_name(&resolved), i, &files, &study.id, agent_model.as_deref());
        let variant = new.variant.clone().unwrap_or_default();
        study = join_study(&out, &study, &run_name(&new_dir), &variant).map_err(|e| fail(con, &e.to_string()))?;
        write_json(&new_dir.join("config.json"), &new).map_err(|e| fail(con, &e.to_string()))?;
        let llm = make_llm(Some(&new_dir), new.concurrency, new.think, Some(&new))
            .map_err(|e| fail(con, &format!("{variant} failed: {e}")))?;
        let status = con.status(&format!("{variant} of {runs} (seed {})...", new.seed));
        let outcome = block_on(run_pipeline(llm.as_ref(), &new_dir, &new, &files, None, None, None, &ReportWriter));
        drop(status);
        let done = match outcome {
            Outcome::Interrupted => {
                let _ = mark_stopped(&new_dir);
                con.print(&format!("\n[yellow]Interrupted.[/] The check stopped at {variant}."));
                return Err(Exit::Code(EXIT_INTERRUPTED));
            }
            Outcome::Done(Err(e)) => return Err(fail(con, &format!("{variant} failed: {e}"))),
            Outcome::Done(Ok(None)) => return Err(fail(con, &format!("{variant} stopped before a round finished"))),
            Outcome::Done(Ok(Some(done))) => done,
        };
        con.print_soft(&format!("{variant}: {}", done.title));
    }
    let members = study_members(&out, &study, false);
    let llm = make_llm(Some(&run_dir), 1, false, Some(&config)).map_err(|e| fail(con, &e.0))?;
    let status = con.status("Checking what held...");
    let outcome = block_on(build_agreement(
        llm.as_ref(),
        &study.id,
        &members,
        &config.question,
        &config.planner_model,
        &config.language,
    ));
    drop(status);
    let agreement = match outcome {
        Outcome::Interrupted => return Err(Exit::Abort),
        Outcome::Done(Err(e)) => return Err(fail(con, &e.to_string())),
        Outcome::Done(Ok(a)) => a,
    };
    let folder = study_dir(&out, &study.id).map_err(|e| fail(con, &e.to_string()))?;
    write_json(&folder.join(AGREEMENT_FILE), &agreement).map_err(|e| fail(con, &e.to_string()))?;
    let joined = |items: &[String]| if items.is_empty() { "-".to_string() } else { items.join("; ") };
    let mut body = Text::plain(&agreement.summary);
    body.push("\n\n", "");
    body.push("held: ", "dim");
    body.push(&joined(&agreement.held), "");
    body.push("\n", "");
    body.push("split: ", "dim");
    body.push(&joined(&agreement.split), "");
    con.panel(
        body,
        &format!(
            "{} across {} runs",
            py_capitalize(
                serde_json::to_value(agreement.verdict)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .as_deref()
                    .unwrap_or("")
            ),
            agreement.runs
        ),
    );
    con.print_soft(&format!("Check: [bold]{}[/]", folder.join(AGREEMENT_FILE).display()));
    Ok(())
}

/// `str.capitalize()`: the first character upper-cased, the rest lower-cased.
fn py_capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars.flat_map(char::to_lowercase)).collect(),
    }
}

pub fn branch(con: &Console, p: &Parsed) -> Done {
    let run_dir = py_path(&p.str("run_dir").unwrap_or_default());
    let at = p.int("at");
    let (config, world) = load_run(con, &run_dir)?;
    // includes events added while the run was going, which config.json lacks
    let happened = world.events();
    let _ = world.close();
    let happened = happened.map_err(|e| fail(con, &e.to_string()))?;
    let new_events = p
        .list("event")
        .iter()
        .map(|e| parse_event(e, config.rounds))
        .collect::<Result<Vec<_>, _>>()
        .map_err(Exit::Usage)?;
    let seen: Vec<(i64, &str)> = happened.iter().map(|e| (e.round, e.content.as_str())).collect();
    let mut planned = happened.clone();
    planned.extend(config.events.iter().filter(|e| !seen.contains(&(e.round, e.content.as_str()))).cloned());
    let mut events: Vec<GodEvent> =
        if p.flag("drop_events") { Vec::new() } else { planned.into_iter().filter(|e| e.round > at).collect() };
    events.extend(new_events);
    let resolved = py_resolve(&run_dir);
    let out = resolved.parent().map(Path::to_path_buf).unwrap_or_default();
    let variant = format!("Branch at round {at}");
    let (study, label) =
        plan_variant(&out, &run_name(&resolved), &config, None, Some(&variant), &now_iso(), Some("Original"))
            .map_err(|e| fail(con, &e.to_string()))?;
    let new_dir = create_run_dir(&out, &config.question).map_err(|e| fail(con, &py_os_error(&e, &out)))?;
    let options = BranchOptions { source_id: None, immutable: false, study_id: Some(&study.id), variant: Some(&label) };
    let new = match prepare_branch(&resolved, &new_dir, &config, at, &events, options) {
        Ok(new) => new,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&new_dir);
            return Err(fail(con, &e.to_string()));
        }
    };
    join_study(&out, &study, &run_name(&new_dir), &label).map_err(|e| fail(con, &e.to_string()))?;
    con.print_soft(&format!(
        "[bold]Tellfold[/] branch [cyan]{}[/] of {} after round {at}{}",
        new_dir.display(),
        run_name(&run_dir),
        fake_note()
    ));
    let seeds: Vec<PathBuf> = new.seed_files.iter().map(PathBuf::from).filter(|p| p.is_file()).collect();
    let llm = make_llm(Some(&new_dir), new.concurrency, new.think, Some(&new)).map_err(|e| {
        con.line();
        fail(con, &format!("{e}\nPartial results are in {}.", new_dir.display()))
    })?;
    let built = match run_live(con, llm.as_ref(), &new_dir, &new, &seeds) {
        Outcome::Interrupted => {
            let _ = mark_stopped(&new_dir);
            con.print_soft(&format!(
                "\n[yellow]Interrupted.[/] Everything written so far is in {}.",
                new_dir.display()
            ));
            return Err(Exit::Code(EXIT_INTERRUPTED));
        }
        Outcome::Done(Err(e)) => {
            con.line();
            return Err(fail(con, &format!("{e}\nPartial results are in {}.", new_dir.display())));
        }
        Outcome::Done(Ok(None)) => return Err(fail(con, "the branch stopped before a round finished")),
        Outcome::Done(Ok(Some(built))) => built,
    };
    print_summary(con, &built, &new_dir);
    con.print_soft(&format!(
        "Study:  {}. Compare it with the original: tellfold compare {}",
        study.id,
        new_dir.display()
    ));
    if p.flag("open_report") {
        open_in_browser(&new_dir.join("report.html"));
    }
    Ok(())
}

/// `shlex.quote`.
fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c)) {
        return s.into();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

pub fn rewrite(con: &Console, p: &Parsed) -> Done {
    let run_dir = py_path(&p.str("run_dir").unwrap_or_default());
    let (config, world) = load_run(con, &run_dir)?;
    let _ = world.close();
    let built = read_report(con, &run_dir)?;
    let llm = make_llm(Some(&run_dir), 1, false, Some(&config)).map_err(|e| fail(con, &e.0))?;
    let model = p.str("model").filter(|m| !m.is_empty()).unwrap_or_else(|| config.planner_model.clone());
    let status = con.status("Writing a better version...");
    let outcome = block_on(build_rewrite(llm.as_ref(), &run_dir, &config, &built, &model, false));
    drop(status);
    let suggestion = match outcome {
        Outcome::Interrupted => {
            con.print("\n[yellow]Interrupted.[/] Nothing was written.");
            return Err(Exit::Code(EXIT_INTERRUPTED));
        }
        Outcome::Done(Err(e)) => return Err(fail(con, &e.to_string())),
        Outcome::Done(Ok(s)) => s,
    };
    write_json(&run_dir.join(REWRITE_FILE), &suggestion).map_err(|e| fail(con, &e.to_string()))?;
    let suffix = Path::new(&suggestion.source_name)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .filter(|s| s.len() > 1)
        .unwrap_or_else(|| ".md".into());
    let revised = run_dir.join(format!("rewrite{suffix}"));
    let text = suggestion.revised_text.trim_end_matches(tellfold_models::text::py_isspace);
    std::fs::write(&revised, format!("{text}\n")).map_err(|e| fail(con, &e.to_string()))?;
    let mut others: Vec<PathBuf> = std::fs::read_dir(run_dir.join("seeds"))
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| run_dir.join("seeds").join(e.file_name()))
                .filter(|p| {
                    let name = run_name(p);
                    p.is_file() && !name.starts_with('.') && name != suggestion.source_name
                })
                .collect()
        })
        .unwrap_or_default();
    others.sort();
    con.panel(Text::from_markup(&suggestion.revised_text), &format!("Suggested {}", suggestion.source_name));
    for change in &suggestion.changes {
        let cites = change.cites.iter().map(|pid| format!("#{pid}")).collect::<Vec<_>>().join(", ");
        let cites = if cites.is_empty() { String::new() } else { format!(" [dim]({cites})[/]") };
        con.print_soft(&format!("[bold]{}[/] {}{cites}", change.what, change.why));
    }
    for note in &suggestion.kept_on_purpose {
        con.print_soft(&format!("[dim]kept:[/] {note}"));
    }
    let files = std::iter::once(&revised)
        .chain(others.iter())
        .map(|p| shell_quote(&p.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ");
    con.print_soft(&format!(
        "Saved: [bold]{}[/] (and {REWRITE_FILE}). Test it against the same people with:\n  tellfold run {files} -q {} \
         --cast-from {}",
        revised.display(),
        shell_quote(&config.question),
        shell_quote(&run_dir.to_string_lossy())
    ));
    Ok(())
}

pub fn report(con: &Console, p: &Parsed) -> Done {
    let run_dir = py_path(&p.str("run_dir").unwrap_or_default());
    let (config, world) = load_run(con, &run_dir)?;
    let llm = make_llm(Some(&run_dir), config.concurrency, config.think, None).map_err(|e| fail(con, &e.0))?;
    let graph = load_graph(&run_dir).map_err(|e| fail(con, &e))?;
    let model = p.str("model").filter(|m| !m.is_empty()).unwrap_or_else(|| config.planner_model.clone());
    let status = con.status("Writing the report...");
    let outcome = block_on(build_report(llm.as_ref(), &world, &graph, &config, &model));
    drop(status);
    let built = match outcome {
        Outcome::Interrupted => {
            con.print("\n[yellow]Interrupted.[/] The report was not rewritten.");
            return Err(Exit::Code(EXIT_INTERRUPTED));
        }
        Outcome::Done(Err(e)) => return Err(fail(con, &e.to_string())),
        Outcome::Done(Ok(built)) => built,
    };
    let html_path = write_report(&run_dir, &built, &world, &config).map_err(|e| fail(con, &e.to_string()))?;
    print_summary(con, &built, &run_dir);
    if p.flag("open_report") {
        open_in_browser(&html_path);
    }
    Ok(())
}

/// An agent by id (a03) or handle (@maria_dev).
fn find_persona(con: &Console, world: &World, agent: &str) -> Result<Persona, Exit> {
    let personas = world.personas().map_err(|e| fail(con, &e.to_string()))?;
    let wanted = agent.strip_prefix('@').unwrap_or(agent).to_lowercase();
    if let Some(found) = personas.iter().find(|p| wanted == p.id.to_lowercase() || wanted == p.handle.to_lowercase()) {
        return Ok(found.clone());
    }
    let known = personas.iter().map(|p| p.id.as_str()).collect::<Vec<_>>().join(", ");
    let known = if known.is_empty() { "none".to_string() } else { known };
    Err(fail(con, &format!("unknown agent {}. Known: {known}", py_repr(agent))))
}

pub fn chat(con: &Console, p: &Parsed) -> Done {
    let run_dir = py_path(&p.str("run_dir").unwrap_or_default());
    let (config, world) = load_run(con, &run_dir)?;
    let persona = find_persona(con, &world, &p.str("agent").unwrap_or_default())?;
    con.panel(
        Text::from_markup(&format!("[bold]@{}[/] {}\n{}", persona.handle, persona.name, persona.bio)),
        &format!("Interviewing {}", persona.id),
    );
    let llm = make_llm(Some(&run_dir), 1, false, None).map_err(|e| fail(con, &e.0))?;
    let model = p.str("model").filter(|m| !m.is_empty()).unwrap_or_else(|| config.agent_model.clone());
    let mut history: Vec<(String, String)> = Vec::new();
    loop {
        // read on a thread of its own, so Ctrl-C ends the chat while it waits (the thread dies with the process)
        let reader = con.clone();
        let (send, typed) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || send.send(reader.input("[bold cyan]you>[/] ")));
        let typed = block_on(async move { typed.await.ok().flatten() });
        let question = match typed {
            Outcome::Done(Some(line)) => py_strip(&line).to_string(),
            _ => break,
        };
        if ["exit", "quit", ":q"].contains(&question.to_lowercase().as_str()) {
            break;
        }
        if question.is_empty() {
            continue;
        }
        match block_on(interview(llm.as_ref(), &world, &persona.id, &question, &model, &history)) {
            Outcome::Interrupted => break,
            Outcome::Done(Err(ReportError::Llm(e))) => con.print(&format!("[red]{e}[/]")),
            Outcome::Done(Err(e)) => return Err(fail(con, &e.to_string())),
            Outcome::Done(Ok(answer)) => {
                con.print(&format!("[bold magenta]@{}>[/] {answer}\n", persona.handle));
                history.push((question, answer));
            }
        }
    }
    Ok(())
}

pub fn ask(con: &Console, p: &Parsed) -> Done {
    let run_dir = py_path(&p.str("run_dir").unwrap_or_default());
    let question = p.str("question").unwrap_or_default();
    let (config, world) = load_run(con, &run_dir)?;
    let built = read_report(con, &run_dir)?;
    let llm = make_llm(Some(&run_dir), 1, false, None).map_err(|e| fail(con, &e.0))?;
    let model = p.str("model").filter(|m| !m.is_empty()).unwrap_or_else(|| config.planner_model.clone());
    let status = con.status("Thinking...");
    let outcome = block_on(ask_report(llm.as_ref(), &world, &built, &question, &model));
    drop(status);
    match outcome {
        Outcome::Interrupted => Err(Exit::Code(EXIT_INTERRUPTED)),
        Outcome::Done(Err(e)) => Err(fail(con, &e.to_string())),
        Outcome::Done(Ok(answer)) => {
            con.print(&answer);
            Ok(())
        }
    }
}

pub fn agents(con: &Console, p: &Parsed) -> Done {
    let run_dir = py_path(&p.str("run_dir").unwrap_or_default());
    let (_, world) = load_run(con, &run_dir)?;
    let read = || -> Result<Table, tellfold_store::world::WorldError> {
        let mut post_counts: HashMap<String, usize> = HashMap::new();
        for post in world.posts(None)? {
            *post_counts.entry(post.author_id).or_default() += 1;
        }
        let mut table = Table::new().title(&format!("Agents in {}", run_name(&run_dir)));
        for header in ["ID", "Handle", "Name", "Followers", "Posts", "Activity", "Influence", "Starting stance"] {
            table.add_column_with(header, crate::term::Justify::Left, Overflow::Fold);
        }
        for persona in world.personas()? {
            table.add_row(vec![
                Text::from_markup(&persona.id),
                Text::from_markup(&format!("@{}", persona.handle)),
                Text::from_markup(&persona.name),
                Text::from_markup(&world.followers(&persona.id)?.len().to_string()),
                Text::from_markup(&post_counts.get(&persona.id).copied().unwrap_or(0).to_string()),
                Text::from_markup(&format!("{:.2}", persona.activity)),
                Text::from_markup(&format!("{:.2}", persona.influence)),
                Text::from_markup(py_prefix(&persona.stance, 80)),
            ]);
        }
        Ok(table)
    };
    let table = read().map_err(|e| fail(con, &e.to_string()))?;
    con.table(&table);
    Ok(())
}

// ---------- doctor ----------

fn cli_version(path: &str) -> String {
    let child = std::process::Command::new(path)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(e) => return format!("version check failed: {}", py_os_error(&e, Path::new(path))),
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed().as_secs() >= 20 => {
                let _ = child.kill();
                let _ = child.wait();
                return format!("version check failed: Command '['{path}', '--version']' timed out after 20 seconds");
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Err(e) => return format!("version check failed: {e}"),
        }
    }
    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(e) => return format!("version check failed: {e}"),
    };
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let text = if stdout.is_empty() { String::from_utf8_lossy(&output.stderr).into_owned() } else { stdout };
    let text = py_strip(&text);
    if text.is_empty() {
        "unknown version".into()
    } else {
        text.into()
    }
}

fn ping_model() -> (String, bool, String) {
    let label = format!("Logged in (haiku call){}", if fake_mode() { " [fake LLM]" } else { "" });
    let llm = match make_llm(None, 1, false, None) {
        Ok(llm) => llm,
        Err(e) => return (label, false, format!("{e}. Run `claude` once and log in with /login.")),
    };
    let started = Instant::now();
    let reply =
        block_on(llm.text(tellfold_runtimes::PING_PROMPT, tellfold_runtimes::PING_SYSTEM, "haiku", "doctor.ping"));
    match reply {
        Outcome::Interrupted => {
            eprint!("\nAborted.\n");
            std::process::exit(1)
        }
        Outcome::Done(Err(e)) => (label, false, format!("{e}. Run `claude` once and log in with /login.")),
        Outcome::Done(Ok(reply)) => {
            let reply = py_strip(&reply);
            (
                label,
                !reply.is_empty(),
                format!("replied {} in {:.1}s", py_repr(py_prefix(reply, 40)), started.elapsed().as_secs_f64()),
            )
        }
    }
}

/// Python checks the claude-agent-sdk import and its own version here; the Rust engine drives the `claude` CLI
/// itself, so those two rows are one engine row.
pub fn doctor(con: &Console, _p: &Parsed) -> Done {
    let mut rows: Vec<(String, bool, String)> = Vec::new();
    let path_var = std::env::var("PATH").unwrap_or_else(|_| "/bin:/usr/bin".into());
    let cli_path = tellfold_runtimes::detect::which_in("claude", &path_var);
    rows.push((
        "Claude Code CLI on PATH".into(),
        cli_path.is_some(),
        match &cli_path {
            Some(path) => format!("{path} ({})", cli_version(path)),
            None => "not found: install Claude Code and make sure `claude` is on PATH".into(),
        },
    ));
    rows.push(("Tellfold engine".into(), true, format!("{VERSION} (Rust)")));
    if fake_mode() || cli_path.is_some() {
        rows.push(ping_model());
    } else {
        rows.push(("Logged in (haiku call)".into(), false, "skipped: fix the checks above first".into()));
    }
    let mut table = Table::new().title("tellfold doctor");
    for header in ["Check", "Status", "Detail"] {
        table.add_column_with(header, crate::term::Justify::Left, Overflow::Fold);
    }
    for (name, ok, detail) in &rows {
        let status = if *ok { Text::styled("ok", "green") } else { Text::styled("FAIL", "bold red") };
        table.add_row(vec![Text::from_markup(name), status, Text::from_markup(detail)]);
    }
    con.table(&table);
    if rows.iter().all(|r| r.1) {
        Ok(())
    } else {
        Err(Exit::Code(1))
    }
}

// ---------- runtimes ----------

/// `json.dumps(value, indent=2)`: two-space indents and non-ASCII escaped.
fn json_indent2(value: &serde_json::Value) -> String {
    let pretty = serde_json::to_string_pretty(value).expect("JSON");
    let mut out = String::with_capacity(pretty.len());
    for c in pretty.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

pub fn runtimes(con: &Console, p: &Parsed) -> Done {
    let check_auth = p.flag("check_auth");
    let settings = load_settings(None);
    let infos = match block_on(tellfold_runtimes::detect_all(&settings, check_auth)) {
        Outcome::Done(infos) => infos,
        Outcome::Interrupted => return Err(Exit::Abort),
    };
    if p.flag("as_json") {
        let value = serde_json::to_value(&infos).expect("JSON");
        con.write(&format!("{}\n", json_indent2(&value)));
        return Ok(());
    }
    let mut table = Table::new().title("Agent runtimes");
    for header in ["ID", "Installed", "Version", "Auth", "Tested", "Agent / planner model"] {
        table.add_column_with(header, crate::term::Justify::Left, Overflow::Fold);
    }
    for info in &infos {
        let (auth, style) = match info.auth {
            Auth::Ok => ("ok", "green"),
            Auth::Missing => ("missing", "red"),
            Auth::Unknown => ("unknown", "dim"),
        };
        let defaults = info
            .default_models
            .values()
            .map(|v| if v.is_empty() { "-" } else { v.as_str() })
            .collect::<Vec<_>>()
            .join(" / ");
        table.add_row(vec![
            Text::from_markup(&info.id),
            if info.installed { Text::styled("yes", "green") } else { Text::styled("no", "dim") },
            Text::from_markup(info.version.as_deref().filter(|v| !v.is_empty()).unwrap_or("-")),
            Text::styled(auth, style),
            Text::from_markup(if info.tested { "yes" } else { "no" }),
            Text::from_markup(if defaults.is_empty() { "-" } else { &defaults }),
        ]);
    }
    con.table(&table);
    for info in infos.iter().filter(|i| i.installed && i.auth == Auth::Missing) {
        if let Some(hint) = info.auth_hint.as_deref().filter(|h| !h.is_empty()) {
            con.print_soft(&format!("[yellow]{}[/]: {hint}", info.id));
        }
    }
    if !check_auth {
        con.print("[dim]Add --check-auth to check logins.[/]");
    }
    Ok(())
}

/// `ui` and `serve` run the HTTP server, which comes with the next milestone of the port.
pub fn not_yet(con: &Console, name: &str) -> Done {
    Err(fail(con, &format!("`tellfold {name}` is not in the Rust engine yet; use the Python engine for it")))
}
