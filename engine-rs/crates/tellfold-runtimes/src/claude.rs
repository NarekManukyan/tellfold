//! Claude Code on the user's logged-in subscription (port of `ClaudeLocal` in `tellfold/llm.py` and
//! `IsolatedClaudeLocal` in `tellfold/runtimes/registry.py`).
//!
//! Python drives the CLI through claude-agent-sdk. This drives the same CLI the same way the SDK does: stream-json
//! in and out, one user message marked `client_composed` (Claude Code then delivers it verbatim: no `@path`
//! expansion, no slash commands), tools off, no MCP servers, no setting sources, thinking off except for the think
//! purposes, every engine secret blanked in the child's environment, and an empty scratch folder as the working
//! directory.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rand::Rng;
use serde_json::{json, Value};
use tellfold_llm::adapter::{log_error, log_timestamp, repair_prompt, CallLog};
use tellfold_llm::{extract_json, Llm, LlmError, Schema};
use tellfold_models::json::{py_dumps, py_dumps_ascii, py_round};
use tellfold_models::text::{py_prefix, py_strip};
use tokio::sync::Semaphore;

use crate::detect::is_secret_env;
use crate::process::StreamingChild;

/// Purposes that keep extended thinking on. Thinking makes haiku emit 10-20k hidden tokens per call (measured:
/// graph extraction 126s with thinking vs 9.5s without), so it is off everywhere else.
pub const DEFAULT_THINK_PURPOSES: &[&str] = &["report.write"];
pub const PLANNER_THINK_PURPOSES: &[&str] = &["graph.", "personas.", "report."];
/// Extra turns let the CLI re-prompt the model when its structured output fails schema validation.
pub const STRUCTURED_MAX_TURNS: u32 = 3;

pub const AUTH_HINT: &str = "Claude Code is not logged in: run `claude` once and use /login, then retry.";
const RATE_HINTS: &[&str] = &["rate limit", "rate_limit", "usage limit", "overloaded", "too many requests"];
const AUTH_HINTS: &[&str] = &["failed to authenticate", "oauth", "not logged in", "invalid api key", "/login"];
const RETRYABLE_STATUS: &[i64] = &[408, 409, 429, 500, 502, 503, 504, 529];
const AUTH_STATUS: &[i64] = &[401, 403];

/// The SDK hands the CLI our whole environment and only lets options override it, so every engine secret is
/// blanked: the bearer token, the Keychain keys (TELLFOLD_KEY_*) and ANTHROPIC_API_KEY / OPENAI_API_KEY. With
/// ANTHROPIC_API_KEY set, Claude Code in -p mode would bill the API key instead of using the subscription login.
pub fn blanked_secret_env() -> BTreeMap<String, String> {
    let mut names: Vec<String> = std::env::vars().map(|(k, _)| k).filter(|k| is_secret_env(k)).collect();
    names.extend(["ANTHROPIC_API_KEY", "TELLFOLD_SERVE_TOKEN", "SHOAL_SERVE_TOKEN"].map(String::from));
    names.into_iter().map(|name| (name, String::new())).collect()
}

/// `tempfile.gettempdir() / "tellfold-runtime-cwd"`: the empty folder every Claude Code call starts in.
pub fn isolated_cwd() -> PathBuf {
    let path = std::env::temp_dir().join("tellfold-runtime-cwd");
    let _ = std::fs::create_dir_all(&path);
    path
}

/// The JSON-mode system prompt. `structured`: the CLI enforces the schema, so the prompt only states the rules.
pub fn json_system(system: &str, json_schema: &Value, structured: bool) -> String {
    let contract = if structured {
        "Deliver your final answer through the structured output tool. Every required field must be present, types \
         must match the schema exactly, and no field may be invented."
            .to_string()
    } else {
        format!(
            "Your reply is parsed by a program. Respond with exactly one JSON value that validates against the JSON \
             Schema below: no prose before or after it, no markdown code fences, no comments, no trailing commas. \
             Every required field must be present and every type must match.\n\nJSON Schema:\n{}",
            py_dumps(json_schema)
        )
    };
    format!("{}\n\n# Output format\n{contract}", system.trim_end_matches(tellfold_models::text::py_isspace))
}

