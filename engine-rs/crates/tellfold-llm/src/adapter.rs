//! `AdapterLlm`: the machinery every AI tool shares (port of `AdapterLLM` in `tellfold/runtimes/base.py`).
//!
//! It gives each adapter the same guarantees: only [`LlmError`] escapes, retries with exponential backoff (longer
//! after a rate limit), a per-attempt timeout, a concurrency semaphore, one `calls.jsonl` line per call (lengths
//! and timings, never prompt text), and JSON answers validated against the schema with one repair retry. An
//! adapter implements one method, [`Adapter::send`].

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Utc;
use rand::Rng;
use serde_json::{json, Value};
use tellfold_models::json::{py_dumps, py_dumps_ascii, py_round};
use tellfold_models::text::{py_prefix, py_split, py_strip};
use tokio::sync::Semaphore;

use crate::{extract_json, Llm, LlmError, Schema};

/// A model value meaning "whatever the tool itself defaults to": adapters omit their model flag.
pub const TOOL_DEFAULT: &str = "default";
/// Claude Code model aliases. Other runtimes swap these for their own default model.
pub const CLAUDE_ALIASES: &[&str] = &["haiku", "sonnet", "opus"];
const REPAIR_SNIPPET_CHARS: usize = 1500;

#[derive(Debug, Clone, PartialEq)]
pub struct AdapterRequest {
    pub prompt: String,
    pub system: String,
    /// "" or [`TOOL_DEFAULT`] means: use the tool's or server's default.
    pub model: String,
    pub purpose: String,
    /// Set for JSON calls; adapters may enforce it natively.
    pub json_schema: Option<Value>,
    pub schema_name: String,
}

impl AdapterRequest {
    pub fn wants_json(&self) -> bool {
        self.json_schema.is_some()
    }

    pub fn model_or_none(&self) -> Option<&str> {
        (!self.model.is_empty() && self.model != TOOL_DEFAULT).then_some(self.model.as_str())
    }

