//! `tellfold mcp`: a Model Context Protocol server on stdin/stdout, so coding agents (Claude Code, Claude Desktop,
//! Cursor) can start simulations, wait for them and read the reports.
//!
//! The tools talk to the engine's HTTP API (`tellfold-server`). By default the command starts that API inside its
//! own process, on a free port of 127.0.0.1 with a random token, over the runs folder of the app's settings, so runs
//! started here show up in the app. With `--url` (and the server's token) it talks to a server that is already
//! running instead.
//!
//! Simulations take minutes, so no tool blocks on one: `start_simulation` returns a run id at once, and
//! `wait_for_run` waits at most a couple of minutes per call before it answers with the progress so far.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rand::distributions::{Alphanumeric, DistString};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{tool, tool_handler, tool_router, ErrorData, ServerHandler, ServiceExt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use tellfold_engine::material::{MAX_MATERIAL_FILES, MAX_MATERIAL_FILE_BYTES};
use tellfold_llm::ingest::{self, TEXT_SUFFIXES};
use tellfold_runtimes::load_settings;
use tellfold_runtimes::process::kill_live_process_groups;
use tellfold_runtimes::settings::tellfold_home;
use tellfold_server::upload::{MAX_FILES, MAX_FILE_BYTES, MAX_TOTAL_BYTES};
use tellfold_server::{create_app, log, AppOptions, ManagerOptions};

use crate::args::Parsed;
use crate::commands::{Done, Exit};
use crate::llm::fake_mode;
use crate::paths::py_resolve;
use crate::serve::{bind, examples_folder, exit_later, expanduser, llm_factory, os_error_text, run_server};
use crate::ui::Console;
use crate::VERSION;

/// The longest `wait_for_run` / `get_study` wait in one call: well under the time-outs of MCP clients.
const MAX_WAIT_SECONDS: u64 = 120;
const DEFAULT_WAIT_SECONDS: u64 = 30;
/// How long `compare_drafts` waits for draft A's people before it leaves starting draft B to a background task.
const COMPARE_INLINE_WAIT: Duration = Duration::from_secs(20);
/// How long that background task waits for them.
const COMPARE_BACKGROUND_WAIT: Duration = Duration::from_secs(30 * 60);
const TERMINAL_STATES: [&str; 3] = ["done", "failed", "stopped"];
/// The longest pause between two polls of a run or a study.
const MAX_POLL_PAUSE: Duration = Duration::from_secs(2);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Reads of runs and studies, and the quick actions (stop, events, check), answer at once. Asking the crowd or a
/// person waits on an AI tool, so has no limit.
const QUICK_TIMEOUT: Duration = Duration::from_secs(30);
/// The shortest time-out of a poll's request after the first.
const MIN_POLL_REQUEST: Duration = Duration::from_secs(5);
/// Uploading the seeds of a new run (at most 100 MB, to this machine).
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const LOGGER: &str = "tellfold.mcp";

const INSTRUCTIONS: &str = "Tellfold simulates how a crowd of AI personas reacts to an announcement (a changelog, \
a pricing change, a launch post) and writes a cited report. Runs take minutes: start_simulation returns a run id at \
once; call wait_for_run until it says finished, then get_report. To test two versions of a text on the same crowd, \
use compare_drafts and then get_study. ask_crowd and interview_person work on finished runs.";

// ---------- the engine's HTTP API ----------

/// A client of one Tellfold HTTP API.
#[derive(Clone)]
pub struct Engine {
    base: String,
    /// None for a server without a token (`tellfold ui`)
    token: Option<String>,
    http: reqwest::Client,
}

/// A failed request to the engine.
struct EngineError {
    message: String,
    /// the engine did not answer in time (it may only be busy)
    timed_out: bool,
}

/// One seed document to upload.
struct Seed {
    name: String,
    bytes: Vec<u8>,
}

impl Engine {
    pub fn new(base: &str, token: Option<&str>) -> Self {
        let http =
            reqwest::Client::builder().no_proxy().connect_timeout(CONNECT_TIMEOUT).build().expect("an HTTP client");
        Engine { base: base.trim_end_matches('/').to_string(), token: token.map(str::to_string), http }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// The body of a 2xx response, or the API's `detail` (or the status) as the error.
    async fn send(&self, request: reqwest::RequestBuilder) -> Result<String, String> {
        self.send_timed(request).await.map_err(|e| e.message)
    }

    /// `send`, telling a time-out apart from other failures.
    async fn send_timed(&self, mut request: reqwest::RequestBuilder) -> Result<String, EngineError> {
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(|e| EngineError {
            message: format!("cannot reach the Tellfold engine at {}: {e}", self.base),
            timed_out: e.is_timeout(),
        })?;
        let status = response.status();
        let text = response.text().await.map_err(|e| EngineError {
            message: format!("could not read the engine's answer: {e}"),
            timed_out: e.is_timeout(),
        })?;
        if status.is_success() {
            return Ok(text);
        }
        let detail = serde_json::from_str::<Value>(&text).ok().and_then(|v| match v.get("detail") {
            Some(Value::String(s)) => Some(s.clone()),
            Some(other) if !other.is_null() => Some(other.to_string()),
            _ => None,
        });
        let message = match detail {
            Some(detail) => format!("{detail} (HTTP {})", status.as_u16()),
            None => format!("the engine answered HTTP {}", status.as_u16()),
        };
        Err(EngineError { message, timed_out: false })
    }

    async fn json(&self, request: reqwest::RequestBuilder) -> Result<Value, String> {
        let text = self.send(request).await?;
        serde_json::from_str(&text).map_err(|e| format!("the engine sent invalid JSON: {e}"))
    }

    pub async fn get(&self, path: &str) -> Result<Value, String> {
        self.get_within(path, QUICK_TIMEOUT).await
    }

    /// `get` with its own time-out, telling a time-out apart from other failures.
    async fn get_timed(&self, path: &str, timeout: Duration) -> Result<Value, EngineError> {
        let text = self.send_timed(self.http.get(self.url(path)).timeout(timeout)).await?;
        serde_json::from_str(&text)
            .map_err(|e| EngineError { message: format!("the engine sent invalid JSON: {e}"), timed_out: false })
    }

    async fn get_within(&self, path: &str, timeout: Duration) -> Result<Value, String> {
        self.get_timed(path, timeout).await.map_err(|e| e.message)
    }

    pub async fn get_text(&self, path: &str) -> Result<String, String> {
        self.send(self.http.get(self.url(path)).timeout(QUICK_TIMEOUT)).await
    }

    /// A quick action: stop a run, add an event, start a check.
    pub async fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        self.json(self.http.post(self.url(path)).json(body).timeout(QUICK_TIMEOUT)).await
    }

    /// A question the engine answers with an AI tool (`/ask`, `/chat`): no time-out.
    pub async fn post_slow(&self, path: &str, body: &Value) -> Result<Value, String> {
        self.json(self.http.post(self.url(path)).json(body)).await
    }

    /// `POST /api/runs` with the form fields and the seed files.
    async fn create_run(&self, fields: &[(String, String)], seeds: Vec<Seed>) -> Result<String, String> {
        self.create_grounded_run(fields, seeds, None).await
    }

    /// [`Engine::create_run`] with audience material for the cast.
    async fn create_grounded_run(
        &self,
        fields: &[(String, String)],
        seeds: Vec<Seed>,
        material: Option<MaterialUpload>,
    ) -> Result<String, String> {
        let mut form = reqwest::multipart::Form::new();
        for (key, value) in fields {
            form = form.text(key.clone(), value.clone());
        }
        for seed in seeds {
            form = form.part("files", reqwest::multipart::Part::bytes(seed.bytes).file_name(seed.name));
        }
        if let Some(material) = material {
            form = material.into_form(form);
        }
        let created = self.json(self.http.post(self.url("/api/runs")).multipart(form).timeout(UPLOAD_TIMEOUT)).await?;
        created["id"].as_str().map(str::to_string).ok_or_else(|| "the engine did not return a run id".into())
    }

    /// `POST /api/material/preview`: what the engine would send of this material.
    async fn preview_material(&self, material: MaterialUpload, agents: Option<i64>) -> Result<Value, String> {
        let mut form = material.into_form(reqwest::multipart::Form::new());
        if let Some(agents) = agents {
            form = form.text("agents", agents.to_string());
        }
        self.json(self.http.post(self.url("/api/material/preview")).multipart(form).timeout(UPLOAD_TIMEOUT)).await
    }
}

/// Audience material read from disk, ready for a form: the files, what each holds, and the extra words to hide.
struct MaterialUpload {
    files: Vec<Seed>,
    spec: Vec<Value>,
    hide: Vec<String>,
}

impl MaterialUpload {
    fn into_form(self, mut form: reqwest::multipart::Form) -> reqwest::multipart::Form {
        for file in self.files {
            form = form.part("material", reqwest::multipart::Part::bytes(file.bytes).file_name(file.name));
        }
        form = form.text("material_spec", Value::Array(self.spec).to_string());
        if !self.hide.is_empty() {
            form = form.text("material_hide", self.hide.join("\n"));
        }
        form
    }
}