/// What one CLI call is started with (`ClaudeAgentOptions`).
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeOptions {
    pub system_prompt: String,
    pub model: String,
    pub max_turns: u32,
    /// The structured-output schema (`output_format`), or None for a plain-text reply.
    pub json_schema: Option<Value>,
    /// `thinking={"type": "disabled"}`; false leaves the CLI's default.
    pub thinking_off: bool,
    pub cli_path: Option<String>,
    pub cwd: Option<PathBuf>,
    /// Variables set over the inherited environment.
    pub env: BTreeMap<String, String>,
}

impl ClaudeOptions {
    /// The command line, in the order claude-agent-sdk builds it.
    pub fn argv(&self, cli: &str) -> Vec<String> {
        let mut argv: Vec<String> =
            [cli, "--output-format", "stream-json", "--verbose", "--system-prompt"].map(String::from).to_vec();
        argv.push(self.system_prompt.clone());
        argv.extend(["--tools".into(), String::new(), "--max-turns".into(), self.max_turns.to_string()]);
        if !self.model.is_empty() {
            argv.extend(["--model".into(), self.model.clone()]);
        }
        argv.push("--strict-mcp-config".into()); // never start the user's own MCP servers for simulation calls
        argv.push("--setting-sources=".into());
        if self.thinking_off {
            argv.extend(["--thinking".into(), "disabled".into()]);
        }
        if let Some(schema) = &self.json_schema {
            argv.extend(["--json-schema".into(), py_dumps_ascii(schema)]);
        }
        argv.extend(["--input-format", "stream-json", "--no-session-persistence", "-p"].map(String::from));
        argv
    }
}

/// The user message written to the CLI's stdin. `client_composed`: seed text may contain "@path" or "/command";
/// Claude Code delivers it as written.
pub fn user_message(prompt: &str) -> String {
    let message = json!({
        "type": "user",
        "session_id": "",
        "message": {"role": "user", "content": prompt},
        "parent_tool_use_id": null,
        "client_composed": true,
    });
    format!("{}\n", serde_json::to_string(&message).expect("serializable"))
}

/// Why a query produced no result message.
#[derive(Debug, Clone, PartialEq)]
pub enum QueryError {
    /// The CLI is not installed (`CLINotFoundError`): not retried.
    NotFound(String),
    /// The CLI failed or the stream broke (`ProcessError` and the SDK's other errors): retried.
    Failed { kind: String, message: String },
    /// An error result raised by the SDK (`ResultError`).
    Result { text: String, api_error_status: Option<i64>, terminal_reason: Option<String> },
}

/// The messages one query streamed, up to and including the result.
#[derive(Debug, Clone, Default)]
pub struct QueryOutput {
    pub messages: Vec<Value>,
    /// When the first message arrived (start-up time).
    pub first_message: Option<Instant>,
}

/// How a query reaches Claude Code (`claude_agent_sdk.query`); tests script it.
#[async_trait]
pub trait ClaudeTransport: Send + Sync {
    async fn query(&self, prompt: &str, options: &ClaudeOptions) -> Result<QueryOutput, QueryError>;
}

/// Runs the `claude` CLI.
pub struct CliTransport;

#[async_trait]
impl ClaudeTransport for CliTransport {
    async fn query(&self, prompt: &str, options: &ClaudeOptions) -> Result<QueryOutput, QueryError> {
        let cli = options
            .cli_path
            .clone()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| QueryError::NotFound("Claude Code not found".into()))?;
        let mut env: BTreeMap<String, String> = std::env::vars().collect();
        env.extend(options.env.clone());
        let cwd = options.cwd.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| "/".into()));
        let mut child = match StreamingChild::spawn(&options.argv(&cli), &user_message(prompt), &cwd, &env).await {
            Ok(child) => child,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(QueryError::NotFound(format!("Claude Code not found at: {cli}")));
            }
            Err(e) => return Err(QueryError::Failed { kind: "CLIConnectionError".into(), message: e.to_string() }),
        };
        let mut output = QueryOutput::default();
        let mut finished = false;
        loop {
            match child.stdout.next_line().await {
                Ok(Some(line)) => {
                    let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
                    output.first_message.get_or_insert_with(Instant::now);
                    let is_result = message.get("type").and_then(Value::as_str) == Some("result");
                    output.messages.push(message);
                    if is_result {
                        finished = true;
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => return Err(QueryError::Failed { kind: "CLIJSONDecodeError".into(), message: e.to_string() }),
            }
        }
        // stdin is closed, so the CLI exits once it has answered
        match tokio::time::timeout(Duration::from_secs(10), child.finish()).await {
            Ok(Ok((code, stderr))) if code != 0 && !finished => Err(QueryError::Failed {
                kind: "ProcessError".into(),
                message: format!("Command failed with exit code {code} (exit code: {code})\nError output: {stderr}"),
            }),
            _ => Ok(output),
        }
    }
}