    /// System + prompt as one text, for tools that have no separate system prompt input.
    pub fn combined(&self) -> String {
        format!("<instructions>\n{}\n</instructions>\n\n{}", py_strip(&self.system), self.prompt)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AdapterReply {
    pub text: String,
    /// An already-parsed JSON value, when the backend returned one.
    pub structured: Option<Value>,
    /// The model that actually answered, if the backend says.
    pub model: Option<String>,
    pub output_tokens: i64,
}

impl AdapterReply {
    pub fn text(text: &str) -> Self {
        AdapterReply { text: text.into(), ..Default::default() }
    }
}

/// What can go wrong inside an adapter.
#[derive(Debug, Clone, PartialEq)]
pub enum AdapterError {
    /// Worth retrying after a backoff: process crash, timeout, connection error, 5xx, rate limit.
    Transient { message: String, rate_limited: bool },
    /// Retrying will not help: not installed, not logged in, unknown model, bad request.
    Fatal(String),
    /// The model answered, but not usefully; a repair prompt may fix it.
    BadOutput { message: String, reply_text: String },
    /// Already a final error (passes through unchanged).
    Llm(LlmError),
    /// An unexpected failure (an adapter bug): `kind` is the error's type name.
    Other { kind: String, message: String },
}

impl AdapterError {
    pub fn transient(message: impl Into<String>) -> Self {
        AdapterError::Transient { message: message.into(), rate_limited: false }
    }
    pub fn rate_limited(message: impl Into<String>) -> Self {
        AdapterError::Transient { message: message.into(), rate_limited: true }
    }
    pub fn bad_output(message: impl Into<String>, reply_text: impl Into<String>) -> Self {
        AdapterError::BadOutput { message: message.into(), reply_text: reply_text.into() }
    }

    pub fn message(&self) -> String {
        match self {
            AdapterError::Transient { message, .. } | AdapterError::Fatal(message) => message.clone(),
            AdapterError::BadOutput { message, .. } => message.clone(),
            AdapterError::Llm(e) => e.0.clone(),
            AdapterError::Other { message, .. } => message.clone(),
        }
    }
}

/// One AI tool's transport: send a request, get a reply.
#[async_trait]
pub trait Adapter: Send + Sync {
    /// The runtime id shown in errors and the call log (`"cursor-agent"`, `"ollama"`, ...).
    fn runtime_id(&self) -> &str;
    async fn send(&self, request: &AdapterRequest) -> Result<AdapterReply, AdapterError>;
}

/// A shared adapter (tests keep a handle to inspect what it was sent).
#[async_trait]
impl<A: Adapter + ?Sized> Adapter for std::sync::Arc<A> {
    fn runtime_id(&self) -> &str {
        (**self).runtime_id()
    }
    async fn send(&self, request: &AdapterRequest) -> Result<AdapterReply, AdapterError> {
        (**self).send(request).await
    }
}

// ---------- prompts shared by every adapter ----------

pub fn json_system(system: &str, json_schema: &Value) -> String {
    let contract = format!(
        "Your reply is parsed by a program. Respond with exactly one JSON value that validates against \
         the JSON Schema below: no prose before or after it, no markdown code fences, no comments, \
         no trailing commas. Every required field must be present and every type must match. Do not use \
         any tools; answer from the text you were given.\n\n\
         JSON Schema:\n{}",
        py_dumps(json_schema)
    );
    format!("{}\n\n# Output format\n{contract}", system.trim_end_matches(tellfold_models::text::py_isspace))
}

pub fn repair_prompt(prompt: &str, problem: &str, previous: &str) -> String {
    let snippet = py_prefix(py_strip(previous), REPAIR_SNIPPET_CHARS);
    let snippet = if snippet.is_empty() { "(empty)" } else { snippet };
    format!(
        "{prompt}\n\n---\nYour previous answer to this request could not be used.\n\
         Problem: {}\n\
         Previous answer (truncated):\n{snippet}\n\n\
         Answer the original request again, fixing the problem. Output only the corrected JSON.",
        py_prefix(problem, 800)
    )
}

pub fn root_opener(json_schema: &Value) -> char {
    if json_schema.get("type").and_then(Value::as_str) == Some("array") {
        '['
    } else {
        '{'
    }
}

// ---------- error text classification (CLI stderr, HTTP bodies) ----------

pub const AUTH_HINTS: &[&str] = &[
    "not logged in",
    "login",
    "log in",
    "authenticat",
    "unauthorized",
    "api key",
    "api_key",
    "credentials",
    "forbidden",
    "401",
    "403",
];
pub const RATE_HINTS: &[&str] = &[
    "rate limit",
    "rate_limit",
    "ratelimit",
    "usage limit",
    "quota",
    "too many requests",
    "overloaded",
    "429",
    "resource_exhausted",
];
pub const MODEL_HINTS: &[&str] = &[
    "unknown model",
    "invalid model",
    "model not found",
    "model_not_found",
    "no such model",
    "not a valid model",
    "unsupported model",
    "does not exist",
];

/// Map a tool's error text onto Fatal (auth / model) or Transient (rate limit / anything else).
pub fn classify_failure(message: &str, auth_hint: Option<&str>) -> AdapterError {
    let joined = py_split(message).collect::<Vec<_>>().join(" ");
    let text = py_prefix(&joined, 400).to_string();
    let lowered = text.to_lowercase();
    if RATE_HINTS.iter().any(|h| lowered.contains(h)) {
        return AdapterError::rate_limited(if text.is_empty() { "rate limited".into() } else { text });
    }
    if MODEL_HINTS.iter().any(|h| lowered.contains(h)) {
        return AdapterError::Fatal(text);
    }
    if AUTH_HINTS.iter().any(|h| lowered.contains(h)) {
        return AdapterError::Fatal(match auth_hint {
            Some(hint) => format!("{hint} ({text})"),
            None => text,
        });
    }
    AdapterError::transient(if text.is_empty() { "the tool failed without an error message".into() } else { text })
}

// ---------- the call log ----------

struct CallRecord {
    purpose: String,
    model: String,
    prompt_chars: usize,
    mode: &'static str,
    started: Instant,
    attempts: u32,
    output_chars: usize,
    output_tokens: i64,
}

/// Appends one `calls.jsonl` line per call: lengths and timings, never prompt or answer text.
#[derive(Debug, Clone, Default)]
pub struct CallLog {
    path: Option<PathBuf>,
}

impl CallLog {
    pub fn new(path: Option<PathBuf>) -> Self {
        CallLog { path }
    }

    pub fn path(&self) -> Option<&PathBuf> {
        self.path.as_ref()
    }

    /// Writes one record; `fields` follow the common ones in order. Failures to write are ignored.
    pub fn write(&self, entry: &Value) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = file.write_all((py_dumps(entry) + "\n").as_bytes());
        }
    }
}

/// `datetime.now(timezone.utc).isoformat(timespec="seconds")`: `2026-11-10T12:00:00+00:00`.
pub fn log_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
}

/// `f"{type(error).__name__}: {str(error)[:300]}"` for the log's `error` field.
pub fn log_error(kind: &str, message: &str) -> String {
    format!("{kind}: {}", py_prefix(message, 300))
}