/// The material files given to a tool, read; None without any.
fn collect_material(given: &[MaterialIn], hide: &[String]) -> Result<Option<MaterialUpload>, String> {
    if given.is_empty() {
        return Ok(None);
    }
    if given.len() > MAX_MATERIAL_FILES {
        return Err(format!("at most {MAX_MATERIAL_FILES} files of audience material"));
    }
    let mut files = Vec::new();
    let mut spec = Vec::new();
    for entry in given {
        let path = expanduser(Path::new(&entry.file));
        if !path.is_file() {
            return Err(format!("{}: no such file (relative paths start at {})", entry.file, cwd_text()));
        }
        let shown = path.display().to_string();
        let size = std::fs::metadata(&path).map_err(|e| format!("{shown}: {}", os_error_text(&e)))?.len();
        if size > MAX_MATERIAL_FILE_BYTES as u64 {
            return Err(format!("{shown} is larger than {} MB", MAX_MATERIAL_FILE_BYTES >> 20));
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("{shown}: {}", os_error_text(&e)))?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "material.txt".into());
        files.push(Seed { name, bytes });
        spec.push(json!({"kind": entry.kind, "column": entry.column}));
    }
    Ok(Some(MaterialUpload { files, spec, hide: hide.to_vec() }))
}

fn run_path(run_id: &str, rest: &str) -> String {
    let id = percent_encoding::utf8_percent_encode(run_id, percent_encoding::NON_ALPHANUMERIC);
    format!("/api/runs/{id}{rest}")
}

fn study_path(study_id: &str) -> String {
    format!("/api/studies/{}", percent_encoding::utf8_percent_encode(study_id, percent_encoding::NON_ALPHANUMERIC))
}

// ---------- seeds ----------

/// The file name `text` is uploaded under: `title` when it ends in a text suffix, else `title.md`.
fn text_name(title: Option<&str>) -> String {
    let title = title.map(str::trim).filter(|t| !t.is_empty()).unwrap_or("announcement");
    let lower = title.to_lowercase();
    if TEXT_SUFFIXES.iter().any(|s| lower.ends_with(s)) {
        title.to_string()
    } else {
        format!("{title}.md")
    }
}

/// The upload so far, held to the server's limits before anything more is read.
#[derive(Default)]
struct Budget {
    files: usize,
    bytes: usize,
}

impl Budget {
    fn add(&mut self, name: &str, size: u64) -> Result<(), String> {
        let size = usize::try_from(size).unwrap_or(usize::MAX);
        if size > MAX_FILE_BYTES {
            return Err(format!("{name} is larger than {} MB", MAX_FILE_BYTES >> 20));
        }
        self.files += 1;
        self.bytes = self.bytes.saturating_add(size);
        if self.files > MAX_FILES {
            return Err(format!("too many seed files: a run takes at most {MAX_FILES}"));
        }
        if self.bytes > MAX_TOTAL_BYTES {
            return Err(format!("the seed files add up to more than {} MB", MAX_TOTAL_BYTES >> 20));
        }
        Ok(())
    }
}

/// The seed files `given` stands for, by the rules `tellfold run` reads seeds with: a supported file, or the
/// supported files under a folder (symlinked folders are not followed).
fn seed_files(given: &str) -> Result<Vec<PathBuf>, String> {
    let path = expanduser(Path::new(given));
    if !path.exists() {
        return Err(format!("{given}: no such file or folder (relative paths start at {})", cwd_text()));
    }
    // the walk stops at one file more than a run takes, which is enough to say the limit is broken
    let found = ingest::expand_at_most(std::slice::from_ref(&path), MAX_FILES).map_err(|e| e.to_string())?;
    if found.is_empty() {
        return Err(format!("{given} holds no .md, .markdown, .txt or .pdf files"));
    }
    Ok(found)
}

/// The documents of a run: `text` (saved as `title`) and every file or folder in `files`.
fn collect_seeds(text: Option<&str>, title: Option<&str>, files: &[String]) -> Result<Vec<Seed>, String> {
    let mut seeds = Vec::new();
    let mut budget = Budget::default();
    if let Some(text) = text.filter(|t| !t.trim().is_empty()) {
        let name = text_name(title);
        budget.add(&name, text.len() as u64)?;
        seeds.push(Seed { name, bytes: text.as_bytes().to_vec() });
    }
    for given in files {
        for file in seed_files(given)? {
            let shown = file.display().to_string();
            let size = std::fs::metadata(&file).map_err(|e| format!("{shown}: {}", os_error_text(&e)))?.len();
            budget.add(&shown, size)?;
            let bytes = std::fs::read(&file).map_err(|e| format!("{shown}: {}", os_error_text(&e)))?;
            let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "seed.md".into());
            seeds.push(Seed { name, bytes });
        }
    }
    if seeds.is_empty() {
        return Err("give the text to test as `text`, or seed documents as `files`".into());
    }
    Ok(seeds)
}

fn cwd_text() -> String {
    std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_else(|_| "the working directory".into())
}

// ---------- what the tools answer ----------

fn ok(value: Value) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::structured(value))
}

/// A failure the caller should read (a bad id, a run that is not finished): a tool error, not a protocol error.
fn fail(message: impl Into<String>) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::error(vec![ContentBlock::text(message.into())]))
}

/// Rough share of the work done, from the stage and round (the rounds are most of a run).
fn progress(state: &str, stage: &str, round: i64, total_rounds: i64) -> f64 {
    if state == "done" {
        return 1.0;
    }
    let value = match stage {
        "reading" => 0.02,
        "graph" => 0.05,
        "personas" => 0.10,
        "seeding" => 0.15,
        "rounds" => 0.15 + 0.75 * (round as f64 / total_rounds.max(1) as f64).clamp(0.0, 1.0),
        "report" => 0.92,
        "finished" => 1.0,
        _ => 0.0,
    };
    (value * 100.0).round() / 100.0
}

/// What the caller should do next with a run.
fn next_step(state: &str, has_report: bool, pause: Option<&str>) -> &'static str {
    match (state, has_report) {
        ("done", _) | (_, true) => "Call get_report for the report.",
        ("failed", _) => "The run failed; see error.",
        ("stopped", _) => "The run was stopped.",
        _ if pause.is_some() => "Waiting for the cast to be checked in the Tellfold app.",
        _ => "Still running: call wait_for_run again.",
    }
}

/// The parts of `GET /api/runs/{id}` a caller needs to follow a run.
fn run_status(detail: &Value) -> Value {
    let status = &detail["status"];
    let config = &detail["config"];
    let state = status["state"].as_str().unwrap_or("queued");
    let stage = status["stage"].as_str().unwrap_or("reading");
    let round = status["round"].as_i64().unwrap_or(0);
    let total = status["total_rounds"].as_i64().or_else(|| config["rounds"].as_i64()).unwrap_or(0);
    let has_report = !detail["report"].is_null();
    // set while the run waits for its cast to be confirmed in the app
    let pause = status["pause"].as_str().filter(|p| !p.is_empty());
    let mut out = json!({
        "run_id": detail["id"],
        "question": config["question"],
        "state": state,
        "stage": stage,
        "round": round,
        "total_rounds": total,
        "progress": progress(state, stage, round, total),
        "detail": status["detail"],
        "finished": TERMINAL_STATES.contains(&state),
        "has_report": has_report,
        "people": detail["personas"].as_array().map_or(0, Vec::len),
        "next": next_step(state, has_report, pause),
    });
    if let Some(pause) = pause {
        out["pause"] = json!(pause);
    }
    if let Some(error) = status["error"].as_str().filter(|e| !e.is_empty()) {
        out["error"] = json!(error);
    }
    if let Some(failed) = status["failed_batches"].as_array().filter(|f| !f.is_empty()) {
        out["failed_batches"] = json!(failed.len());
    }
    for key in ["study_id", "variant"] {
        if let Some(value) = config[key].as_str().filter(|v| !v.is_empty()) {
            out[key] = json!(value);
        }
    }
    if let Ok(material) = serde_json::from_value::<tellfold_models::MaterialSummary>(config["material"].clone()) {
        out["grounded_in"] = json!(tellfold_models::material_counts_text(&material.counts));
    }
    copy_read_only_flags(detail, out.as_object_mut().expect("an object"));
    out
}

/// `example` and `imported`, when set: such runs can be read, not changed.
fn copy_read_only_flags(run: &Value, out: &mut Map<String, Value>) {
    for key in ["example", "imported"] {
        if run[key].as_bool() == Some(true) {
            out.insert(key.into(), json!(true));
        }
    }
}

/// Polls from 250 ms apart, backing off to 2 s, until a deadline.
struct Poll {
    deadline: Instant,
    pause: Duration,
}

impl Poll {
    fn until(limit: Duration) -> Self {
        Poll { deadline: Instant::now() + limit, pause: Duration::from_millis(250) }
    }

    /// Sleeps until the next poll; false once the deadline has passed.
    async fn wait(&mut self) -> bool {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        tokio::time::sleep(self.pause.min(left)).await;
        self.pause = (self.pause * 2).min(MAX_POLL_PAUSE);
        true
    }

    /// The time-out of one poll's request. The first read of a wait gets the whole `QUICK_TIMEOUT`, as it has
    /// nothing to fall back on. A later one gets what is left of the wait (at least `MIN_POLL_REQUEST`), so a busy
    /// engine cannot stretch the wait much: if it times out, the caller answers with what it read last.
    fn request_timeout(&self, first: bool) -> Duration {
        if first {
            return QUICK_TIMEOUT;
        }
        self.deadline.saturating_duration_since(Instant::now()).clamp(MIN_POLL_REQUEST, QUICK_TIMEOUT)
    }
}

fn clamp_wait(seconds: Option<u64>) -> Duration {
    Duration::from_secs(seconds.unwrap_or(DEFAULT_WAIT_SECONDS).min(MAX_WAIT_SECONDS))
}