/// A failure inside one attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    Transient { message: String, rate_limited: bool },
    BadOutput { message: String, reply_text: String },
    Llm(LlmError),
}

#[derive(Default)]
struct Reply {
    text: String,
    structured: Option<Value>,
    turns: i64,
    api_seconds: f64,
    startup_seconds: f64,
    output_tokens: i64,
}

struct CallRecord {
    purpose: String,
    model: String,
    prompt_chars: usize,
    mode: &'static str,
    started: Instant,
    attempts: u32,
    output_chars: usize,
    turns: i64,
    api_seconds: f64,
    startup_seconds: f64,
    output_tokens: i64,
}

/// Sorts an error result: not logged in (final), worth retrying, or unusable output (repairable).
pub fn classify_api_failure(message: &str, status: Option<i64>, terminal_reason: Option<&str>) -> Failure {
    let lowered = message.to_lowercase();
    let stripped = py_strip(message);
    if status.is_some_and(|s| AUTH_STATUS.contains(&s))
        || (status.is_none() && AUTH_HINTS.iter().any(|h| lowered.contains(h)))
    {
        return Failure::Llm(LlmError(format!("{AUTH_HINT} ({})", py_prefix(stripped, 200))));
    }
    let rate_limited = status == Some(429) || RATE_HINTS.iter().any(|h| lowered.contains(h));
    if rate_limited || status.is_some_and(|s| RETRYABLE_STATUS.contains(&s)) || terminal_reason == Some("api_error") {
        return Failure::Transient { message: py_prefix(stripped, 300).into(), rate_limited };
    }
    let text = py_prefix(stripped, 300);
    Failure::BadOutput {
        message: if text.is_empty() { "the CLI reported an error result".into() } else { text.into() },
        reply_text: String::new(),
    }
}

fn query_failure(error: QueryError) -> Failure {
    match error {
        QueryError::NotFound(e) => Failure::Llm(LlmError(format!(
            "Claude Code CLI not found ({e}). Install it and make sure `claude` is on PATH."
        ))),
        QueryError::Failed { kind, message } => {
            Failure::Transient { message: format!("{kind}: {}", py_prefix(&message, 300)), rate_limited: false }
        }
        QueryError::Result { text, api_error_status, terminal_reason } => {
            classify_api_failure(&text, api_error_status, terminal_reason.as_deref())
        }
    }
}

/// `_from_result`: the result message's text (or the assistant blocks), or the classified error.
fn from_result(result: &Value, texts: &[String]) -> Result<Reply, Failure> {
    let text = match result.get("result").and_then(Value::as_str) {
        Some(s) if !py_strip(s).is_empty() => s.to_string(),
        _ => texts.join("\n"),
    };
    if result.get("is_error").and_then(Value::as_bool) == Some(true) {
        let mut parts = vec![text];
        if let Some(errors) = result.get("errors").and_then(Value::as_array) {
            parts.extend(errors.iter().map(|e| e.as_str().map_or_else(|| e.to_string(), str::to_string)));
        }
        let joined = parts.join(" ");
        let mut detail = py_strip(&joined).to_string();
        if detail.is_empty() {
            detail = result.get("subtype").and_then(Value::as_str).unwrap_or_default().to_string();
        }
        return Err(classify_api_failure(
            &detail,
            result.get("api_error_status").and_then(Value::as_i64),
            result.get("terminal_reason").and_then(Value::as_str),
        ));
    }
    let structured = result.get("structured_output").filter(|v| !v.is_null()).cloned();
    Ok(Reply { text, structured, ..Reply::default() })
}