// ---------- the shared implementation ----------

pub struct AdapterLlm<A: Adapter> {
    pub adapter: A,
    semaphore: Semaphore,
    log: CallLog,
    pub timeout: Duration,
    pub retries: u32,
    pub backoff_base_s: f64,
    pub rate_limit_backoff_s: f64,
    /// Replaces the sleep between attempts (tests record or skip it).
    #[allow(clippy::type_complexity)]
    pub backoff_hook: Option<Box<dyn Fn(u32, bool) -> f64 + Send + Sync>>,
}

/// Logs the call if it is dropped before finishing, as Python logs a cancelled call.
struct Pending<'a, A: Adapter> {
    llm: &'a AdapterLlm<A>,
    record: Mutex<Option<CallRecord>>,
}

impl<A: Adapter> Pending<'_, A> {
    fn with<T>(&self, f: impl FnOnce(&mut CallRecord) -> T) -> T {
        f(self.record.lock().expect("not poisoned").as_mut().expect("record present until finished"))
    }

    fn finish(&self, error: Option<&LlmError>) {
        if let Some(record) = self.record.lock().expect("not poisoned").take() {
            self.llm.write_log(&record, error.map(|e| log_error("LLMError", &e.0)));
        }
    }
}

impl<A: Adapter> Drop for Pending<'_, A> {
    fn drop(&mut self) {
        if let Some(record) = self.record.get_mut().ok().and_then(Option::take) {
            self.llm.write_log(&record, Some("CancelledError: ".into()));
        }
    }
}