// ---------- tool descriptions ----------

const START_SIMULATION: &str = "Starts a simulation of how a crowd of AI personas reacts to a text \
(announcement, changelog, pricing change, launch post) and returns its run id at once. A run takes minutes: follow it \
with wait_for_run, then read get_report.";
const PREVIEW_MATERIAL: &str = "Show exactly what start_simulation would send of your audience material: every item \
after names, email addresses, phone numbers, street addresses and order or account numbers were removed on this \
computer, what was hidden, warnings when the material is thin, and the extra model calls reading it costs. Show it \
to the user before starting a run with it.";
const GET_RUN_STATUS: &str = "The state, stage, round and rough progress (0-1) of a run, without waiting. pause is \
set while the run waits for its cast to be checked in the Tellfold app. Example and imported runs (example or \
imported true) are read-only: they can be read and asked about, not changed.";
const WAIT_FOR_RUN: &str = "Wait until a run finishes, or until timeout_seconds pass (default 30, at most 120), and \
return its status. Call it again while finished is false. While pause is set, the run waits for someone to check its \
cast in the Tellfold app: tell the user before waiting again.";
const GET_REPORT: &str = "The report of a finished run: the prediction, confidence, camps, reasons, risks and \
recommendations, each citing the posts behind it. format: \"markdown\" (default), \"json\" or \"both\".";
const LIST_RUNS: &str = "The runs in the runs folder, newest first, with their state and question. Example and \
imported runs (example or imported true) are read-only: they can be read and asked about, not changed.";
const LIST_PEOPLE: &str = "The simulated people of a run: id, handle, name, bio and starting stance.";
const ASK_CROWD: &str = "Ask a question about how the crowd reacted in a finished run. The report agent answers from \
the posts and cites them.";
const INTERVIEW_PERSON: &str = "Interview one simulated person of a run, by id or handle; they answer in character.";
const INJECT_EVENT: &str = "Inject breaking news into a running simulation at a round that has not started, e.g. a \
competitor's response or a correction.";
const STOP_RUN: &str = "Stop a running simulation. What it has so far is kept.";
const COMPARE_DRAFTS: &str = "Compare two versions of a text on the same crowd: draft A runs first, then \
draft B is shown to the same people with the same settings, and a cited comparison is written once both finish. \
Returns the run ids at once; follow with get_study. While draft A is still making its people, run_b and study_id are \
null and draft B starts later from this MCP session, so keep the session open until get_study (with run_id set to \
run_a) returns a study_id.";
const GET_STUDY: &str = "A study: the runs of compare_drafts with their cited comparison, or the repeats of check_run \
with what held across them. ready is true once every run has ended and the result covers them all; error says why \
a result will not be written (e.g. fewer than two runs have a report). wait_seconds waits for either, and for a draft \
B that has not started yet.";
const CHECK_RUN: &str = "Check a finished run: run it again with the same people and text but other random seeds \
(runs in all, 2-5), then say which findings held. Returns a study id; follow with get_study.";

// ---------- tool parameters ----------

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct EventIn {
    /// The round it happens at (1 to the number of rounds).
    pub round: i64,
    /// What happens, e.g. "A competitor cuts its price in half."
    pub content: String,
}

/// How a new run is set up.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct RunOptions {
    /// Number of simulated people, 2-200 (engine default 20). Fewer is faster.
    #[serde(default)]
    pub agents: Option<i64>,
    /// Simulation rounds, 1-50 (engine default 6). Fewer is faster.
    #[serde(default)]
    pub rounds: Option<i64>,
    /// Language the people post in and the report is written in (default English).
    #[serde(default)]
    pub language: Option<String>,
    /// Breaking news that happens at a given round.
    #[serde(default)]
    pub events: Vec<EventIn>,
    /// Runtime for the people's decisions (see the app's Settings or `tellfold runtimes`). Default: settings.
    #[serde(default)]
    pub agent_runtime: Option<String>,
    /// Runtime for the topic map, the people and the report. Default: settings.
    #[serde(default)]
    pub planner_runtime: Option<String>,
    /// Model for the people's decisions (default "haiku").
    #[serde(default)]
    pub agent_model: Option<String>,
    /// Model for the topic map, the people and the report (default "sonnet").
    #[serde(default)]
    pub planner_model: Option<String>,
}