/// `LLM` backed by the local Claude Code CLI on the user's logged-in subscription (no API key).
pub struct ClaudeCode {
    semaphore: Semaphore,
    log: CallLog,
    pub cli_path: Option<String>,
    pub timeout: Duration,
    pub retries: u32,
    pub think_purposes: Vec<String>,
    /// The working directory of every call; None inherits the engine's (Python's plain `ClaudeLocal`).
    pub cwd: Option<PathBuf>,
    /// Whether json() asks the CLI for structured output (always true with a current CLI).
    pub structured_output: bool,
    pub backoff_base_s: f64,
    pub rate_limit_backoff_s: f64,
    #[allow(clippy::type_complexity)]
    pub backoff_hook: Option<Box<dyn Fn(u32, bool) -> f64 + Send + Sync>>,
    transport: Arc<dyn ClaudeTransport>,
}

/// Logs the call if it is dropped before finishing, as Python logs a cancelled call.
struct Pending<'a> {
    llm: &'a ClaudeCode,
    record: Mutex<Option<CallRecord>>,
}

impl Pending<'_> {
    fn with<T>(&self, f: impl FnOnce(&mut CallRecord) -> T) -> T {
        f(self.record.lock().expect("not poisoned").as_mut().expect("record present until finished"))
    }

    fn finish(&self, error: Option<String>) {
        if let Some(record) = self.record.lock().expect("not poisoned").take() {
            self.llm.write_log(&record, error);
        }
    }
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        if let Some(record) = self.record.get_mut().ok().and_then(Option::take) {
            self.llm.write_log(&record, Some("CancelledError: ".into()));
        }
    }
}

impl ClaudeCode {
    /// `ClaudeLocal(concurrency=..., log_path=..., cli_path=...)` with the CLI transport.
    pub fn new(concurrency: usize, log_path: Option<PathBuf>, cli_path: Option<String>) -> ClaudeCode {
        ClaudeCode {
            semaphore: Semaphore::new(concurrency.max(1)),
            log: CallLog::new(log_path),
            cli_path,
            timeout: Duration::from_secs(240),
            retries: 2,
            think_purposes: DEFAULT_THINK_PURPOSES.iter().map(|p| p.to_string()).collect(),
            cwd: None,
            structured_output: true,
            backoff_base_s: 2.0,
            rate_limit_backoff_s: 20.0,
            backoff_hook: None,
            transport: Arc::new(CliTransport),
        }
    }

    /// `IsolatedClaudeLocal`: every call starts in an empty scratch folder instead of the caller's directory.
    pub fn isolated(concurrency: usize, log_path: Option<PathBuf>, cli_path: Option<String>) -> ClaudeCode {
        ClaudeCode { cwd: Some(isolated_cwd()), ..ClaudeCode::new(concurrency, log_path, cli_path) }
    }

    pub fn with_transport(mut self, transport: Arc<dyn ClaudeTransport>) -> ClaudeCode {
        self.transport = transport;
        self
    }

    pub fn with_think_purposes(mut self, purposes: &[&str]) -> ClaudeCode {
        self.think_purposes = purposes.iter().map(|p| p.to_string()).collect();
        self
    }

    /// The options one call is started with.
    pub fn options(&self, system: &str, model: &str, json_schema: Option<&Value>, purpose: &str) -> ClaudeOptions {
        ClaudeOptions {
            system_prompt: system.into(),
            model: model.into(),
            max_turns: if json_schema.is_some() { STRUCTURED_MAX_TURNS } else { 1 },
            json_schema: json_schema.cloned(),
            thinking_off: !self.think_purposes.iter().any(|p| purpose.starts_with(p.as_str())),
            cli_path: self.cli_path.clone(),
            cwd: self.cwd.clone(),
            env: blanked_secret_env(),
        }
    }

