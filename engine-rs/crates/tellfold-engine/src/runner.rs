//! The run pipeline (port of `run_pipeline` in `tellfold/runner.py`): seeds in, report out, status on disk.
//!
//! [`run_pipeline`] reads the seed files, extracts the knowledge graph, creates personas, seeds the world, runs
//! the rounds and writes the report. While it works it keeps `status.json` (a [`RunStatus`]) up to date in the run
//! folder and reports progress through an `on_event(name, payload)` callback. Every payload is a JSON object a
//! server can forward as is.
//!
//! The report stage is a [`ReportStage`] the caller passes in, so the pipeline does not depend on the report
//! writer's crate.

use std::cell::RefCell;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use serde_json::{json, Map, Value};
use tellfold_llm::ingest::{chunk_text_default, load_documents};
use tellfold_llm::{Llm, LlmError};
use tellfold_models::text::py_prefix;
use tellfold_models::{BatchRef, FailedBatch, KnowledgeGraph, Report, RunStage, RunState, RunStatus, SimConfig};
use tellfold_store::estimate::estimate_calls;
use tellfold_store::files::{
    load_cast, now_iso, read_status, write_json, write_json_list, write_status, RunError, CONFIG_FILE, GRAPH_FILE,
    PERSONAS_FILE, WORLD_FILE,
};
use tellfold_store::world::{World, WorldError};

use crate::engine::{Engine, EngineError};
use crate::graph::{extract_graph, select_chunks, MAX_EXTRACT_CHUNKS};
use crate::personas::generate_personas;

/// characters of a failed batch's error kept in status.json
const FAILED_BATCH_ERROR_MAX: usize = 500;