impl RunOptions {
    /// The options a run was started with, read back from its config.
    fn of_run(config: &Value) -> Self {
        let text = |key: &str| config[key].as_str().filter(|v| !v.is_empty()).map(str::to_string);
        RunOptions {
            agents: None,
            rounds: config["rounds"].as_i64(),
            language: text("language"),
            events: serde_json::from_value(config["events"].clone()).unwrap_or_default(),
            agent_runtime: text("agent_runtime"),
            planner_runtime: text("planner_runtime"),
            agent_model: text("agent_model"),
            planner_model: text("planner_model"),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StartSimulation {
    /// What you want predicted, e.g. "How will existing users react to this pricing change?"
    pub question: String,
    /// The text to test (an announcement, changelog, launch post, email) in Markdown or plain text. Give this,
    /// `files`, or both.
    #[serde(default)]
    pub text: Option<String>,
    /// File name for `text` (default "announcement.md"; ".md" is added unless it ends in .md, .markdown or .txt).
    #[serde(default)]
    pub title: Option<String>,
    /// Seed documents (.md, .markdown, .txt, .pdf) or folders of them. Relative paths start at the folder the MCP
    /// server runs in.
    #[serde(default)]
    pub files: Vec<String>,
    /// Optional: your own audience material to build the crowd from (reviews, survey answers, support emails,
    /// social comments, meeting notes). It is redacted on this computer before anything is sent; check it first
    /// with preview_material.
    #[serde(default)]
    pub material: Vec<MaterialIn>,
    /// Extra names or words to hide in the material (staff, pupils, customers).
    #[serde(default)]
    pub hide: Vec<String>,
    #[serde(flatten)]
    pub options: RunOptions,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MaterialIn {
    /// A .txt, .md, .csv, .tsv or .eml file. Relative paths start at the folder the MCP server runs in.
    pub file: String,
    /// "reviews", "survey", "emails", "comments" or "notes". Optional for .eml (emails) and .csv/.tsv (survey).
    #[serde(default)]
    pub kind: Option<String>,
    /// For a spreadsheet: the column to read, by header or 1-based number (default: the column with the most text).
    #[serde(default)]
    pub column: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PreviewMaterial {
    /// The material files, as for start_simulation.
    pub material: Vec<MaterialIn>,
    /// Extra names or words to hide.
    #[serde(default)]
    pub hide: Vec<String>,
    /// People in the planned crowd, to warn when the material is thin for it.
    #[serde(default)]
    pub agents: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunId {
    /// The run id start_simulation or list_runs returned.
    pub run_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WaitForRun {
    /// The run id start_simulation returned.
    pub run_id: String,
    /// Longest wait in seconds before answering with the progress so far (default 30, at most 120).
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetReport {
    /// The run id.
    pub run_id: String,
    /// "markdown" (default, also "md"), "json", or "both".
    #[serde(default)]
    pub format: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListRuns {
    /// Most runs to list, newest first (default 20).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AskCrowd {
    /// A finished run.
    pub run_id: String,
    /// A question about how the crowd reacted, e.g. "What made the skeptics change their mind?"
    pub question: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Interview {
    /// A run that has people.
    pub run_id: String,
    /// The person's id ("a03") or handle ("@maria" or "maria", any case); see list_people.
    pub person: String,
    /// What to ask them. They answer in character, from what they saw and posted.
    pub message: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InjectEvent {
    /// A running run.
    pub run_id: String,
    /// The round it happens at; must be a round that has not started yet.
    pub round: i64,
    /// What happens, e.g. "The CEO apologises on the blog."
    pub content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CompareDrafts {
    /// What you want predicted, the same for both drafts. Not needed with `baseline_run_id` (its question is used).
    #[serde(default)]
    pub question: Option<String>,
    /// The first version of the text. Not needed with `baseline_run_id`.
    #[serde(default)]
    pub draft_a: Option<String>,
    /// The second version of the text.
    pub draft_b: String,
    /// An existing run to use as draft A instead of starting one: draft B is shown to its people, with its
    /// settings (the run options here are then not used).
    #[serde(default)]
    pub baseline_run_id: Option<String>,
    /// Label of draft B in the comparison (default: the next letter).
    #[serde(default)]
    pub label_b: Option<String>,
    /// How draft A is set up; draft B uses the same people and settings.
    #[serde(flatten)]
    pub options: RunOptions,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetStudy {
    /// The study id compare_drafts or check_run returned. Give this or `run_id`.
    #[serde(default)]
    pub study_id: Option<String>,
    /// A run of the study (for compare_drafts, the draft A run id).
    #[serde(default)]
    pub run_id: Option<String>,
    /// Wait up to this many seconds for the result, or for draft B to start (default 0, at most 120).
    #[serde(default)]
    pub wait_seconds: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckRun {
    /// A finished run with a report.
    pub run_id: String,
    /// Runs in all, this one included, 2-5 (default 3).
    #[serde(default)]
    pub runs: Option<i64>,
}

// ---------- the MCP server ----------

/// A draft B that has started.
#[derive(Clone, Debug)]
struct StartedB {
    run_id: String,
    study_id: Option<String>,
}

/// What `compare_drafts` knows about a draft B it could not start at once.
#[derive(Clone, Debug)]
enum PendingB {
    Waiting,
    Started(StartedB),
    Failed(String),
}

/// How waiting for draft A's people ended.
enum People {
    Ready,
    /// the wait ran out; `pause` is set while the cast waits to be checked in the app
    NotYet {
        pause: Option<String>,
    },
}

/// What `get_study` found for its arguments.
enum StudyTarget {
    Study(String),
    /// draft B of this draft A has not started yet
    WaitingForB(String),
}

#[derive(Clone)]
pub struct Tellfold {
    engine: Engine,
    /// draft A run id -> draft B
    pending: Arc<Mutex<HashMap<String, PendingB>>>,
    tool_router: ToolRouter<Self>,
}

/// The form fields of a new run.
fn run_fields(question: &str, options: &RunOptions) -> Vec<(String, String)> {
    let mut fields = vec![("question".to_string(), question.to_string())];
    if let Some(n) = options.agents {
        fields.push(("agents".into(), n.to_string()));
    }
    if let Some(n) = options.rounds {
        fields.push(("rounds".into(), n.to_string()));
    }
    for (key, value) in [
        ("language", &options.language),
        ("agent_runtime", &options.agent_runtime),
        ("planner_runtime", &options.planner_runtime),
        ("agent_model", &options.agent_model),
        ("planner_model", &options.planner_model),
    ] {
        if let Some(value) = value.as_deref().filter(|v| !v.trim().is_empty()) {
            fields.push((key.into(), value.into()));
        }
    }
    if !options.events.is_empty() {
        fields.push(("events".into(), json!(options.events).to_string()));
    }
    fields
}

/// The settings of a run that the tools do not offer (batch size, concurrency, random seed, thinking), as form
/// fields, so a draft B started from it differs only in the text.
fn hidden_fields(config: &Value) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    for (field, key) in [("batch", "batch_size"), ("concurrency", "concurrency"), ("seed", "seed")] {
        if let Some(n) = config[key].as_i64() {
            fields.push((field.into(), n.to_string()));
        }
    }
    if config["think"].as_bool() == Some(true) {
        fields.push(("think".into(), "true".into()));
    }
    fields
}

/// Draft A has its people and topic map, and no cast check in the app is pending, so draft B can copy them.
fn cast_ready(detail: &Value) -> bool {
    detail["personas"].as_array().is_some_and(|p| !p.is_empty())
        && !detail["graph"].is_null()
        && detail["status"]["pause"].is_null()
}

/// The pause of a run (`personas_ready` while its cast waits to be checked in the app), if it is paused.
fn pause_of(detail: &Value) -> Option<String> {
    detail["status"]["pause"].as_str().filter(|p| !p.is_empty()).map(str::to_string)
}

/// The answer while draft B waits for draft A's people, or for draft A's cast to be checked in the app.
fn waiting_for_b(run_a: &str, pause: Option<&str>) -> Value {
    let mut out = json!({
        "study_id": null, "ready": false, "run_a": run_a, "run_b": null,
        "next": "Draft B starts from this MCP session as soon as draft A has its people, so keep it open. Call \
                 get_study with run_id set to run_a and wait_seconds until it returns a study_id.",
    });
    if let Some(pause) = pause {
        out["pause"] = json!(pause);
        out["next"] = json!(
            "Draft A's cast is waiting to be checked in the Tellfold app. Ask the user to confirm it there, and wait \
             for them before calling get_study (run_id set to run_a) again. Draft B starts from this MCP session \
             after that, so keep it open."
        );
    }
    out
}

/// The persona `wanted` names: an id, or a handle with or without "@", in any case.
fn find_person<'a>(detail: &'a Value, wanted: &str) -> Option<&'a Value> {
    let wanted = wanted.trim();
    let handle = wanted.trim_start_matches('@').to_lowercase();
    let people = detail["personas"].as_array()?;
    people.iter().find(|p| p["id"].as_str() == Some(wanted)).or_else(|| {
        people.iter().find(|p| p["handle"].as_str().is_some_and(|h| h.trim_start_matches('@').to_lowercase() == handle))
    })
}

/// The run ids a study's comparison was written from.
fn compared_runs(study: &Value) -> Vec<&str> {
    study["compare"]["runs"].as_array().into_iter().flatten().filter_map(|r| r["run_id"].as_str()).collect()
}

/// Where a study's result stands.
#[derive(Debug, PartialEq)]
enum StudyOutcome {
    /// runs are still going, or the result is being written or is older than the runs
    Waiting,
    Ready,
    /// it will not be written; the reason
    Failed(String),
}

/// Whether a study's result is final. It is ready once every run has ended, nothing is being written, and the
/// result covers every run with a report: the server keeps an older comparison until a run added later (another
/// draft on the same crowd) has finished and been compared, so a comparison alone is not enough. Once every run has
/// ended and nothing is being written, anything else is final: writing it failed, fewer than two runs have a report,
/// or the engine stopped before it was written. An error while runs are still going is the previous attempt's: a new
/// one starts once they end.
fn study_outcome(study: &Value) -> StudyOutcome {
    let runs = study["runs"].as_array().map(Vec::as_slice).unwrap_or_default();
    let ended = |r: &Value| TERMINAL_STATES.contains(&r["state"].as_str().unwrap_or(""));
    let writing = study["compare_state"]["running"] == json!(true) || study["repeating"] == json!(true);
    if runs.is_empty() || !runs.iter().all(ended) || writing {
        return StudyOutcome::Waiting;
    }
    let reported: Vec<&str> =
        runs.iter().filter(|r| r["has_report"] == json!(true)).filter_map(|r| r["id"].as_str()).collect();
    let repeat = study["kind"] == "repeat";
    let covered = if repeat {
        study["agreement"]["runs"].as_u64().is_some_and(|n| n as usize >= reported.len())
    } else {
        let compared = compared_runs(study);
        !study["compare"].is_null() && reported.iter().all(|id| compared.contains(id))
    };
    if covered && reported.len() >= 2 {
        return StudyOutcome::Ready;
    }
    if let Some(error) = study["compare_state"]["error"].as_str().filter(|e| !e.is_empty()) {
        return StudyOutcome::Failed(error.to_string());
    }
    let what = if repeat { "a check" } else { "a comparison" };
    if reported.len() < 2 {
        let n = reported.len();
        return StudyOutcome::Failed(format!(
            "only {n} run{} finished with a report; {what} needs two",
            if n == 1 { "" } else { "s" }
        ));
    }
    // The engine starts writing the result as the last run ends, in the same step, so with every run ended and
    // nothing being written it will not come: the engine stopped while writing it (a restart, or a shutdown).
    StudyOutcome::Failed(format!(
        "the result of {what} of these runs was not written, probably because the engine stopped while writing it"
    ))
}

/// What `get_study` answers for a study.
fn study_answer(study_id: &str, study: &Value, outcome: &StudyOutcome) -> Value {
    let runs: Vec<Value> = study["runs"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            json!({"run_id": r["id"], "variant": r["variant"], "letter": r["letter"], "state": r["state"],
                   "has_report": r["has_report"]})
        })
        .collect();
    let ready = *outcome == StudyOutcome::Ready;
    let repeat = study["kind"] == "repeat";
    let mut out = json!({"study_id": study_id, "kind": study["kind"], "ready": ready, "runs": runs});
    // an older result than the runs is left out, so it is never mistaken for the answer
    if repeat {
        out["agreement"] = if ready { study["agreement"].clone() } else { Value::Null };
    } else {
        out["comparison"] = if ready { study["compare"]["report"].clone() } else { Value::Null };
        out["compared_runs"] = if ready { json!(compared_runs(study)) } else { json!([]) };
    }
    match outcome {
        StudyOutcome::Ready => {}
        StudyOutcome::Failed(error) => {
            out["error"] = json!(error);
            out["next"] = json!(if repeat {
                "No result will be written. Call check_run again on a finished run."
            } else {
                "No comparison will be written. Call compare_drafts with baseline_run_id set to a finished run."
            });
        }
        StudyOutcome::Waiting => {
            let writing = study["compare_state"]["running"] == json!(true) || study["repeating"] == json!(true);
            out["next"] = json!(if writing {
                "Being written: call get_study again with wait_seconds."
            } else {
                "Written once every run has finished: wait for the runs, then call get_study again."
            });
        }
    }
    out
}

fn no_study(run_id: &str) -> String {
    format!(
        "run {run_id} belongs to no study. If it is draft A of compare_drafts and the MCP session closed before \
         draft B started, call compare_drafts again with baseline_run_id set to {run_id}."
    )
}

impl Tellfold {
    pub fn new(engine: Engine) -> Self {
        Tellfold { engine, pending: Arc::default(), tool_router: Self::tool_router() }
    }

    async fn detail(&self, run_id: &str) -> Result<Value, String> {
        self.engine.get(&run_path(run_id, "")).await
    }

    /// One poll's read of `path`. `None` when a read after the `first` timed out: the caller keeps what it read
    /// last rather than failing on a busy engine.
    async fn polled(&self, path: &str, poll: &Poll, first: bool) -> Result<Option<Value>, String> {
        match self.engine.get_timed(path, poll.request_timeout(first)).await {
            Ok(value) => Ok(Some(value)),
            Err(e) if e.timed_out && !first => Ok(None),
            Err(e) => Err(e.message),
        }
    }

    /// The study a run belongs to: its own `study_id`, else the study that lists it (a draft A has no
    /// `study_id` of its own).
    async fn study_of(&self, run_id: &str) -> Result<Option<String>, String> {
        let detail = self.detail(run_id).await?;
        if let Some(study_id) = detail["config"]["study_id"].as_str().filter(|s| !s.is_empty()) {
            return Ok(Some(study_id.to_string()));
        }
        let studies = self.engine.get("/api/studies").await?;
        let listed = studies.as_array().into_iter().flatten().find(|study| {
            study["runs"].as_array().is_some_and(|runs| runs.iter().any(|r| r["id"].as_str() == Some(run_id)))
        });
        Ok(listed.and_then(|study| study["id"].as_str()).map(str::to_string))
    }

    /// The names the engine saved a run's seeds under (it renames clashes), else the names they were sent with.
    async fn saved_seeds(&self, run_id: &str, sent: Vec<String>) -> Vec<String> {
        match self.engine.get(&run_path(run_id, "/seeds")).await {
            Ok(Value::Array(files)) => files.iter().filter_map(|f| f["name"].as_str().map(str::to_string)).collect(),
            other => {
                let why = other.err().unwrap_or_else(|| "not a list".into());
                log::warning(
                    LOGGER,
                    &format!("cannot read the seeds of {run_id} back ({why}); listing the names sent"),
                );
                sent
            }
        }
    }

    /// Starts draft B on draft A's people: `cast_from` A, with A's question and settings.
    async fn start_b(&self, run_a: &str, draft_b: &str, label: Option<&str>) -> Result<StartedB, String> {
        let detail = self.detail(run_a).await?;
        let config = &detail["config"];
        let question = config["question"].as_str().unwrap_or_default();
        let mut fields = run_fields(question, &RunOptions::of_run(config));
        fields.extend(hidden_fields(config));
        fields.push(("cast_from".into(), run_a.to_string()));
        if let Some(label) = label.filter(|l| !l.trim().is_empty()) {
            fields.push(("variant".into(), label.to_string()));
        }
        let seeds = vec![Seed { name: "draft-b.md".into(), bytes: draft_b.as_bytes().to_vec() }];
        let run_id = self.engine.create_run(&fields, seeds).await?;
        let study_id = self.detail(&run_id).await?["config"]["study_id"].as_str().map(str::to_string);
        Ok(StartedB { run_id, study_id })
    }

    /// Waits until run A has its people, ends without them (an error), or `limit` passes.
    async fn wait_for_people(&self, run_a: &str, limit: Duration) -> Result<People, String> {
        let mut poll = Poll::until(limit);
        let mut last: Option<Value> = None;
        loop {
            if let Some(detail) = self.polled(&run_path(run_a, ""), &poll, last.is_none()).await? {
                if cast_ready(&detail) {
                    return Ok(People::Ready);
                }
                let state = detail["status"]["state"].as_str().unwrap_or("");
                if TERMINAL_STATES.contains(&state) {
                    let error = detail["status"]["error"].as_str().unwrap_or("").to_string();
                    return Err(format!("draft A ended ({state}) before it had people. {error}").trim().to_string());
                }
                last = Some(detail);
            }
            if !poll.wait().await {
                return Ok(People::NotYet { pause: last.as_ref().and_then(pause_of) });
            }
        }
    }

    /// Starts draft B once draft A has its people, in the background; `get_study` reads the outcome.
    fn start_b_later(&self, run_a: &str, draft_b: String, label: Option<String>) {
        self.pending.lock().expect("not poisoned").insert(run_a.to_string(), PendingB::Waiting);
        let me = self.clone();
        let run_a = run_a.to_string();
        tokio::spawn(async move {
            let outcome = match me.wait_for_people(&run_a, COMPARE_BACKGROUND_WAIT).await {
                Ok(People::Ready) => me.start_b(&run_a, &draft_b, label.as_deref()).await,
                Ok(People::NotYet { pause: Some(_) }) => Err(format!(
                    "draft A's cast is still waiting to be checked in the Tellfold app. Confirm it there, then call \
                     compare_drafts again with baseline_run_id set to {run_a}."
                )),
                Ok(People::NotYet { pause: None }) => Err("draft A took too long to make its people".into()),
                Err(e) => Err(e),
            };
            let state = match outcome {
                Ok(started) => PendingB::Started(started),
                Err(e) => PendingB::Failed(e),
            };
            me.pending.lock().expect("not poisoned").insert(run_a, state);
        });
    }

    /// The study `get_study` asked for: by id, or by a run of it, waiting (until `poll` ends) for a draft B
    /// that `compare_drafts` left to start in the background.
    async fn study_target(&self, p: &GetStudy, poll: &mut Poll) -> Result<StudyTarget, String> {
        if let Some(study_id) = p.study_id.as_deref().filter(|s| !s.is_empty()) {
            return Ok(StudyTarget::Study(study_id.to_string()));
        }
        let Some(run_id) = p.run_id.as_deref().filter(|r| !r.is_empty()) else {
            return Err("give study_id or run_id".into());
        };
        loop {
            let pending = self.pending.lock().expect("not poisoned").get(run_id).cloned();
            let member = match pending {
                Some(PendingB::Waiting) if poll.wait().await => continue,
                Some(PendingB::Waiting) => return Ok(StudyTarget::WaitingForB(run_id.to_string())),
                Some(PendingB::Failed(e)) => return Err(format!("draft B could not start: {e}")),
                Some(PendingB::Started(StartedB { study_id: Some(study_id), .. })) => {
                    return Ok(StudyTarget::Study(study_id))
                }
                Some(PendingB::Started(b)) => b.run_id,
                None => run_id.to_string(),
            };
            return match self.study_of(&member).await? {
                Some(study_id) => Ok(StudyTarget::Study(study_id)),
                None => Err(no_study(run_id)),
            };
        }
    }
}

#[tool_router]
impl Tellfold {
    #[tool(description = START_SIMULATION)]
    async fn start_simulation(&self, Parameters(p): Parameters<StartSimulation>) -> Result<CallToolResult, ErrorData> {
        if p.question.trim().is_empty() {
            return fail("question is required");
        }
        let seeds = match collect_seeds(p.text.as_deref(), p.title.as_deref(), &p.files) {
            Ok(seeds) => seeds,
            Err(e) => return fail(e),
        };
        let material = match collect_material(&p.material, &p.hide) {
            Ok(material) => material,
            Err(e) => return fail(e),
        };
        let fields = run_fields(&p.question, &p.options);
        let sent: Vec<String> = seeds.iter().map(|s| s.name.clone()).collect();
        let run_id = match self.engine.create_grounded_run(&fields, seeds, material).await {
            Ok(run_id) => run_id,
            Err(e) => return fail(e),
        };
        let documents = self.saved_seeds(&run_id, sent).await;
        ok(json!({
            "run_id": run_id,
            "documents": documents,
            "next": "Call wait_for_run with this run_id until finished is true, then get_report.",
        }))
    }

    #[tool(description = PREVIEW_MATERIAL)]
    async fn preview_material(&self, Parameters(p): Parameters<PreviewMaterial>) -> Result<CallToolResult, ErrorData> {
        let material = match collect_material(&p.material, &p.hide) {
            Ok(Some(material)) => material,
            Ok(None) => return fail("give at least one material file"),
            Err(e) => return fail(e),
        };
        match self.engine.preview_material(material, p.agents).await {
            Ok(preview) => ok(preview),
            Err(e) => fail(e),
        }
    }

    #[tool(description = GET_RUN_STATUS)]
    async fn get_run_status(&self, Parameters(p): Parameters<RunId>) -> Result<CallToolResult, ErrorData> {
        match self.detail(&p.run_id).await {
            Ok(detail) => ok(run_status(&detail)),
            Err(e) => fail(e),
        }
    }

    #[tool(description = WAIT_FOR_RUN)]
    async fn wait_for_run(&self, Parameters(p): Parameters<WaitForRun>) -> Result<CallToolResult, ErrorData> {
        let mut poll = Poll::until(clamp_wait(p.timeout_seconds));
        let mut last: Option<Value> = None;
        loop {
            match self.polled(&run_path(&p.run_id, ""), &poll, last.is_none()).await {
                Ok(Some(detail)) => last = Some(run_status(&detail)),
                Ok(None) => {}
                Err(e) => return fail(e),
            }
            let status = last.as_ref().expect("the first read answers or fails");
            // a run paused for its cast check keeps being waited for: the pause ends when someone confirms it
            if status["finished"] == json!(true) || !poll.wait().await {
                return ok(status.clone());
            }
        }
    }

    #[tool(description = GET_REPORT)]
    async fn get_report(&self, Parameters(p): Parameters<GetReport>) -> Result<CallToolResult, ErrorData> {
        let format = p.format.as_deref().unwrap_or("markdown").trim().to_lowercase();
        if !["markdown", "md", "json", "both"].contains(&format.as_str()) {
            return fail("format must be markdown, json or both");
        }
        let detail = match self.detail(&p.run_id).await {
            Ok(detail) => detail,
            Err(e) => return fail(e),
        };
        if detail["report"].is_null() {
            let status = run_status(&detail);
            return fail(format!(
                "run {} has no report yet (state {}, stage {}). Call wait_for_run first.",
                p.run_id, status["state"], status["stage"]
            ));
        }
        let mut content = Vec::new();
        if format != "json" {
            match self.engine.get_text(&run_path(&p.run_id, "/report.md")).await {
                Ok(markdown) => content.push(ContentBlock::text(markdown)),
                Err(e) => return fail(e),
            }
        }
        if format == "json" || format == "both" {
            content.push(ContentBlock::text(detail["report"].to_string()));
        }
        let mut result = CallToolResult::success(content);
        if format == "json" {
            result.structured_content = Some(detail["report"].clone());
        }
        Ok(result)
    }

    #[tool(description = LIST_RUNS)]
    async fn list_runs(&self, Parameters(p): Parameters<ListRuns>) -> Result<CallToolResult, ErrorData> {
        let runs = match self.engine.get("/api/runs").await {
            Ok(Value::Array(runs)) => runs,
            Ok(_) => return fail("the engine did not return a list"),
            Err(e) => return fail(e),
        };
        let total = runs.len();
        let keep = ["id", "question", "state", "stage", "round", "total_rounds", "agents", "created_at", "has_report"];
        let listed: Vec<Value> = runs
            .into_iter()
            .take(p.limit.unwrap_or(20).max(1))
            .map(|run| {
                let mut out: Map<String, Value> = keep.iter().map(|k| (k.to_string(), run[*k].clone())).collect();
                for key in ["study_id", "variant"] {
                    if !run[key].is_null() {
                        out.insert(key.into(), run[key].clone());
                    }
                }
                copy_read_only_flags(&run, &mut out);
                Value::Object(out)
            })
            .collect();
        ok(json!({"runs": listed, "total": total}))
    }

    #[tool(description = LIST_PEOPLE)]
    async fn list_people(&self, Parameters(p): Parameters<RunId>) -> Result<CallToolResult, ErrorData> {
        let detail = match self.detail(&p.run_id).await {
            Ok(detail) => detail,
            Err(e) => return fail(e),
        };
        let people: Vec<Value> = detail["personas"]
            .as_array()
            .map(|all| {
                all.iter()
                    .map(|p| {
                        let mut person = json!({"id": p["id"], "handle": p["handle"], "name": p["name"],
                                                "bio": p["bio"], "stance": p["stance"]});
                        if let Ok(g) = serde_json::from_value::<tellfold_models::Grounding>(p["grounding"].clone()) {
                            person["based_on"] = json!(tellfold_models::material_counts_text(&g.counts));
                            person["theme"] = json!(g.theme);
                        }
                        person
                    })
                    .collect()
            })
            .unwrap_or_default();
        ok(json!({"run_id": p.run_id, "people": people}))
    }

    #[tool(description = ASK_CROWD)]
    async fn ask_crowd(&self, Parameters(p): Parameters<AskCrowd>) -> Result<CallToolResult, ErrorData> {
        match self.engine.post_slow(&run_path(&p.run_id, "/ask"), &json!({"question": p.question})).await {
            Ok(reply) => ok(json!({"run_id": p.run_id, "answer": reply["reply"]})),
            Err(e) => fail(e),
        }
    }

    #[tool(description = INTERVIEW_PERSON)]
    async fn interview_person(&self, Parameters(p): Parameters<Interview>) -> Result<CallToolResult, ErrorData> {
        let detail = match self.detail(&p.run_id).await {
            Ok(detail) => detail,
            Err(e) => return fail(e),
        };
        // the chat route takes ids only
        let Some(person) = find_person(&detail, &p.person) else {
            return fail(format!(
                "run {} has no person {:?}; list_people gives their ids and handles",
                p.run_id, p.person
            ));
        };
        let body = json!({"agent_id": person["id"], "message": p.message, "history": []});
        match self.engine.post_slow(&run_path(&p.run_id, "/chat"), &body).await {
            Ok(reply) => ok(json!({
                "run_id": p.run_id, "person": person["id"], "handle": person["handle"], "reply": reply["reply"],
            })),
            Err(e) => fail(e),
        }
    }

    #[tool(description = INJECT_EVENT)]
    async fn inject_event(&self, Parameters(p): Parameters<InjectEvent>) -> Result<CallToolResult, ErrorData> {
        let body = json!({"round": p.round, "content": p.content});
        match self.engine.post(&run_path(&p.run_id, "/events"), &body).await {
            Ok(_) => ok(json!({"ok": true, "run_id": p.run_id, "round": p.round})),
            Err(e) => fail(e),
        }
    }

    #[tool(description = STOP_RUN)]
    async fn stop_run(&self, Parameters(p): Parameters<RunId>) -> Result<CallToolResult, ErrorData> {
        match self.engine.post(&run_path(&p.run_id, "/stop"), &json!({})).await {
            Ok(_) => ok(json!({"ok": true, "run_id": p.run_id})),
            Err(e) => fail(e),
        }
    }

    #[tool(description = COMPARE_DRAFTS)]
    async fn compare_drafts(&self, Parameters(p): Parameters<CompareDrafts>) -> Result<CallToolResult, ErrorData> {
        if p.draft_b.trim().is_empty() {
            return fail("draft_b is required");
        }
        let run_a = match p.baseline_run_id.as_deref().filter(|r| !r.trim().is_empty()) {
            Some(run_a) => run_a.to_string(),
            None => {
                let Some(question) = p.question.as_deref().filter(|q| !q.trim().is_empty()) else {
                    return fail("question is required (or give baseline_run_id)");
                };
                let Some(draft_a) = p.draft_a.as_deref().filter(|d| !d.trim().is_empty()) else {
                    return fail("draft_a is required (or give baseline_run_id)");
                };
                let fields = run_fields(question, &p.options);
                let seeds = vec![Seed { name: "draft-a.md".into(), bytes: draft_a.as_bytes().to_vec() }];
                match self.engine.create_run(&fields, seeds).await {
                    Ok(run_id) => run_id,
                    Err(e) => return fail(e),
                }
            }
        };
        match self.wait_for_people(&run_a, COMPARE_INLINE_WAIT).await {
            Err(e) => fail(e),
            Ok(People::Ready) => match self.start_b(&run_a, &p.draft_b, p.label_b.as_deref()).await {
                Ok(b) => ok(json!({
                    "run_a": run_a, "run_b": b.run_id, "study_id": b.study_id,
                    "next": "Call wait_for_run on both runs, then get_study with wait_seconds for the comparison.",
                })),
                Err(e) => fail(e),
            },
            Ok(People::NotYet { pause }) => {
                // Draft A is still working out its people, or waits for its cast to be checked in the app
                self.start_b_later(&run_a, p.draft_b, p.label_b);
                ok(waiting_for_b(&run_a, pause.as_deref()))
            }
        }
    }

    #[tool(description = GET_STUDY)]
    async fn get_study(&self, Parameters(p): Parameters<GetStudy>) -> Result<CallToolResult, ErrorData> {
        let mut poll = Poll::until(Duration::from_secs(p.wait_seconds.unwrap_or(0).min(MAX_WAIT_SECONDS)));
        let study_id = match self.study_target(&p, &mut poll).await {
            Ok(StudyTarget::Study(study_id)) => study_id,
            Ok(StudyTarget::WaitingForB(run_a)) => {
                // one read of draft A, so a cast waiting in the app is said, not waited on again and again
                let pause = self.detail(&run_a).await.ok().as_ref().and_then(pause_of);
                return ok(waiting_for_b(&run_a, pause.as_deref()));
            }
            Err(e) => return fail(e),
        };
        let mut last: Option<Value> = None;
        loop {
            match self.polled(&study_path(&study_id), &poll, last.is_none()).await {
                Ok(Some(study)) => last = Some(study),
                Ok(None) => {}
                Err(e) => return fail(e),
            }
            let study = last.as_ref().expect("the first read answers or fails");
            let outcome = study_outcome(study);
            if outcome != StudyOutcome::Waiting || !poll.wait().await {
                return ok(study_answer(&study_id, study, &outcome));
            }
        }
    }

    #[tool(description = CHECK_RUN)]
    async fn check_run(&self, Parameters(p): Parameters<CheckRun>) -> Result<CallToolResult, ErrorData> {
        let body = json!({"runs": p.runs.unwrap_or(3)});
        match self.engine.post(&run_path(&p.run_id, "/check"), &body).await {
            Ok(reply) => ok(json!({
                "run_id": p.run_id, "study_id": reply["study_id"],
                "next": "The repeats run one after another. Call get_study with this study_id and wait_seconds.",
            })),
            Err(e) => fail(e),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Tellfold {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("tellfold", VERSION).with_title("Tellfold"))
            .with_instructions(INSTRUCTIONS)
    }
}

// ---------- the command ----------

/// The runs folder of the server this command starts: `--runs`, else the settings' `runs_dir`, else the Mac
/// app's default (`~/Library/Application Support/Tellfold/runs`).
fn runs_folder(p: &Parsed) -> PathBuf {
    let given = p.str("runs").or_else(|| load_settings(None).runs_dir.filter(|d| !d.is_empty()));
    match given {
        Some(dir) => py_resolve(&expanduser(Path::new(&dir))),
        None => tellfold_home().join("runs"),
    }
}

pub fn mcp(_con: &Console, p: &Parsed) -> Done {
    let err = Console::stderr();
    let Some(level) = log::parse_level(&p.str("log_level").unwrap_or_else(|| "warning".into())) else {
        err.print("[bold red]error:[/] --log-level must be debug, info, warning or error");
        return Err(Exit::Code(2));
    };
    log::set_level(level);
    // the parser fills these from TELLFOLD_URL and TELLFOLD_SERVE_TOKEN
    let url = p.str("url").filter(|u| !u.trim().is_empty());
    let token = p.str("token").filter(|t| !t.is_empty());
    if token.as_ref().is_some_and(|t| t.chars().count() < 16) {
        err.print("[bold red]error:[/] --token must be at least 16 characters");
        return Err(Exit::Code(2));
    }
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("a tokio runtime");
    let result = match url {
        Some(url) => {
            err.print_soft(&format!("tellfold mcp: using the engine at {url}"));
            runtime.block_on(serve_stdio(Engine::new(&url, token.as_deref())))
        }
        None => runtime.block_on(embedded(p, token, &err)),
    };
    exit_later(crate::serve::SERVE_SHUTDOWN_EXIT);
    runtime.shutdown_timeout(Duration::from_secs(1));
    kill_live_process_groups();
    result.map_err(|e| {
        err.print(&format!("[bold red]error:[/] {e}"));
        Exit::Code(1)
    })
}

/// Serves the MCP tools on stdin/stdout until the client closes stdin.
async fn serve_stdio(engine: Engine) -> Result<(), String> {
    let service = Tellfold::new(engine)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| format!("the MCP session did not start: {e}"))?;
    if let Err(e) = service.waiting().await {
        log::warning(LOGGER, &format!("the MCP session ended with an error: {e}"));
    }
    Ok(())
}

/// Starts the HTTP API in this process, then serves the MCP tools against it; the API stops (and marks its live
/// runs stopped) when the MCP client goes away.
async fn embedded(p: &Parsed, token: Option<String>, err: &Console) -> Result<(), String> {
    let runs_dir = runs_folder(p);
    std::fs::create_dir_all(&runs_dir)
        .map_err(|e| format!("cannot create {}: {}", runs_dir.display(), os_error_text(&e)))?;
    let examples_dir = examples_folder(p, err);
    let listener = bind(0).map_err(|e| format!("cannot bind 127.0.0.1: {}", os_error_text(&e)))?;
    let port = listener.local_addr().map(|a| a.port()).map_err(|e| e.to_string())?;
    let token = token.unwrap_or_else(|| Alphanumeric.sample_string(&mut rand::thread_rng(), 32));
    let fake = fake_mode();
    let mut manager = ManagerOptions::new(&runs_dir, llm_factory());
    manager.examples_dir = examples_dir;
    let (router, manager) =
        create_app(manager, AppOptions { fake_llm: fake, token: Some(token.clone()), ..Default::default() });
    err.print_soft(&format!(
        "tellfold mcp: engine on 127.0.0.1:{port}  runs: {}{}",
        runs_dir.display(),
        if fake { "  (fake LLM)" } else { "" }
    ));
    let (closed_tx, closed_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut server = tokio::spawn(run_server(listener, router, manager, closed_rx));
    let engine = Engine::new(&format!("http://127.0.0.1:{port}"), Some(&token));
    let outcome = tokio::select! {
        outcome = serve_stdio(engine) => outcome,
        // a signal stopped the server first
        ended = &mut server => return ended.map_err(|e| e.to_string())?.map_err(|e| e.to_string()),
    };
    let _ = closed_tx.send(());
    server.await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_follows_stage_and_round() {
        assert_eq!(progress("running", "graph", 0, 6), 0.05);
        assert_eq!(progress("running", "rounds", 3, 6), 0.53);
        assert_eq!(progress("done", "report", 0, 6), 1.0);
        assert_eq!(progress("running", "rounds", 9, 0), 0.9);
    }

    #[test]
    fn text_seeds_get_a_text_name() {
        let seeds = collect_seeds(Some("# v2"), Some("changelog"), &[]).unwrap();
        assert_eq!(seeds[0].name, "changelog.md");
        let seeds = collect_seeds(Some("hi"), Some("notes.txt"), &[]).unwrap();
        assert_eq!(seeds[0].name, "notes.txt");
        // text is never uploaded as a PDF
        let seeds = collect_seeds(Some("hi"), Some("deck.PDF"), &[]).unwrap();
        assert_eq!(seeds[0].name, "deck.PDF.md");
        assert!(collect_seeds(Some("  "), None, &[]).is_err());
        assert!(collect_seeds(None, None, &["/no/such/file.md".into()]).is_err());
    }

    #[test]
    fn folders_give_their_seed_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("b.md"), "b").unwrap();
        std::fs::write(tmp.path().join("a.txt"), "a").unwrap();
        std::fs::write(tmp.path().join("image.png"), "p").unwrap();
        std::fs::create_dir(tmp.path().join("more")).unwrap();
        std::fs::write(tmp.path().join("more/c.markdown"), "c").unwrap();
        let seeds = collect_seeds(None, None, &[tmp.path().to_string_lossy().into_owned()]).unwrap();
        let names: Vec<&str> = seeds.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["a.txt", "b.md", "c.markdown"]);
        let png = tmp.path().join("image.png").to_string_lossy().into_owned();
        assert!(collect_seeds(None, None, &[png]).is_err());
        let empty = tempfile::tempdir().unwrap();
        let error = collect_seeds(None, None, &[empty.path().to_string_lossy().into_owned()]).err().unwrap();
        assert!(error.contains("holds no"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_folders_are_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.md"), "s").unwrap();
        std::fs::write(tmp.path().join("a.md"), "a").unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("link")).unwrap();
        let seeds = collect_seeds(None, None, &[tmp.path().to_string_lossy().into_owned()]).unwrap();
        let names: Vec<&str> = seeds.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["a.md"]);
    }

    #[test]
    fn a_huge_folder_is_not_walked_to_the_end() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..MAX_FILES + 30 {
            std::fs::write(tmp.path().join(format!("{i:03}.md")), "x").unwrap();
        }
        let found = ingest::expand_at_most(&[tmp.path().to_path_buf()], MAX_FILES).unwrap();
        assert_eq!(found.len(), MAX_FILES + 1);
        let error = collect_seeds(None, None, &[tmp.path().to_string_lossy().into_owned()]).err().unwrap();
        assert!(error.contains("at most 100"), "{error}");
    }

    #[test]
    fn the_upload_limits_apply_before_reading() {
        let mut budget = Budget::default();
        let error = budget.add("big.pdf", MAX_FILE_BYTES as u64 + 1).err().unwrap();
        assert!(error.contains("big.pdf is larger than 20 MB"), "{error}");
        let mut budget = Budget::default();
        for _ in 0..MAX_FILES {
            budget.add("a.md", 1).unwrap();
        }
        assert!(budget.add("a.md", 1).err().unwrap().contains("at most 100"));
        let mut budget = Budget::default();
        for _ in 0..5 {
            budget.add("a.pdf", MAX_FILE_BYTES as u64).unwrap();
        }
        assert!(budget.add("a.md", 1).err().unwrap().contains("more than 100 MB"));
    }

    #[test]
    fn a_pause_is_reported_and_holds_back_draft_b() {
        let mut detail = json!({
            "id": "r1", "config": {"question": "Q", "rounds": 3},
            "status": {"state": "running", "stage": "personas", "pause": "personas_ready"},
            "personas": [{"id": "a01", "handle": "@maria"}], "graph": {}, "report": null,
        });
        let status = run_status(&detail);
        assert_eq!(status["pause"], "personas_ready");
        assert_eq!(status["next"], "Waiting for the cast to be checked in the Tellfold app.");
        assert!(!cast_ready(&detail));
        detail["status"]["pause"] = Value::Null;
        assert!(run_status(&detail).get("pause").is_none());
        assert!(cast_ready(&detail));
    }

    #[test]
    fn read_only_runs_say_so() {
        let detail = json!({"id": "r1", "config": {}, "status": {"state": "done"}, "imported": true});
        let status = run_status(&detail);
        assert_eq!(status["imported"], true);
        assert!(status.get("example").is_none());
    }

    #[test]
    fn people_are_found_by_id_or_handle() {
        let detail = json!({"personas": [{"id": "a01", "handle": "@Maria"}, {"id": "a02", "handle": "tom"}]});
        for wanted in ["a01", "@maria", "maria", " MARIA "] {
            assert_eq!(find_person(&detail, wanted).unwrap()["id"], "a01", "{wanted}");
        }
        assert_eq!(find_person(&detail, "@tom").unwrap()["id"], "a02");
        assert!(find_person(&detail, "@nobody").is_none());
        assert!(find_person(&json!({}), "a01").is_none());
    }

    #[test]
    fn draft_b_keeps_draft_a_settings() {
        let config = json!({
            "question": "Q", "rounds": 4, "language": "German", "batch_size": 3, "concurrency": 2, "seed": 11,
            "think": true, "events": [{"round": 2, "content": "News"}], "agent_runtime": "codex", "agent_model": "",
        });
        let mut fields = run_fields("Q", &RunOptions::of_run(&config));
        fields.extend(hidden_fields(&config));
        let get = |key: &str| fields.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        assert_eq!(get("rounds"), Some("4"));
        assert_eq!(get("language"), Some("German"));
        assert_eq!(get("batch"), Some("3"));
        assert_eq!(get("concurrency"), Some("2"));
        assert_eq!(get("seed"), Some("11"));
        assert_eq!(get("think"), Some("true"));
        assert_eq!(get("agent_runtime"), Some("codex"));
        assert_eq!(get("agent_model"), None);
        assert_eq!(get("agents"), None, "the people come from cast_from");
        let events: Value = serde_json::from_str(get("events").unwrap()).unwrap();
        assert_eq!(events, json!([{"round": 2, "content": "News"}]));
    }

    fn study(runs: Value, compared: &[&str]) -> Value {
        let compare = if compared.is_empty() {
            Value::Null
        } else {
            json!({"runs": compared.iter().map(|id| json!({"run_id": id})).collect::<Vec<_>>(), "report": {"x": 1}})
        };
        json!({"kind": "compare", "runs": runs, "compare": compare, "compare_state": {"running": false}})
    }

    fn done(id: &str) -> Value {
        json!({"id": id, "state": "done", "has_report": true})
    }

    #[test]
    fn an_older_comparison_is_not_ready() {
        use StudyOutcome::*;
        // a third draft joined the study: the comparison on disk is still A against B1
        let stale = study(json!([done("a"), done("b1"), {"id": "b2", "state": "running"}]), &["a", "b1"]);
        assert_eq!(study_outcome(&stale), Waiting);
        let answer = study_answer("s1", &stale, &Waiting);
        assert!(answer["comparison"].is_null(), "{answer}");
        let finished = study(json!([done("a"), done("b1"), done("b2")]), &["a", "b1"]);
        // every run has ended and nothing is being written, yet B2 was never compared: the engine stopped
        assert!(matches!(study_outcome(&finished), Failed(e) if e.contains("was not written")));
        let answer = study_answer("s1", &finished, &study_outcome(&finished));
        assert!(answer["next"].as_str().unwrap().contains("compare_drafts"), "{answer}");
        let fresh = study(json!([done("a"), done("b1"), done("b2")]), &["a", "b1", "b2"]);
        assert_eq!(study_outcome(&fresh), Ready);
        assert_eq!(study_answer("s1", &fresh, &Ready)["compared_runs"], json!(["a", "b1", "b2"]));
        // a run that failed without a report is not waited for
        let failed = study(json!([done("a"), done("b1"), {"id": "b2", "state": "failed"}]), &["a", "b1"]);
        assert_eq!(study_outcome(&failed), Ready);
        let mut writing = fresh.clone();
        writing["compare_state"]["running"] = json!(true);
        assert_eq!(study_outcome(&writing), Waiting);
    }

    #[test]
    fn a_study_with_one_report_fails_instead_of_waiting() {
        let short = study(json!([done("a"), {"id": "b", "state": "failed", "has_report": false}]), &[]);
        let outcome = study_outcome(&short);
        assert_eq!(outcome, StudyOutcome::Failed("only 1 run finished with a report; a comparison needs two".into()));
        let answer = study_answer("s1", &short, &outcome);
        assert_eq!((answer["ready"].as_bool(), answer["error"].as_str().is_some()), (Some(false), true), "{answer}");
        assert!(answer["next"].as_str().unwrap().contains("compare_drafts"), "{answer}");
        let none = study(json!([{"id": "a", "state": "stopped"}, {"id": "b", "state": "failed"}]), &[]);
        assert!(matches!(study_outcome(&none), StudyOutcome::Failed(e) if e.starts_with("only 0 runs")));
        // a check cut off by a restart
        let cut = json!({"kind": "repeat", "runs": [done("a"), {"id": "b", "state": "stopped"}], "agreement": null});
        assert_eq!(
            study_outcome(&cut),
            StudyOutcome::Failed("only 1 run finished with a report; a check needs two".into())
        );
    }

    #[test]
    fn an_old_compare_error_waits_for_the_new_runs() {
        let mut errored = study(json!([done("a"), done("b1"), {"id": "b2", "state": "running"}]), &["a", "b1"]);
        errored["compare_state"]["error"] = json!("the model timed out");
        assert_eq!(study_outcome(&errored), StudyOutcome::Waiting, "the error is from before b2 joined");
        assert!(study_answer("s1", &errored, &StudyOutcome::Waiting).get("error").is_none());
        errored["runs"][2] = done("b2");
        assert_eq!(study_outcome(&errored), StudyOutcome::Failed("the model timed out".into()));
    }

    #[test]
    fn repeats_are_ready_once_they_all_agree() {
        let mut repeat = json!({"kind": "repeat", "runs": [done("a"), done("b"), done("c")],
                                "agreement": {"runs": 2}, "repeating": true});
        assert_eq!(study_outcome(&repeat), StudyOutcome::Waiting);
        // a series cut off by a restart: two or more reports, and nothing will write the agreement
        repeat["repeating"] = json!(false);
        assert!(matches!(study_outcome(&repeat), StudyOutcome::Failed(e) if e.contains("a check")));
        assert!(study_answer("s1", &repeat, &study_outcome(&repeat))["next"].as_str().unwrap().contains("check_run"));
        repeat["agreement"]["runs"] = json!(3);
        assert_eq!(study_outcome(&repeat), StudyOutcome::Ready);
    }

    #[test]
    fn a_poll_request_never_outlasts_the_wait() {
        // the first read has nothing to fall back on, so gets the whole time-out
        assert_eq!(Poll::until(Duration::ZERO).request_timeout(true), QUICK_TIMEOUT);
        assert_eq!(Poll::until(Duration::ZERO).request_timeout(false), MIN_POLL_REQUEST);
        let poll = Poll::until(Duration::from_secs(10));
        assert!((MIN_POLL_REQUEST..=Duration::from_secs(10)).contains(&poll.request_timeout(false)));
        assert_eq!(Poll::until(Duration::from_secs(600)).request_timeout(false), QUICK_TIMEOUT);
    }

    #[test]
    fn a_cast_waiting_in_the_app_is_reported() {
        let waiting = waiting_for_b("a1", None);
        assert!(waiting.get("pause").is_none());
        let paused = waiting_for_b("a1", Some("personas_ready"));
        assert_eq!(paused["pause"], "personas_ready");
        assert!(paused["next"].as_str().unwrap().contains("Ask the user to confirm it"), "{paused}");
        let detail = json!({"status": {"state": "running", "pause": "personas_ready"}});
        assert_eq!(pause_of(&detail).as_deref(), Some("personas_ready"));
        assert_eq!(pause_of(&json!({"status": {"pause": null}})), None);
    }

    /// An engine that answers its first request with `first` and never answers another, as a busy one might.
    async fn answers_once(first: Value) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let answered = Arc::new(std::sync::atomic::AtomicBool::new(false));
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let (first, answered) = (first.to_string(), answered.clone());
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buf = [0u8; 4096];
                    loop {
                        let n = socket.read(&mut buf).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        request.extend_from_slice(&buf[..n]);
                        if !request.windows(4).any(|w| w == b"\r\n\r\n") {
                            continue;
                        }
                        request.clear();
                        if answered.swap(true, std::sync::atomic::Ordering::SeqCst) {
                            tokio::time::sleep(Duration::from_secs(600)).await;
                        }
                        let reply = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{first}",
                            first.len()
                        );
                        socket.write_all(reply.as_bytes()).await.unwrap();
                    }
                });
            }
        });
        url
    }