    fn pending(&self, purpose: &str, model: &str, prompt_chars: usize, mode: &'static str) -> Pending<'_> {
        Pending {
            llm: self,
            record: Mutex::new(Some(CallRecord {
                purpose: purpose.into(),
                model: model.into(),
                prompt_chars,
                mode,
                started: Instant::now(),
                attempts: 0,
                output_chars: 0,
                turns: 0,
                api_seconds: 0.0,
                startup_seconds: 0.0,
                output_tokens: 0,
            })),
        }
    }

    fn write_log(&self, record: &CallRecord, error: Option<String>) {
        self.log.write(&json!({
            "ts": log_timestamp(),
            "purpose": record.purpose,
            "model": record.model,
            "mode": record.mode,
            "seconds": py_round(record.started.elapsed().as_secs_f64(), 3),
            "attempts": record.attempts,
            "ok": error.is_none(),
            "error": error,
            "prompt_chars": record.prompt_chars,
            "output_chars": record.output_chars,
            "turns": record.turns,
            "api_seconds": py_round(record.api_seconds, 2),
            "startup_seconds": py_round(record.startup_seconds, 2),
            "output_tokens": record.output_tokens,
        }));
    }

    fn backoff(&self, attempt: u32, rate_limited: bool) -> f64 {
        if let Some(hook) = &self.backoff_hook {
            return hook(attempt, rate_limited);
        }
        let base = if rate_limited { self.rate_limit_backoff_s } else { self.backoff_base_s };
        base * 2f64.powi(attempt as i32) * rand::thread_rng().gen_range(0.8..=1.2)
    }

    async fn run_once(&self, prompt: &str, options: &ClaudeOptions) -> Result<Reply, Failure> {
        let started = Instant::now();
        let output = self.transport.query(prompt, options).await.map_err(query_failure)?;
        let mut texts: Vec<String> = Vec::new();
        let mut result = None;
        for message in &output.messages {
            match message.get("type").and_then(Value::as_str) {
                Some("assistant") if message.get("parent_tool_use_id").map_or(true, Value::is_null) => {
                    let blocks = message.pointer("/message/content").and_then(Value::as_array);
                    texts.extend(blocks.into_iter().flatten().filter_map(|b| {
                        (b.get("type").and_then(Value::as_str) == Some("text"))
                            .then(|| b.get("text").and_then(Value::as_str).map(str::to_string))
                            .flatten()
                    }));
                }
                Some("result") => {
                    result = Some(message);
                    break;
                }
                _ => {}
            }
        }
        let Some(result) = result else {
            return Err(Failure::Transient {
                message: "the CLI stream ended without a result".into(),
                rate_limited: false,
            });
        };
        let mut reply = from_result(result, &texts)?;
        reply.turns = result.get("num_turns").and_then(Value::as_i64).unwrap_or(0);
        reply.api_seconds = result.get("duration_api_ms").and_then(Value::as_f64).unwrap_or(0.0) / 1000.0;
        reply.startup_seconds =
            output.first_message.unwrap_or(started).saturating_duration_since(started).as_secs_f64();
        reply.output_tokens = result.pointer("/usage/output_tokens").and_then(Value::as_i64).unwrap_or(0);
        Ok(reply)
    }

    async fn call_with_retries(
        &self,
        prompt: &str,
        options: &ClaudeOptions,
        pending: &Pending<'_>,
        purpose: &str,
    ) -> Result<Reply, Failure> {
        for attempt in 0..=self.retries {
            pending.with(|r| r.attempts += 1);
            let outcome = {
                let _permit = self.semaphore.acquire().await.expect("semaphore never closed");
                tokio::time::timeout(self.timeout, self.run_once(prompt, options)).await
            };
            let (message, rate_limited) = match outcome {
                Ok(Ok(reply)) => {
                    pending.with(|r| {
                        r.turns += reply.turns;
                        r.api_seconds += reply.api_seconds;
                        r.startup_seconds += reply.startup_seconds;
                        r.output_tokens += reply.output_tokens;
                    });
                    return Ok(reply);
                }
                Err(_) => (format!("timed out after {:.0}s", self.timeout.as_secs_f64()), false),
                Ok(Err(Failure::Transient { message, rate_limited })) => (message, rate_limited),
                Ok(Err(other)) => return Err(other),
            };
            if attempt == self.retries {
                return Err(Failure::Llm(LlmError(format!(
                    "{purpose}: failed after {} attempts: {message}",
                    attempt + 1
                ))));
            }
            let wait = self.backoff(attempt, rate_limited);
            if wait > 0.0 {
                tokio::time::sleep(Duration::from_secs_f64(wait)).await;
            }
        }
        unreachable!("the loop returns on its last attempt")
    }

    /// The reply as a validated value, or BadOutput (problem, reply text).
    fn parse(reply: &Reply, schema: &Schema, pending: &Pending<'_>) -> Result<Value, Failure> {
        let structured_chars = reply.structured.as_ref().map(|v| py_dumps_ascii(v).chars().count()).unwrap_or(0);
        let output_chars = if structured_chars > 0 { structured_chars } else { reply.text.chars().count() };
        pending.with(|r| r.output_chars = output_chars);
        let bad = |message: String| Failure::BadOutput {
            message,
            reply_text: if reply.text.is_empty() {
                py_dumps_ascii(reply.structured.as_ref().unwrap_or(&Value::Null))
            } else {
                reply.text.clone()
            },
        };
        let opener = schema.root_opener();
        let value = match &reply.structured {
            Some(Value::String(s)) => extract_json(s, opener).map_err(|e| bad(e.to_string()))?,
            None => extract_json(&reply.text, opener).map_err(|e| bad(e.to_string()))?,
            Some(other) => other.clone(),
        };
        schema.validate(&value).map_err(bad)?;
        Ok(value)
    }
}

#[async_trait]
impl Llm for ClaudeCode {
    async fn text(&self, prompt: &str, system: &str, model: &str, purpose: &str) -> Result<String, LlmError> {
        let pending = self.pending(purpose, model, prompt.chars().count() + system.chars().count(), "text");
        let options = self.options(system, model, None, purpose);
        match self.call_with_retries(prompt, &options, &pending, purpose).await {
            Ok(reply) if !py_strip(&reply.text).is_empty() => {
                pending.with(|r| r.output_chars = reply.text.chars().count());
                pending.finish(None);
                Ok(py_strip(&reply.text).to_string())
            }
            Ok(_) => {
                pending.finish(Some(log_error("_BadOutput", "empty reply")));
                Err(LlmError(format!("{purpose}: unusable reply from Claude: empty reply")))
            }
            Err(Failure::BadOutput { message, .. }) => {
                pending.finish(Some(log_error("_BadOutput", &message)));
                Err(LlmError(format!("{purpose}: unusable reply from Claude: {message}")))
            }
            Err(Failure::Llm(e)) => {
                pending.finish(Some(log_error("LLMError", &e.0)));
                Err(e)
            }
            Err(Failure::Transient { .. }) => unreachable!("call_with_retries turns transient failures into LlmError"),
        }
    }

