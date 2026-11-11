//! `tellfold mcp`: a Model Context Protocol server on stdin/stdout, so coding agents (Claude Code, Claude Desktop,
//! Cursor) can start simulations, wait for them and read the reports.
//!
//! The tools talk to the engine's HTTP API (`tellfold-server`). By default the command starts that API inside its
//! own process, on a free port of 127.0.0.1 with a random token, over the same runs folder the Mac app uses, so runs
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
use serde::Deserialize;
use serde_json::{json, Map, Value};

use tellfold_runtimes::load_settings;
use tellfold_runtimes::process::kill_live_process_groups;
use tellfold_runtimes::settings::tellfold_home;
use tellfold_server::upload::SEED_SUFFIXES;
use tellfold_server::{create_app, log, AppOptions, ManagerOptions};

use crate::args::Parsed;
use crate::commands::{Done, Exit};
use crate::llm::fake_mode;
use crate::paths::py_resolve;
use crate::serve::{bind, exit_later, expanduser, llm_factory, os_error_text, run_server};
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

/// One seed document to upload.
struct Seed {
    name: String,
    bytes: Vec<u8>,
}

impl Engine {
    pub fn new(base: &str, token: Option<&str>) -> Self {
        let http = reqwest::Client::builder().no_proxy().build().expect("an HTTP client");
        Engine { base: base.trim_end_matches('/').to_string(), token: token.map(str::to_string), http }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// The body of a 2xx response, or the API's `detail` (or the status) as the error.
    async fn send(&self, mut request: reqwest::RequestBuilder) -> Result<String, String> {
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response =
            request.send().await.map_err(|e| format!("cannot reach the Tellfold engine at {}: {e}", self.base))?;
        let status = response.status();
        let text = response.text().await.map_err(|e| format!("could not read the engine's answer: {e}"))?;
        if status.is_success() {
            return Ok(text);
        }
        let detail = serde_json::from_str::<Value>(&text).ok().and_then(|v| match v.get("detail") {
            Some(Value::String(s)) => Some(s.clone()),
            Some(other) if !other.is_null() => Some(other.to_string()),
            _ => None,
        });
        Err(match detail {
            Some(detail) => format!("{detail} (HTTP {})", status.as_u16()),
            None => format!("the engine answered HTTP {}", status.as_u16()),
        })
    }

    async fn json(&self, request: reqwest::RequestBuilder) -> Result<Value, String> {
        let text = self.send(request).await?;
        serde_json::from_str(&text).map_err(|e| format!("the engine sent invalid JSON: {e}"))
    }

    pub async fn get(&self, path: &str) -> Result<Value, String> {
        self.json(self.http.get(self.url(path))).await
    }

    pub async fn get_text(&self, path: &str) -> Result<String, String> {
        self.send(self.http.get(self.url(path))).await
    }

    pub async fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        self.json(self.http.post(self.url(path)).json(body)).await
    }

    /// `POST /api/runs` with the form fields and the seed files.
    async fn create_run(&self, fields: &[(String, String)], seeds: Vec<Seed>) -> Result<String, String> {
        let mut form = reqwest::multipart::Form::new();
        for (key, value) in fields {
            form = form.text(key.clone(), value.clone());
        }
        for seed in seeds {
            form = form.part("files", reqwest::multipart::Part::bytes(seed.bytes).file_name(seed.name));
        }
        let created = self.json(self.http.post(self.url("/api/runs")).multipart(form)).await?;
        created["id"].as_str().map(str::to_string).ok_or_else(|| "the engine did not return a run id".into())
    }
}

fn run_path(run_id: &str, rest: &str) -> String {
    let id = percent_encoding::utf8_percent_encode(run_id, percent_encoding::NON_ALPHANUMERIC);
    format!("/api/runs/{id}{rest}")
}

fn study_path(study_id: &str) -> String {
    format!("/api/studies/{}", percent_encoding::utf8_percent_encode(study_id, percent_encoding::NON_ALPHANUMERIC))
}

// ---------- seeds ----------

fn is_seed(name: &str) -> bool {
    let lower = name.to_lowercase();
    SEED_SUFFIXES.iter().any(|s| lower.ends_with(s))
}