/// Why a run failed. The text is what `status.json` and the `run_failed` event carry.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error(transparent)]
    Run(#[from] RunError),
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    World(#[from] WorldError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

/// Writes the report of a finished (or stopped) run: `build_report` then `write_report` in Python.
#[async_trait(?Send)]
pub trait ReportStage {
    async fn write_report(
        &self,
        llm: &dyn Llm,
        run_dir: &Path,
        world: &World,
        graph: &KnowledgeGraph,
        config: &SimConfig,
    ) -> Result<Report, PipelineError>;
}

pub type OnEvent<'a> = &'a dyn Fn(&str, &Value);

fn engine_step(step: &str) -> String {
    match step {
        "follow_graph" => "building the follow graph".into(),
        "follow_graph_done" => "follow graph ready".into(),
        "opening_posts" => "writing opening posts".into(),
        "opening_posts_done" => "opening posts written".into(),
        other => other.replace('_', " "),
    }
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// Owns the [`RunStatus`] of one run: every change is written to disk and announced.
struct Tracker<'a> {
    run_dir: PathBuf,
    on_event: Option<OnEvent<'a>>,
    status: RefCell<RunStatus>,
}

impl<'a> Tracker<'a> {
    fn new(run_dir: &Path, config: &SimConfig, on_event: Option<OnEvent<'a>>) -> Self {
        let now = now_iso();
        Tracker {
            run_dir: run_dir.to_path_buf(),
            on_event,
            status: RefCell::new(RunStatus {
                state: RunState::Running,
                stage: RunStage::Reading,
                round: 0,
                total_rounds: config.rounds,
                detail: String::new(),
                error: None,
                pid: Some(i64::from(std::process::id())),
                started_at: now.clone(),
                updated_at: now,
                failed_batches: Vec::new(),
            }),
        }
    }

    fn emit(&self, name: &str, payload: Value) {
        if let Some(f) = self.on_event {
            // a broken listener must never break the run
            if std::panic::catch_unwind(AssertUnwindSafe(|| f(name, &payload))).is_err() {
                eprintln!("on_event listener failed for {name}");
            }
        }
    }

    fn update(&self, change: impl FnOnce(&mut RunStatus)) -> std::io::Result<()> {
        let snapshot = {
            let mut status = self.status.borrow_mut();
            change(&mut status);
            status.updated_at = now_iso();
            status.clone()
        };
        write_status(&self.run_dir, &snapshot)?;
        self.emit("status", serde_json::to_value(&snapshot).expect("status serializes"));
        Ok(())
    }

    fn stage(&self, stage: RunStage, detail: &str, extra: Value) -> std::io::Result<()> {
        self.update(|s| {
            s.stage = stage;
            s.detail = detail.into();
        })?;
        let mut payload = Map::new();
        payload.insert("stage".into(), json!(stage.value()));
        payload.insert("detail".into(), json!(detail));
        payload.extend(object(extra));
        self.emit("stage", Value::Object(payload));
        Ok(())
    }

    /// Same stage, new activity text (re-announced as a `stage` event so the UI can show it).
    fn detail(&self, detail: &str, extra: Value) -> std::io::Result<()> {
        let stage = self.status.borrow().stage;
        self.stage(stage, detail, extra)
    }

    fn engine_event(&self, name: &str, payload: Value) {
        // a failed status write inside an engine callback is logged, like Python's listener guard
        let logged = |r: std::io::Result<()>| {
            if let Err(e) = r {
                eprintln!("could not update run status: {e}");
            }
        };
        match name {
            "stage" => {
                // the engine's own seeding steps become details of the SEEDING stage
                let step = payload.get("stage").and_then(Value::as_str).unwrap_or("").to_string();
                logged(self.detail(&engine_step(&step), json!({"step": step})));
                return;
            }
            "round_start" => {
                let r = payload.get("round").cloned().unwrap_or(Value::Null);
                let total = self.status.borrow().total_rounds;
                let batches = payload.get("batches").cloned().unwrap_or(json!(0));
                logged(self.update(|s| s.detail = format!("round {r} of {total}: {batches} batches deciding")));
            }
            "batch_failed" => self.record_failed_batch(&payload),
            _ => {}
        }
        self.emit(name, payload);
    }

    /// Keeps failed batches in status.json so they survive a reload (the SSE event alone is transient).
    fn record_failed_batch(&self, payload: &Value) {
        let round = payload.get("round").and_then(Value::as_i64).unwrap_or(self.status.borrow().round);
        let batch = match payload.get("batch") {
            Some(Value::Number(n)) => BatchRef::Index(n.as_i64().unwrap_or(0)),
            Some(Value::String(s)) => BatchRef::Name(s.clone()),
            _ => BatchRef::Name("?".into()),
        };
        let agents = payload
            .get("agents")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|x| x.as_str().map_or_else(|| x.to_string(), str::to_string)).collect())
            .unwrap_or_default();
        let error = match payload.get("error") {
            Some(Value::String(s)) => s.clone(),
            _ => String::new(),
        };
        let batch = FailedBatch { round, batch, agents, error: py_prefix(&error, FAILED_BATCH_ERROR_MAX).into() };
        if let Err(e) = self.update(|s| s.failed_batches.push(batch)) {
            eprintln!("could not update run status: {e}");
        }
    }

    fn finish_stopped(&self) -> std::io::Result<()> {
        let round = self.status.borrow().round;
        self.update(|s| {
            s.state = RunState::Stopped;
            s.detail = format!("stopped after round {round}");
        })?;
        self.emit("run_stopped", json!({}));
        Ok(())
    }
}

/// Marks the run stopped when the pipeline future is dropped mid-run (Python's cancellation branch).
struct CancelGuard<'t, 'a> {
    tracker: &'t Tracker<'a>,
    armed: bool,
}

impl Drop for CancelGuard<'_, '_> {
    fn drop(&mut self) {
        if self.armed {
            if let Err(e) = self.tracker.finish_stopped() {
                eprintln!("could not update run status: {e}");
            }
        }
    }
}