    #[tokio::test]
    async fn a_busy_engine_gives_the_last_status_not_an_error() {
        let detail = json!({"id": "r1", "config": {"rounds": 3}, "status": {"state": "running", "stage": "rounds"}});
        let server = Tellfold::new(Engine::new(&answers_once(detail).await, None));
        let p = WaitForRun { run_id: "r1".into(), timeout_seconds: Some(1) };
        let start = Instant::now();
        let answer = serde_json::to_value(server.wait_for_run(Parameters(p)).await.unwrap()).unwrap();
        assert_ne!(answer["isError"], true, "{answer}");
        assert_eq!(answer["structuredContent"]["state"], "running", "{answer}");
        assert!(start.elapsed() < MIN_POLL_REQUEST + Duration::from_secs(3), "{:?}", start.elapsed());
    }

    /// A server whose engine is not reachable: only the pending map can answer.
    fn offline() -> Tellfold {
        Tellfold::new(Engine::new("http://127.0.0.1:9", None))
    }

    async fn study_by_run(server: &Tellfold, run_id: &str, wait_seconds: u64) -> Value {
        let p = GetStudy { study_id: None, run_id: Some(run_id.into()), wait_seconds: Some(wait_seconds) };
        serde_json::to_value(server.get_study(Parameters(p)).await.unwrap()).unwrap()
    }