impl<A: Adapter> AdapterLlm<A> {
    pub fn new(adapter: A, concurrency: usize, log_path: Option<PathBuf>) -> Self {
        AdapterLlm {
            adapter,
            semaphore: Semaphore::new(concurrency.max(1)),
            log: CallLog::new(log_path),
            timeout: Duration::from_secs(240),
            retries: 2,
            backoff_base_s: 2.0,
            rate_limit_backoff_s: 20.0,
            backoff_hook: None,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_retries(mut self, retries: u32) -> Self {
        self.retries = retries;
        self
    }

    /// No waiting between attempts (tests).
    pub fn without_backoff(mut self) -> Self {
        self.backoff_base_s = 0.0;
        self.rate_limit_backoff_s = 0.0;
        self
    }

    fn runtime_id(&self) -> &str {
        self.adapter.runtime_id()
    }

    fn pending(&self, purpose: &str, model: &str, prompt_chars: usize, mode: &'static str) -> Pending<'_, A> {
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
                output_tokens: 0,
            })),
        }
    }

    fn write_log(&self, record: &CallRecord, error: Option<String>) {
        self.log.write(&json!({
            "ts": log_timestamp(),
            "purpose": record.purpose,
            "model": record.model,
            "runtime": self.runtime_id(),
            "mode": record.mode,
            "seconds": py_round(record.started.elapsed().as_secs_f64(), 3),
            "attempts": record.attempts,
            "ok": error.is_none(),
            "error": error,
            "prompt_chars": record.prompt_chars,
            "output_chars": record.output_chars,
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

    /// Sends with retries. BadOutput comes back as `Err(Ok(..))` for the caller's repair logic; every other failure
    /// is final (`Err(Err(LlmError))`).
    async fn send_with_retries(
        &self,
        request: &AdapterRequest,
        pending: &Pending<'_, A>,
    ) -> Result<AdapterReply, Result<(String, String), LlmError>> {
        let purpose = &request.purpose;
        let id = self.runtime_id();
        for attempt in 0..=self.retries {
            pending.with(|r| r.attempts += 1);
            let outcome = {
                let _permit = self.semaphore.acquire().await.expect("semaphore never closed");
                tokio::time::timeout(self.timeout, self.adapter.send(request)).await
            };
            let failure = match outcome {
                Ok(Ok(reply)) => {
                    pending.with(|r| r.output_tokens += reply.output_tokens);
                    return Ok(reply);
                }
                Err(_) => AdapterError::transient(format!("timed out after {:.0}s", self.timeout.as_secs_f64())),
                Ok(Err(e @ AdapterError::Transient { .. })) => e,
                Ok(Err(AdapterError::Fatal(m))) => return Err(Err(LlmError(format!("{purpose}: {id}: {m}")))),
                Ok(Err(AdapterError::BadOutput { message, reply_text })) => return Err(Ok((message, reply_text))),
                Ok(Err(AdapterError::Llm(e))) => return Err(Err(e)),
                Ok(Err(AdapterError::Other { kind, message })) => {
                    return Err(Err(LlmError(format!("{purpose}: {id}: {kind}: {}", py_prefix(&message, 300)))));
                }
            };
            let AdapterError::Transient { message, rate_limited } = failure else { unreachable!() };
            if attempt == self.retries {
                return Err(Err(LlmError(format!("{purpose}: {id} failed after {} attempts: {message}", attempt + 1))));
            }
            let wait = self.backoff(attempt, rate_limited);
            if wait > 0.0 {
                tokio::time::sleep(Duration::from_secs_f64(wait)).await;
            }
        }
        unreachable!("the loop returns on its last attempt")
    }

    /// The reply as a validated value, or BadOutput (problem, reply text).
    fn parse(reply: &AdapterReply, schema: &Schema, pending: &Pending<'_, A>) -> Result<Value, (String, String)> {
        let structured_chars = reply.structured.as_ref().map(|v| py_dumps_ascii(v).chars().count()).unwrap_or(0);
        let output_chars = if structured_chars > 0 { structured_chars } else { reply.text.chars().count() };
        pending.with(|r| r.output_chars = output_chars);
        let reply_text = || {
            if reply.text.is_empty() {
                py_dumps_ascii(&reply.structured.clone().unwrap_or(Value::Null))
            } else {
                reply.text.clone()
            }
        };
        let opener = schema.root_opener();
        let value = match &reply.structured {
            Some(Value::String(s)) => extract_json(s, opener).map_err(|e| (e.to_string(), reply_text()))?,
            Some(Value::Null) | None => extract_json(&reply.text, opener).map_err(|e| (e.to_string(), reply_text()))?,
            Some(other) => other.clone(),
        };
        schema.validate(&value).map_err(|problem| (problem, reply_text()))?;
        Ok(value)
    }
}

#[async_trait]
impl<A: Adapter> Llm for AdapterLlm<A> {
    async fn text(&self, prompt: &str, system: &str, model: &str, purpose: &str) -> Result<String, LlmError> {
        let pending = self.pending(purpose, model, prompt.chars().count() + system.chars().count(), "text");
        let request = AdapterRequest {
            prompt: prompt.into(),
            system: system.into(),
            model: model.into(),
            purpose: purpose.into(),
            json_schema: None,
            schema_name: "result".into(),
        };
        let unusable =
            |problem: &str| LlmError(format!("{purpose}: unusable reply from {}: {problem}", self.runtime_id()));
        let result = match self.send_with_retries(&request, &pending).await {
            Ok(reply) => {
                let mut text = py_strip(&reply.text).to_string();
                if text.is_empty() {
                    if let Some(structured) = &reply.structured {
                        text = py_dumps(structured);
                    }
                }
                if text.is_empty() {
                    Err(unusable("empty reply"))
                } else {
                    Ok(text)
                }
            }
            Err(Ok((problem, _))) => Err(unusable(&problem)),
            Err(Err(e)) => Err(e),
        };
        match &result {
            Ok(text) => {
                pending.with(|r| r.output_chars = text.chars().count());
                pending.finish(None);
            }
            Err(e) => pending.finish(Some(e)),
        }
        result
    }

    async fn json_value(
        &self,
        prompt: &str,
        system: &str,
        model: &str,
        purpose: &str,
        schema: &Schema,
    ) -> Result<Value, LlmError> {
        let pending = self.pending(purpose, model, prompt.chars().count() + system.chars().count(), "json");
        let request = AdapterRequest {
            prompt: prompt.into(),
            system: json_system(system, &schema.value),
            model: model.into(),
            purpose: purpose.into(),
            json_schema: Some(schema.value.clone()),
            schema_name: schema.name.clone(),
        };
        let pending = &pending;
        let attempt = |req: AdapterRequest| async move {
            match self.send_with_retries(&req, pending).await {
                Ok(reply) => Self::parse(&reply, schema, pending).map_err(Ok),
                Err(e) => Err(e),
            }
        };
        let result = match attempt(request.clone()).await {
            Ok(value) => Ok(value),
            Err(Err(e)) => Err(e),
            Err(Ok((problem, previous))) => {
                let repair = AdapterRequest { prompt: repair_prompt(prompt, &problem, &previous), ..request };
                match attempt(repair).await {
                    Ok(value) => Ok(value),
                    Err(Err(e)) => Err(e),
                    Err(Ok((second, _))) => Err(LlmError(format!("{purpose}: invalid JSON after repair: {second}"))),
                }
            }
        };
        pending.finish(result.as_ref().err());
        result
    }
}