    async fn json_value(
        &self,
        prompt: &str,
        system: &str,
        model: &str,
        purpose: &str,
        schema: &Schema,
    ) -> Result<Value, LlmError> {
        let structured = self.structured_output;
        let mode = if structured { "structured" } else { "json" };
        let pending = self.pending(purpose, model, prompt.chars().count() + system.chars().count(), mode);
        let pending = &pending;
        let attempt = |prompt: String, options: ClaudeOptions| async move {
            let reply = self.call_with_retries(&prompt, &options, pending, purpose).await?;
            Self::parse(&reply, schema, pending)
        };
        let first = self.options(
            &json_system(system, &schema.value, structured),
            model,
            structured.then_some(&schema.value),
            purpose,
        );
        let result = match attempt(prompt.to_string(), first).await {
            Ok(value) => Ok(value),
            Err(Failure::Llm(e)) => Err(e),
            Err(Failure::Transient { .. }) => unreachable!("call_with_retries turns transient failures into LlmError"),
            Err(Failure::BadOutput { message, reply_text }) => {
                // One repair attempt in plain-JSON mode: it also covers a CLI that rejects the structured schema.
                let options = self.options(&json_system(system, &schema.value, false), model, None, purpose);
                match attempt(repair_prompt(prompt, &message, &reply_text), options).await {
                    Ok(value) => Ok(value),
                    Err(Failure::Llm(e)) => Err(e),
                    Err(Failure::BadOutput { message, .. }) => {
                        Err(LlmError(format!("{purpose}: invalid JSON after repair: {message}")))
                    }
                    Err(Failure::Transient { .. }) => unreachable!("turned into LlmError"),
                }
            }
        };
        pending.finish(result.as_ref().err().map(|e| log_error("LLMError", &e.0)));
        result
    }
}