    #[tokio::test]
    async fn a_waiting_draft_b_is_not_a_study_yet() {
        let server = offline();
        server.pending.lock().unwrap().insert("a1".into(), PendingB::Waiting);
        let answer = study_by_run(&server, "a1", 0).await;
        assert_eq!(answer["structuredContent"]["ready"], false, "{answer}");
        assert!(answer["structuredContent"]["study_id"].is_null());
    }

    #[tokio::test]
    async fn get_study_waits_for_draft_b_to_start() {
        let server = offline();
        server.pending.lock().unwrap().insert("a1".into(), PendingB::Waiting);
        let pending = server.pending.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let started = StartedB { run_id: "b1".into(), study_id: Some("s1".into()) };
            pending.lock().unwrap().insert("a1".into(), PendingB::Started(started));
        });
        let start = Instant::now();
        let answer = study_by_run(&server, "a1", 5).await;
        // it went on to read study s1, which the offline engine cannot serve
        assert_eq!(answer["isError"], true, "{answer}");
        assert!(answer["content"][0]["text"].as_str().unwrap().contains("cannot reach"), "{answer}");
        assert!((Duration::from_millis(250)..Duration::from_secs(4)).contains(&start.elapsed()));
    }

    #[tokio::test]
    async fn a_draft_b_that_failed_says_why() {
        let server = offline();
        server.pending.lock().unwrap().insert("a1".into(), PendingB::Failed("draft A took too long".into()));
        let answer = study_by_run(&server, "a1", 0).await;
        assert_eq!(answer["isError"], true);
        assert_eq!(answer["content"][0]["text"], "draft B could not start: draft A took too long");
    }
}