/// The seed files under `dir`, hidden ones skipped, in name order.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            walk(&path, out)?;
        } else if is_seed(&name) {
            out.push(path);
        }
    }
    Ok(())
}

/// The documents of a run: `text` (saved as `title`) and every file or folder in `files`.
fn collect_seeds(text: Option<&str>, title: Option<&str>, files: &[String]) -> Result<Vec<Seed>, String> {
    let mut seeds = Vec::new();
    if let Some(text) = text.filter(|t| !t.trim().is_empty()) {
        let title = title.map(str::trim).filter(|t| !t.is_empty()).unwrap_or("announcement");
        let name = if is_seed(title) { title.to_string() } else { format!("{title}.md") };
        seeds.push(Seed { name, bytes: text.as_bytes().to_vec() });
    }
    for given in files {
        let path = expanduser(Path::new(given));
        let mut found = Vec::new();
        if path.is_dir() {
            walk(&path, &mut found).map_err(|e| format!("{given}: {}", os_error_text(&e)))?;
            if found.is_empty() {
                return Err(format!("{given} holds no .md, .markdown, .txt or .pdf files"));
            }
        } else if path.is_file() {
            if !is_seed(given) {
                return Err(format!("{given}: only .pdf, .md, .markdown and .txt are supported"));
            }
            found.push(path);
        } else {
            return Err(format!("{given}: no such file or folder (relative paths start at {})", cwd_text()));
        }
        for file in found {
            let bytes = std::fs::read(&file).map_err(|e| format!("{}: {}", file.display(), os_error_text(&e)))?;
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

/// The parts of `GET /api/runs/{id}` a caller needs to follow a run.
fn run_status(detail: &Value) -> Value {
    let status = &detail["status"];
    let config = &detail["config"];
    let state = status["state"].as_str().unwrap_or("queued");
    let stage = status["stage"].as_str().unwrap_or("reading");
    let round = status["round"].as_i64().unwrap_or(0);
    let total = status["total_rounds"].as_i64().or_else(|| config["rounds"].as_i64()).unwrap_or(0);
    let finished = TERMINAL_STATES.contains(&state);
    let has_report = !detail["report"].is_null();
    let next = match (state, has_report) {
        ("done", _) | (_, true) => "Call get_report for the report.",
        ("failed", _) => "The run failed; see error.",
        ("stopped", _) => "The run was stopped.",
        _ => "Still running: call wait_for_run again.",
    };
    let mut out = json!({
        "run_id": detail["id"],
        "question": config["question"],
        "state": state,
        "stage": stage,
        "round": round,
        "total_rounds": total,
        "progress": progress(state, stage, round, total),
        "detail": status["detail"],
        "finished": finished,
        "has_report": has_report,
        "people": detail["personas"].as_array().map_or(0, Vec::len),
        "posts_written_by": config["language"],
        "next": next,
    });
    out.as_object_mut().expect("an object").remove("posts_written_by");
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
    if detail["example"].as_bool() == Some(true) {
        out["example"] = json!(true);
    }
    out
}

fn clamp_wait(seconds: Option<u64>) -> Duration {
    Duration::from_secs(seconds.unwrap_or(DEFAULT_WAIT_SECONDS).min(MAX_WAIT_SECONDS))
}

// ---------- tool descriptions ----------

const START_SIMULATION: &str = "Starts a simulation of how a crowd of AI personas reacts to a text \
(announcement, changelog, pricing change, launch post) and returns its run id at once. A run takes minutes: follow it \
with wait_for_run, then read get_report.";
const GET_RUN_STATUS: &str = "The state, stage, round and rough progress (0-1) of a run, without waiting.";
const WAIT_FOR_RUN: &str = "Wait until a run finishes, or until timeout_seconds pass (default 30, at most 120), and \
return its status. Call it again while finished is false.";
const GET_REPORT: &str = "The report of a finished run: the prediction, confidence, camps, reasons, risks and \
recommendations, each citing the posts behind it. format: \"markdown\" (default), \"json\" or \"both\".";
const LIST_RUNS: &str = "The runs in the runs folder, newest first, with their state and question.";
const LIST_PEOPLE: &str = "The simulated people of a run: id, handle, name, bio and starting stance.";
const ASK_CROWD: &str = "Ask a question about how the crowd reacted in a finished run. The report agent answers from \
the posts and cites them.";
const INTERVIEW_PERSON: &str = "Interview one simulated person of a run; they answer in character.";
const INJECT_EVENT: &str = "Inject breaking news into a running simulation at a round that has not started, e.g. a \
competitor's response or a correction.";
const STOP_RUN: &str = "Stop a running simulation. What it has so far is kept.";
const COMPARE_DRAFTS: &str = "Compare two versions of a text on the same crowd: draft A runs first, then \
draft B is shown to the same people, and a cited comparison is written once both finish. Returns the run ids at \
once; follow with get_study (by study_id, or by the draft A run_id while draft B is still being set up).";
const GET_STUDY: &str = "A study: the runs of compare_drafts with their cited comparison, or the repeats of check_run \
with what held across them. wait_seconds waits for the comparison to be written.";
const CHECK_RUN: &str = "Check a finished run: run it again with the same people and text but other random seeds \
(runs in all, 2-5), then say which findings held. Returns a study id; follow with get_study.";

// ---------- tool parameters ----------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EventIn {
    /// The round it happens at (1 to the number of rounds).
    pub round: i64,
    /// What happens, e.g. "A competitor cuts its price in half."
    pub content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StartSimulation {
    /// What you want predicted, e.g. "How will existing users react to this pricing change?"
    pub question: String,
    /// The text to test (an announcement, changelog, launch post, email) in Markdown or plain text. Give this,
    /// `files`, or both.
    #[serde(default)]
    pub text: Option<String>,
    /// File name for `text` (default "announcement.md").
    #[serde(default)]
    pub title: Option<String>,
    /// Seed documents (.md, .markdown, .txt, .pdf) or folders of them. Relative paths start at the folder the MCP
    /// server runs in.
    #[serde(default)]
    pub files: Vec<String>,
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
    /// "markdown" (default), "json", or "both".
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
    /// The person's id ("a03") or handle ("@maria"); see list_people.
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
    /// An existing run to use as draft A instead of starting one: draft B is shown to its people.
    #[serde(default)]
    pub baseline_run_id: Option<String>,
    /// Label of draft B in the comparison (default: the next letter).
    #[serde(default)]
    pub label_b: Option<String>,
    /// Number of simulated people for draft A, 2-200 (draft B reuses the same people).
    #[serde(default)]
    pub agents: Option<i64>,
    /// Simulation rounds, 1-50.
    #[serde(default)]
    pub rounds: Option<i64>,
    /// Language the people post in and the reports are written in.
    #[serde(default)]
    pub language: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetStudy {
    /// The study id compare_drafts or check_run returned. Give this or `run_id`.
    #[serde(default)]
    pub study_id: Option<String>,
    /// A run of the study (for compare_drafts, the draft A run id).
    #[serde(default)]
    pub run_id: Option<String>,
    /// Wait up to this many seconds for the comparison to be written (default 0, at most 120).
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

/// What `compare_drafts` knows about a draft B it could not start at once.
#[derive(Clone, Debug)]
enum PendingB {
    Waiting,
    Started { run_id: String, study_id: Option<String> },
    Failed(String),
}

#[derive(Clone)]
pub struct Tellfold {
    engine: Engine,
    /// draft A run id -> draft B
    pending: Arc<Mutex<HashMap<String, PendingB>>>,
    tool_router: ToolRouter<Self>,
}

/// The form fields of a new run.
fn run_fields(
    question: &str,
    agents: Option<i64>,
    rounds: Option<i64>,
    language: Option<&str>,
) -> Vec<(String, String)> {
    let mut fields = vec![("question".to_string(), question.to_string())];
    if let Some(n) = agents {
        fields.push(("agents".into(), n.to_string()));
    }
    if let Some(n) = rounds {
        fields.push(("rounds".into(), n.to_string()));
    }
    if let Some(language) = language.filter(|l| !l.trim().is_empty()) {
        fields.push(("language".into(), language.to_string()));
    }
    fields
}

impl Tellfold {
    pub fn new(engine: Engine) -> Self {
        Tellfold { engine, pending: Arc::default(), tool_router: Self::tool_router() }
    }

    async fn detail(&self, run_id: &str) -> Result<Value, String> {
        self.engine.get(&run_path(run_id, "")).await
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

    /// Starts draft B on draft A's people: `cast_from` A, with A's question.
    async fn start_b(
        &self,
        run_a: &str,
        draft_b: &str,
        label: Option<&str>,
    ) -> Result<(String, Option<String>), String> {
        let detail = self.detail(run_a).await?;
        let config = &detail["config"];
        let question = config["question"].as_str().unwrap_or_default();
        let mut fields = run_fields(question, None, config["rounds"].as_i64(), config["language"].as_str());
        // the same AI tools and models as draft A, so only the text differs
        for key in ["agent_runtime", "planner_runtime", "agent_model", "planner_model"] {
            if let Some(value) = config[key].as_str().filter(|v| !v.is_empty()) {
                fields.push((key.into(), value.into()));
            }
        }
        fields.push(("cast_from".into(), run_a.to_string()));
        if let Some(label) = label.filter(|l| !l.trim().is_empty()) {
            fields.push(("variant".into(), label.to_string()));
        }
        let seeds = vec![Seed { name: "draft-b.md".into(), bytes: draft_b.as_bytes().to_vec() }];
        let run_b = self.engine.create_run(&fields, seeds).await?;
        let study_id = self.detail(&run_b).await?["config"]["study_id"].as_str().map(str::to_string);
        Ok((run_b, study_id))
    }

    /// Waits until run A has its people (`Ok(true)`), ends without them (an error), or `limit` passes (`Ok(false)`).
    async fn wait_for_people(&self, run_a: &str, limit: Duration) -> Result<bool, String> {
        let deadline = Instant::now() + limit;
        loop {
            let detail = self.detail(run_a).await?;
            if detail["personas"].as_array().is_some_and(|p| !p.is_empty()) && !detail["graph"].is_null() {
                return Ok(true);
            }
            let state = detail["status"]["state"].as_str().unwrap_or("");
            if TERMINAL_STATES.contains(&state) {
                let error = detail["status"]["error"].as_str().unwrap_or("").to_string();
                return Err(format!("draft A ended ({state}) before it had people. {error}").trim().to_string());
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
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
        let mut fields = run_fields(&p.question, p.agents, p.rounds, p.language.as_deref());
        for (key, value) in [
            ("agent_runtime", &p.agent_runtime),
            ("planner_runtime", &p.planner_runtime),
            ("agent_model", &p.agent_model),
            ("planner_model", &p.planner_model),
        ] {
            if let Some(value) = value.as_deref().filter(|v| !v.trim().is_empty()) {
                fields.push((key.into(), value.into()));
            }
        }
        if !p.events.is_empty() {
            let events: Vec<Value> = p.events.iter().map(|e| json!({"round": e.round, "content": e.content})).collect();
            fields.push(("events".into(), Value::Array(events).to_string()));
        }
        let documents: Vec<String> = seeds.iter().map(|s| s.name.clone()).collect();
        match self.engine.create_run(&fields, seeds).await {
            Ok(run_id) => ok(json!({
                "run_id": run_id,
                "documents": documents,
                "next": "Call wait_for_run with this run_id until finished is true, then get_report.",
            })),
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
        let deadline = Instant::now() + clamp_wait(p.timeout_seconds);
        let mut pause = Duration::from_millis(250);
        loop {
            let status = match self.detail(&p.run_id).await {
                Ok(detail) => run_status(&detail),
                Err(e) => return fail(e),
            };
            if status["finished"].as_bool() == Some(true) || Instant::now() >= deadline {
                return ok(status);
            }
            tokio::time::sleep(pause.min(deadline.saturating_duration_since(Instant::now()))).await;
            pause = (pause * 2).min(Duration::from_secs(2));
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
                if run["example"].as_bool() == Some(true) {
                    out.insert("example".into(), json!(true));
                }
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
                        json!({"id": p["id"], "handle": p["handle"], "name": p["name"], "bio": p["bio"],
                               "stance": p["stance"]})
                    })
                    .collect()
            })
            .unwrap_or_default();
        ok(json!({"run_id": p.run_id, "people": people}))
    }

    #[tool(description = ASK_CROWD)]
    async fn ask_crowd(&self, Parameters(p): Parameters<AskCrowd>) -> Result<CallToolResult, ErrorData> {
        match self.engine.post(&run_path(&p.run_id, "/ask"), &json!({"question": p.question})).await {
            Ok(reply) => ok(json!({"run_id": p.run_id, "answer": reply["reply"]})),
            Err(e) => fail(e),
        }
    }

    #[tool(description = INTERVIEW_PERSON)]
    async fn interview_person(&self, Parameters(p): Parameters<Interview>) -> Result<CallToolResult, ErrorData> {
        let body = json!({"agent_id": p.person, "message": p.message, "history": []});
        match self.engine.post(&run_path(&p.run_id, "/chat"), &body).await {
            Ok(reply) => ok(json!({"run_id": p.run_id, "person": p.person, "reply": reply["reply"]})),
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
                let fields = run_fields(question, p.agents, p.rounds, p.language.as_deref());
                let seeds = vec![Seed { name: "draft-a.md".into(), bytes: draft_a.as_bytes().to_vec() }];
                match self.engine.create_run(&fields, seeds).await {
                    Ok(run_id) => run_id,
                    Err(e) => return fail(e),
                }
            }
        };
        match self.wait_for_people(&run_a, COMPARE_INLINE_WAIT).await {
            Err(e) => return fail(e),
            Ok(true) => {
                return match self.start_b(&run_a, &p.draft_b, p.label_b.as_deref()).await {
                    Ok((run_b, study_id)) => ok(json!({
                        "run_a": run_a, "run_b": run_b, "study_id": study_id,
                        "next": "Call wait_for_run on both runs, then get_study with wait_seconds for the comparison.",
                    })),
                    Err(e) => fail(e),
                };
            }
            Ok(false) => {}
        }
        // Draft A is still working out its people: start draft B in the background once it has them.
        self.pending.lock().expect("not poisoned").insert(run_a.clone(), PendingB::Waiting);
        let me = self.clone();
        let (a, draft_b, label) = (run_a.clone(), p.draft_b.clone(), p.label_b.clone());
        tokio::spawn(async move {
            let outcome = match me.wait_for_people(&a, COMPARE_BACKGROUND_WAIT).await {
                Ok(true) => me.start_b(&a, &draft_b, label.as_deref()).await,
                Ok(false) => Err("draft A took too long to make its people".into()),
                Err(e) => Err(e),
            };
            let state = match outcome {
                Ok((run_id, study_id)) => PendingB::Started { run_id, study_id },
                Err(e) => PendingB::Failed(e),
            };
            me.pending.lock().expect("not poisoned").insert(a, state);
        });
        ok(json!({
            "run_a": run_a, "run_b": null, "study_id": null,
            "next": "Draft B starts as soon as draft A has its people. Call get_study with run_id set to run_a.",
        }))
    }

    #[tool(description = GET_STUDY)]
    async fn get_study(&self, Parameters(p): Parameters<GetStudy>) -> Result<CallToolResult, ErrorData> {
        let deadline = Instant::now() + Duration::from_secs(p.wait_seconds.unwrap_or(0).min(MAX_WAIT_SECONDS));
        let study_id = match (p.study_id.as_deref().filter(|s| !s.is_empty()), p.run_id.as_deref()) {
            (Some(study_id), _) => study_id.to_string(),
            (None, Some(run_id)) => {
                let pending = self.pending.lock().expect("not poisoned").get(run_id).cloned();
                match pending {
                    Some(PendingB::Waiting) => {
                        return ok(json!({
                            "study_id": null, "ready": false, "run_a": run_id, "run_b": null,
                            "next": "Draft B starts once draft A has its people; ask again shortly.",
                        }))
                    }
                    Some(PendingB::Failed(e)) => return fail(format!("draft B could not start: {e}")),
                    Some(PendingB::Started { study_id: Some(study_id), .. }) => study_id,
                    started => {
                        let member = match &started {
                            Some(PendingB::Started { run_id: run_b, .. }) => run_b.as_str(),
                            _ => run_id,
                        };
                        match self.study_of(member).await {
                            Ok(Some(study_id)) => study_id,
                            Ok(None) => return fail(format!("run {run_id} belongs to no study")),
                            Err(e) => return fail(e),
                        }
                    }
                }
            }
            (None, None) => return fail("give study_id or run_id"),
        };
        let mut pause = Duration::from_millis(250);
        loop {
            let study = match self.engine.get(&study_path(&study_id)).await {
                Ok(study) => study,
                Err(e) => return fail(e),
            };
            let repeat = study["kind"] == "repeat";
            let result = if repeat { &study["agreement"] } else { &study["compare"] };
            let error = study["compare_state"]["error"].as_str().filter(|e| !e.is_empty());
            let ready = !result.is_null();
            if ready || error.is_some() || Instant::now() >= deadline {
                let runs: Vec<Value> = study["runs"]
                    .as_array()
                    .map(|runs| {
                        runs.iter()
                            .map(|r| {
                                json!({"run_id": r["id"], "variant": r["variant"], "letter": r["letter"],
                                       "state": r["state"], "has_report": r["has_report"]})
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let mut out = json!({"study_id": study_id, "kind": study["kind"], "ready": ready, "runs": runs});
                if repeat {
                    out["agreement"] = study["agreement"].clone();
                } else {
                    out["comparison"] = study["compare"]["report"].clone();
                }
                if let Some(error) = error {
                    out["error"] = json!(error);
                }
                if !ready {
                    out["next"] = json!(if study["compare_state"]["running"].as_bool() == Some(true)
                        || study["repeating"].as_bool() == Some(true)
                    {
                        "Being written: call get_study again with wait_seconds."
                    } else {
                        "Written once every run has finished: wait for the runs, then call get_study again."
                    });
                }
                return ok(out);
            }
            tokio::time::sleep(pause.min(deadline.saturating_duration_since(Instant::now()))).await;
            pause = (pause * 2).min(Duration::from_secs(2));
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
    let _ = service.waiting().await;
    Ok(())
}

/// Starts the HTTP API in this process, then serves the MCP tools against it; the API stops (and marks its live
/// runs stopped) when the MCP client goes away.
async fn embedded(p: &Parsed, token: Option<String>, err: &Console) -> Result<(), String> {
    let runs_dir = runs_folder(p);
    std::fs::create_dir_all(&runs_dir)
        .map_err(|e| format!("cannot create {}: {}", runs_dir.display(), os_error_text(&e)))?;
    let examples_dir = p.str("examples").map(|e| py_resolve(&expanduser(Path::new(&e)))).filter(|d| d.is_dir());
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
    fn text_seeds_get_a_markdown_name() {
        let seeds = collect_seeds(Some("# v2"), Some("changelog"), &[]).unwrap();
        assert_eq!(seeds[0].name, "changelog.md");
        let seeds = collect_seeds(Some("hi"), Some("notes.txt"), &[]).unwrap();
        assert_eq!(seeds[0].name, "notes.txt");
        assert!(collect_seeds(Some("  "), None, &[]).is_err());
        assert!(collect_seeds(None, None, &["/no/such/file.md".into()]).is_err());
    }

    #[test]
    fn folders_give_their_seed_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("b.md"), "b").unwrap();
        std::fs::write(tmp.path().join("a.txt"), "a").unwrap();
        std::fs::write(tmp.path().join(".hidden.md"), "h").unwrap();
        std::fs::write(tmp.path().join("image.png"), "p").unwrap();
        let seeds = collect_seeds(None, None, &[tmp.path().to_string_lossy().into_owned()]).unwrap();
        let names: Vec<&str> = seeds.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["a.txt", "b.md"]);
        let png = tmp.path().join("image.png").to_string_lossy().into_owned();
        assert!(collect_seeds(None, None, &[png]).is_err());
    }
}