/// Runs a whole simulation in `run_dir`.
///
/// With `config.cast_from` set, the graph and persona stages are skipped: their files are copied from `cast_dir`
/// (default: the sibling run folder named `cast_from`), and both stages are still announced as done.
///
/// Returns the report, or None when the run was stopped before a single round completed. `stop` is checked
/// between stages and between rounds: the current round always finishes, then the run is marked `stopped` and the
/// report is still written if at least one round completed. On an error the run is marked `failed`, `run_failed`
/// is emitted and the error is returned; when the future is dropped (cancelled) the run is marked `stopped`.
#[allow(clippy::too_many_arguments)]
pub async fn run_pipeline(
    llm: &dyn Llm,
    run_dir: &Path,
    config: &SimConfig,
    seeds: &[PathBuf],
    on_event: Option<OnEvent<'_>>,
    stop: Option<&AtomicBool>,
    cast_dir: Option<&Path>,
    report: &dyn ReportStage,
) -> Result<Option<Report>, PipelineError> {
    std::fs::create_dir_all(run_dir)?;
    if !run_dir.join(CONFIG_FILE).exists() {
        write_json(&run_dir.join(CONFIG_FILE), config)?;
    }
    let track = Tracker::new(run_dir, config, on_event);
    let mut guard = CancelGuard { tracker: &track, armed: true };
    let result = pipeline(llm, run_dir, config.clone(), seeds, stop, cast_dir, report, &track).await;
    guard.armed = false;
    if let Err(e) = &result {
        let message = e.to_string();
        let message = if message.is_empty() { "Exception".to_string() } else { message };
        if let Err(write_error) = track.update(|s| {
            s.state = RunState::Failed;
            s.error = Some(message.clone());
            s.detail = "failed".into();
        }) {
            eprintln!("could not update run status: {write_error}");
        }
        track.emit("run_failed", json!({"error": message}));
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn pipeline(
    llm: &dyn Llm,
    run_dir: &Path,
    mut config: SimConfig,
    seeds: &[PathBuf],
    stop: Option<&AtomicBool>,
    cast_dir: Option<&Path>,
    report: &dyn ReportStage,
    track: &Tracker<'_>,
) -> Result<Option<Report>, PipelineError> {
    let stopping = || stop.is_some_and(|s| s.load(Ordering::SeqCst));
    let plural = if seeds.len() != 1 { "s" } else { "" };
    track.stage(RunStage::Reading, &format!("reading {} seed file{plural}", seeds.len()), json!({}))?;
    let docs = load_documents(seeds).map_err(|e| RunError(format!("could not read seed files: {e}")))?;
    let chunks = chunk_text_default(&docs);
    if chunks.is_empty() {
        return Err(RunError("no readable text found in the seed files".into()).into());
    }
    if stopping() {
        track.finish_stopped()?;
        return Ok(None);
    }
    let n_chunks = chunks.len() as i64;

    let world;
    let graph;
    let first_round;
    if let (Some(branched_from), Some(branch_round)) = (config.branched_from.clone(), config.branch_round) {
        // A branch: rounds 0..branch_round are already in the copied world.
        first_round = branch_round + 1;
        graph = read_graph(&run_dir.join(GRAPH_FILE))?;
        track.stage(
            RunStage::Graph,
            &format!("reusing the topic map of {branched_from}"),
            json!({"reused": true, "estimated_calls": estimate_calls(&config, n_chunks)}),
        )?;
        track.stage(RunStage::Personas, &format!("reusing the people of {branched_from}"), json!({"reused": true}))?;
        world = World::open(&run_dir.join(WORLD_FILE), config.seed)?;
        let personas = world.personas()?.len();
        track.stage(
            RunStage::Seeding,
            &format!("rounds 0 to {branch_round} copied from {branched_from}"),
            json!({"reused": true, "personas": personas}),
        )?;
        track.update(|s| s.round = branch_round)?;
    } else {
        first_round = 1;
        let personas;
        if let Some(cast_from) = config.cast_from.clone() {
            let source = cast_dir.map(Path::to_path_buf).unwrap_or_else(|| {
                run_dir.parent().map(|p| p.join(&cast_from)).unwrap_or_else(|| PathBuf::from(&cast_from))
            });
            let cast = load_cast(&source)?;
            graph = cast.graph;
            personas = cast.personas;
            if config.num_agents != personas.len() as i64 {
                // the crowd is the source's, whatever size was asked for
                config.num_agents = personas.len() as i64;
                write_json(&run_dir.join(CONFIG_FILE), &config)?;
            }
            track.stage(
                RunStage::Graph,
                &format!("reusing the topic map of {cast_from}"),
                json!({"chunks": chunks.len(), "reused": true, "estimated_calls": estimate_calls(&config, n_chunks)}),
            )?;
            write_json(&run_dir.join(GRAPH_FILE), &graph)?;
            track.stage(
                RunStage::Personas,
                &format!("reusing the {} people of {cast_from}", personas.len()),
                json!({"entities": graph.entities.len(), "relations": graph.relations.len(), "reused": true}),
            )?;
            write_json_list(&run_dir.join(PERSONAS_FILE), &personas)?;
        } else {
            let selected = select_chunks(&chunks, &config.question, MAX_EXTRACT_CHUNKS);
            let detail = if selected.len() == chunks.len() {
                format!("extracting from {} chunks", chunks.len())
            } else {
                format!(
                    "extracting from the {} of {} chunks most relevant to the question",
                    selected.len(),
                    chunks.len()
                )
            };
            track.stage(
                RunStage::Graph,
                &detail,
                json!({"chunks": chunks.len(), "estimated_calls": estimate_calls(&config, n_chunks)}),
            )?;
            let progress = |message: &str| {
                if let Err(e) = track.detail(message, json!({})) {
                    eprintln!("could not update run status: {e}");
                }
            };
            graph = extract_graph(llm, &selected, &config.question, &config.planner_model, Some(&progress)).await?;
            write_json(&run_dir.join(GRAPH_FILE), &graph)?;
            if stopping() {
                track.finish_stopped()?;
                return Ok(None);
            }
            track.stage(
                RunStage::Personas,
                &format!("creating {} personas", config.num_agents),
                json!({"entities": graph.entities.len(), "relations": graph.relations.len()}),
            )?;
            personas = generate_personas(
                llm,
                &graph,
                &config.question,
                config.num_agents.max(0) as usize,
                &config.planner_model,
                &config.language,
            )
            .await?;
            write_json_list(&run_dir.join(PERSONAS_FILE), &personas)?;
        }
        if stopping() {
            track.finish_stopped()?;
            return Ok(None);
        }
        world = World::open(&run_dir.join(WORLD_FILE), config.seed)?;
        world.add_personas(&personas)?;
        track.stage(RunStage::Seeding, "seeding follows and opening posts", json!({"personas": personas.len()}))?;
        let engine = Engine::new(llm, &world, &config, Some(Box::new(|n: &str, p: Value| track.engine_event(n, p))));
        engine.seed_world(&graph).await?;
        track.update(|s| s.round = 0)?;
    }

    let engine = Engine::new(llm, &world, &config, Some(Box::new(|n: &str, p: Value| track.engine_event(n, p))));
    track.stage(RunStage::Rounds, &format!("simulating {} rounds", config.rounds), json!({}))?;
    for r in first_round..=config.rounds {
        if stopping() {
            break;
        }
        engine.run_round(r).await?;
        track.update(|s| {
            s.round = r;
            s.detail = format!("round {r} of {} done", config.rounds);
        })?;
    }

    let stopped = stopping();
    let round = track.status.borrow().round;
    if stopped && round < 1 {
        track.finish_stopped()?;
        return Ok(None);
    }
    let note = if stopped { format!(" (stopped after round {round})") } else { String::new() };
    track.stage(RunStage::Report, &format!("writing the report{note}"), json!({}))?;
    let written = report.write_report(llm, run_dir, &world, &graph, &config).await?;
    if stopped {
        track.update(|s| s.stage = RunStage::Finished)?;
        track.finish_stopped()?;
    } else {
        track.update(|s| {
            s.state = RunState::Done;
            s.stage = RunStage::Finished;
            s.detail = "report ready".into();
        })?;
        track.emit("run_done", json!({}));
    }
    Ok(Some(written))
}

fn read_graph(path: &Path) -> Result<KnowledgeGraph, PipelineError> {
    let text = std::fs::read_to_string(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => {
            PipelineError::Other(format!("[Errno 2] No such file or directory: '{}'", path.display()))
        }
        _ => PipelineError::Io(e),
    })?;
    serde_json::from_str(&text).map_err(|e| PipelineError::Other(e.to_string()))
}

/// Marks a run that is still `running` on disk as `stopped` (e.g. after Ctrl-C outside the pipeline).
pub fn mark_stopped(run_dir: &Path) -> std::io::Result<()> {
    if let Some(mut status) = read_status(run_dir) {
        if matches!(status.state, RunState::Running | RunState::Queued) {
            status.state = RunState::Stopped;
            status.detail = format!("stopped after round {}", status.round);
            status.updated_at = now_iso();
            write_status(run_dir, &status)?;
        }
    }
    Ok(())
}
