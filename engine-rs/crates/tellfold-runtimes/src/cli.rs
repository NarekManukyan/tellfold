//! Adapters for agent CLIs driven headless as subprocesses (port of `tellfold/runtimes/cli_tools.py`).
//!
//! Every call runs in a fresh, empty temporary directory (never the user's repos), without a shell, with the
//! prompt passed as one argv element or on stdin. No adapter grants tool permissions; where a CLI has a read-only
//! / ask / plan mode, a sandbox switch or a permission config, it is used to turn tools off. A CLI with no way to
//! turn its tools off is marked unsafe and refuses to run unless the user opts in with
//! TELLFOLD_ALLOW_UNSAFE_RUNTIMES. Children get an allowlisted environment (see [`crate::detect::child_env`]):
//! never the engine's token or the Keychain keys.
//!
//! Flags were taken from each tool's own `--help` (installed tools) or its official docs; see the notes on each
//! runtime in [`crate::registry`].

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use regex::Regex;
use serde_json::{Map, Value};
use tellfold_llm::adapter::{classify_failure, Adapter, AdapterError, AdapterReply, AdapterRequest};
use tellfold_models::json::py_str;
use tellfold_models::text::{py_prefix, py_splitlines, py_strip, py_suffix};

use crate::detect::{child_env, strip_ansi};
use crate::process::{ProcessResult, ProcessRunner, RealRunner};
use crate::settings::env;

pub const UNSAFE_OPT_IN_ENV: &str = "TELLFOLD_ALLOW_UNSAFE_RUNTIMES";

pub fn unsafe_runtime_allowed(runtime_id: &str) -> bool {
    let raw = env(UNSAFE_OPT_IN_ENV).unwrap_or_default().replace(';', ",");
    raw.split(',').map(|part| py_strip(part).to_lowercase()).any(|part| !part.is_empty() && part == runtime_id)
}

// ---------- output parsing ----------

const TEXT_KEYS: &[&str] = &["result", "response", "text", "content", "output", "message", "answer"];

/// Python truthiness of a JSON value.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

fn text_of(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) => Some(s.clone()),
        Value::Object(map) => ["text", "content", "result", "response"]
            .iter()
            .find_map(|key| map.get(*key).and_then(Value::as_str).map(str::to_string)),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().filter_map(|v| text_of(Some(v))).filter(|t| !t.is_empty()).collect();
            (!parts.is_empty()).then(|| parts.join("\n"))
        }
        _ => None,
    }
}

/// `a or b`: the text only when it is non-empty.
fn nonempty(text: Option<String>) -> Option<String> {
    text.filter(|t| !t.is_empty())
}

fn error_of(obj: &Map<String, Value>) -> Option<String> {
    let is = |key: &str, value: &str| obj.get(key).and_then(Value::as_str) == Some(value);
    if obj.get("is_error") == Some(&Value::Bool(true)) || is("subtype", "error") || is("type", "error") {
        return Some(
            nonempty(text_of(obj.get("error")))
                .or_else(|| nonempty(text_of(obj.get("result"))))
                .or_else(|| nonempty(text_of(obj.get("message"))))
                .unwrap_or_else(|| "the tool reported an error".into()),
        );
    }
    let error = obj.get("error").filter(|e| truthy(e))?;
    let answered =
        ["result", "response"].iter().any(|k| obj.get(*k).and_then(Value::as_str).is_some_and(|s| !s.is_empty()));
    if answered {
        return None;
    }
    Some(nonempty(text_of(Some(error))).unwrap_or_else(|| py_str(error)))
}

/// One JSON object (`{result}` / `{response}` style), a JSON list of messages, or JSONL events; else text.
pub fn parse_json_reply(stdout: &str, auth_hint: Option<&str>) -> Result<AdapterReply, AdapterError> {
    let stripped = strip_ansi(stdout);
    let text = py_strip(&stripped);
    if text.is_empty() {
        return Err(AdapterError::bad_output("the tool printed nothing", ""));
    }
    let value = match serde_json::from_str::<Value>(text) {
        Ok(value) => value,
        Err(_) => {
            let mut events: Vec<Value> = py_splitlines(text)
                .into_iter()
                .map(py_strip)
                .filter(|line| line.starts_with('{') || line.starts_with('['))
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            match events.len() {
                0 => return Ok(AdapterReply::text(text)),
                1 => events.remove(0),
                _ => Value::Array(events),
            }
        }
    };
    match &value {
        Value::Object(obj) => {
            if let Some(error) = error_of(obj) {
                return Err(classify_failure(&error, auth_hint));
            }
            for key in TEXT_KEYS {
                if let Some(found) = text_of(obj.get(*key)).filter(|t| !py_strip(t).is_empty()) {
                    let model = obj.get("model").and_then(Value::as_str).map(str::to_string);
                    return Ok(AdapterReply { text: found, model, ..AdapterReply::default() });
                }
            }
            Err(AdapterError::bad_output("the tool's JSON output had no response text", py_prefix(text, 2000)))
        }
        Value::Array(events) => Ok(AdapterReply::text(&collect_event_text(events, auth_hint)?)),
        other => Ok(AdapterReply::text(&py_str(other))),
    }
}

/// Final text from a list of stream events: the last `result` event, else the concatenated text parts.
pub fn collect_event_text(events: &[Value], auth_hint: Option<&str>) -> Result<String, AdapterError> {
    for ev in events.iter().rev() {
        let Some(obj) = ev.as_object() else { continue };
        if obj.get("type").and_then(Value::as_str) != Some("result") {
            continue;
        }
        if let Some(error) = error_of(obj) {
            return Err(classify_failure(&error, auth_hint));
        }
        if let Some(found) = nonempty(text_of(obj.get("result"))).or_else(|| nonempty(text_of(obj.get("response")))) {
            return Ok(found);
        }
    }
    let mut parts: Vec<String> = Vec::new();
    for ev in events {
        let Some(obj) = ev.as_object() else { continue };
        let kind = obj.get("type").and_then(Value::as_str);
        if kind == Some("error") {
            let message = nonempty(text_of(obj.get("error")))
                .or_else(|| nonempty(text_of(obj.get("message"))))
                .unwrap_or_else(|| "error event".into());
            return Err(classify_failure(&message, auth_hint));
        }
        let part = obj.get("part").and_then(Value::as_object).unwrap_or(obj);
        if part.get("type").and_then(Value::as_str) == Some("text") {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                parts.push(text.to_string());
                continue;
            }
        }
        let role = obj.get("role").map_or(Some("assistant"), Value::as_str);
        if matches!(kind, Some("message" | "assistant")) && role == Some("assistant") {
            if let Some(found) = nonempty(text_of(obj.get("content"))).or_else(|| nonempty(text_of(obj.get("message"))))
            {
                parts.push(found);
            }
        }
    }
    let joined = parts.concat();
    let joined = py_strip(&joined);
    if joined.is_empty() {
        return Err(AdapterError::bad_output("no assistant text in the tool's event stream", ""));
    }
    Ok(joined.to_string())
}

/// Plain-text output: the tool's stdout without ANSI codes.
fn plain_text(stdout: &str) -> Result<AdapterReply, AdapterError> {
    let stripped = strip_ansi(stdout);
    let text = py_strip(&stripped);
    if text.is_empty() {
        return Err(AdapterError::bad_output("the tool printed nothing", ""));
    }
    Ok(AdapterReply::text(text))
}

// ---------- the CLI adapter ----------

/// One agent CLI: how to call it and how to read its answer.
#[async_trait]
pub trait CliTool: Send + Sync {
    fn runtime_id(&self) -> &str;
    fn auth_hint(&self) -> Option<&str> {
        None
    }
    /// Parse stdout with [`parse_json_reply`] (else it is plain text).
    fn json_output(&self) -> bool {
        true
    }
    /// The tool's own variables (API key, config dir) that it may inherit; engine secrets are dropped regardless.
    fn passthrough_env(&self) -> &[&str] {
        &[]
    }
    /// True when the CLI has no way to turn its shell / file tools off in headless mode. Seed text is untrusted,
    /// so such a runtime only runs when the user lists its id in TELLFOLD_ALLOW_UNSAFE_RUNTIMES.
    fn is_unsafe(&self) -> bool {
        false
    }
    /// Variables the adapter sets for the child (they win over inherited ones).
    fn extra_env(&self) -> Vec<(&'static str, &'static str)> {
        Vec::new()
    }
    /// (argv, stdin text).
    async fn build(&self, binary: &str, request: &AdapterRequest, workdir: &Path) -> (Vec<String>, Option<String>);

    fn parse(&self, result: &ProcessResult, _workdir: &Path) -> Result<AdapterReply, AdapterError> {
        if self.json_output() {
            parse_json_reply(&result.stdout, self.auth_hint())
        } else {
            plain_text(&result.stdout)
        }
    }
}

/// Runs one CLI invocation per request.
pub struct CliAdapter {
    pub tool: Box<dyn CliTool>,
    pub binary: String,
    pub runner: Arc<dyn ProcessRunner>,
}

impl CliAdapter {
    pub fn new(tool: impl CliTool + 'static, binary: &str) -> CliAdapter {
        CliAdapter::boxed(Box::new(tool), binary)
    }

    pub fn boxed(tool: Box<dyn CliTool>, binary: &str) -> CliAdapter {
        CliAdapter { tool, binary: binary.into(), runner: Arc::new(RealRunner) }
    }

    pub fn with_runner(mut self, runner: Arc<dyn ProcessRunner>) -> CliAdapter {
        self.runner = runner;
        self
    }

    pub fn env(&self) -> BTreeMap<String, String> {
        child_env(Some(&self.binary), &self.tool.extra_env(), self.tool.passthrough_env())
    }
}

#[async_trait]
impl Adapter for CliAdapter {
    fn runtime_id(&self) -> &str {
        self.tool.runtime_id()
    }

    async fn send(&self, request: &AdapterRequest) -> Result<AdapterReply, AdapterError> {
        let id = self.tool.runtime_id();
        if self.tool.is_unsafe() && !unsafe_runtime_allowed(id) {
            return Err(AdapterError::Fatal(format!(
                "{id} is disabled: it cannot turn off its shell and file tools, so text in a seed document could \
                 make it run commands. Set {UNSAFE_OPT_IN_ENV}={id} to use it anyway."
            )));
        }
        if self.binary.is_empty() || !Path::new(&self.binary).exists() {
            let shown = if self.binary.is_empty() { "not found" } else { &self.binary };
            return Err(AdapterError::Fatal(format!("{id} is not installed (binary {shown})")));
        }
        let workdir = tempfile::Builder::new()
            .prefix("tellfold-rt-")
            .tempdir()
            .map_err(|e| AdapterError::Other { kind: "OSError".into(), message: e.to_string() })?;
        let (argv, stdin) = self.tool.build(&self.binary, request, workdir.path()).await;
        let result = match self.runner.run(&argv, stdin.as_deref(), workdir.path(), &self.env()).await {
            Ok(result) => result,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(AdapterError::Fatal(format!("{id}: cannot start {}: {e}", self.binary)));
            }
            Err(e) => return Err(AdapterError::transient(format!("cannot start {}: {e}", self.binary))),
        };
        if result.returncode != 0 {
            let stderr = strip_ansi(&result.stderr);
            let stdout = strip_ansi(&result.stdout);
            let message = match py_strip(&stderr) {
                "" => py_strip(&stdout),
                text => text,
            };
            // a JSON error object on stdout says more than the exit code
            if let Err(e @ (AdapterError::Fatal(_) | AdapterError::Transient { .. })) =
                parse_json_reply(&result.stdout, self.tool.auth_hint())
            {
                return Err(e);
            }
            return Err(classify_failure(
                &format!("exit {}: {}", result.returncode, py_suffix(message, 1500)),
                self.tool.auth_hint(),
            ));
        }
        self.tool.parse(&result, workdir.path())
    }
}

fn path_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn with_model(mut argv: Vec<String>, flag: &str, request: &AdapterRequest) -> Vec<String> {
    if let Some(model) = request.model_or_none() {
        argv.push(flag.into());
        argv.push(model.into());
    }
    argv
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

// ---------- adapters ----------

/// `cursor-agent -p --output-format json --mode ask --sandbox enabled --trust --workspace <tmp> [--model M] PROMPT`.
///
/// `--mode ask` is read-only Q&A; `--force` and `--approve-mcps` are never passed, so no command runs and no MCP
/// server is approved. `--trust` only skips the "Workspace Trust Required" prompt for the empty scratch folder;
/// without it every headless call exits 1. JSON output is one `{type: "result", is_error, result, ...}` object;
/// errors exit non-zero with text on stderr.
pub struct CursorAgent;

#[async_trait]
impl CliTool for CursorAgent {
    fn runtime_id(&self) -> &str {
        "cursor-agent"
    }
    fn auth_hint(&self) -> Option<&str> {
        Some("Cursor Agent is not logged in: run `cursor-agent login` (or set CURSOR_API_KEY).")
    }
    fn passthrough_env(&self) -> &[&str] {
        &["CURSOR_API_KEY", "CURSOR_CONFIG_DIR"]
    }
    async fn build(&self, binary: &str, request: &AdapterRequest, workdir: &Path) -> (Vec<String>, Option<String>) {
        let mut argv = strings(&[binary, "-p", "--output-format", "json", "--mode", "ask", "--sandbox", "enabled"]);
        argv.extend(strings(&["--trust", "--workspace"]));
        argv.push(path_str(workdir));
        let mut argv = with_model(argv, "--model", request);
        argv.push(request.combined());
        (argv, None)
    }
}

/// `codex exec --skip-git-repo-check --sandbox read-only --ephemeral -C <tmp> -o <tmp>/last-message.txt [-m M] -`
/// with the prompt on stdin. The final message is read from the `-o` file.
pub struct Codex;

const CODEX_LAST_MESSAGE: &str = "last-message.txt";

#[async_trait]
impl CliTool for Codex {
    fn runtime_id(&self) -> &str {
        "codex"
    }
    fn auth_hint(&self) -> Option<&str> {
        Some("Codex is not logged in: run `codex login`.")
    }
    fn json_output(&self) -> bool {
        false
    }
    fn passthrough_env(&self) -> &[&str] {
        &["CODEX_HOME"]
    }
    async fn build(&self, binary: &str, request: &AdapterRequest, workdir: &Path) -> (Vec<String>, Option<String>) {
        let mut argv = strings(&[binary, "exec", "--skip-git-repo-check", "--sandbox", "read-only", "--ephemeral"]);
        argv.extend(["-C".into(), path_str(workdir), "--output-last-message".into()]);
        argv.push(path_str(&workdir.join(CODEX_LAST_MESSAGE)));
        let mut argv = with_model(argv, "-m", request);
        argv.push("-".into());
        (argv, Some(request.combined()))
    }
    fn parse(&self, result: &ProcessResult, workdir: &Path) -> Result<AdapterReply, AdapterError> {
        if let Ok(text) = std::fs::read_to_string(workdir.join(CODEX_LAST_MESSAGE)) {
            let text = py_strip(&text);
            if !text.is_empty() {
                return Ok(AdapterReply::text(text));
            }
        }
        plain_text(&result.stdout)
    }
}

const GEMINI_TRUST: &[(&str, &str)] = &[("GEMINI_CLI_TRUST_WORKSPACE", "true")];

/// `gemini --output-format json --approval-mode default [--model M] --prompt PROMPT`. JSON output is
/// `{response, stats, error?}`. `default` approval never auto-approves a tool. Folder trust is on by default since
/// v0.39.1 and headless runs exit in an untrusted folder, so GEMINI_CLI_TRUST_WORKSPACE trusts the empty scratch
/// folder (older versions ignore the variable).
pub struct Gemini;

async fn gemini_argv(binary: &str, request: &AdapterRequest) -> (Vec<String>, Option<String>) {
    let argv = strings(&[binary, "--output-format", "json", "--approval-mode", "default"]);
    let mut argv = with_model(argv, "--model", request);
    argv.extend(["--prompt".into(), request.combined()]);
    (argv, None)
}

#[async_trait]
impl CliTool for Gemini {
    fn runtime_id(&self) -> &str {
        "gemini-cli"
    }
    fn auth_hint(&self) -> Option<&str> {
        Some("Gemini CLI is not signed in: run `gemini` once to log in, or set GEMINI_API_KEY.")
    }
    fn passthrough_env(&self) -> &[&str] {
        &[
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
            "GOOGLE_CLOUD_PROJECT",
            "GOOGLE_CLOUD_LOCATION",
            "GOOGLE_GENAI_USE_VERTEXAI",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "GEMINI_CLI_HOME",
        ]
    }
    fn extra_env(&self) -> Vec<(&'static str, &'static str)> {
        GEMINI_TRUST.to_vec()
    }
    async fn build(&self, binary: &str, request: &AdapterRequest, _workdir: &Path) -> (Vec<String>, Option<String>) {
        gemini_argv(binary, request).await
    }
}

/// `qwen --output-format json --approval-mode default [--model M] --prompt PROMPT` (a Gemini CLI fork). The JSON
/// shape varies across versions (`{response}` or a list of messages ending in a `result`); both are handled.
/// Without `--approval-mode` Qwen now defaults to `auto`, which approves edits and "safe" commands.
pub struct Qwen;

#[async_trait]
impl CliTool for Qwen {
    fn runtime_id(&self) -> &str {
        "qwen-code"
    }
    fn auth_hint(&self) -> Option<&str> {
        Some("Qwen Code is not signed in: run `qwen` once to log in, or set DASHSCOPE_API_KEY.")
    }
    fn passthrough_env(&self) -> &[&str] {
        &["DASHSCOPE_API_KEY"]
    }
    fn extra_env(&self) -> Vec<(&'static str, &'static str)> {
        GEMINI_TRUST.to_vec()
    }
    async fn build(&self, binary: &str, request: &AdapterRequest, _workdir: &Path) -> (Vec<String>, Option<String>) {
        gemini_argv(binary, request).await
    }
}

/// OpenCode allows its bash, edit and webfetch tools without asking by default, and `run` never prompts. This
/// inline config (OPENCODE_CONFIG_CONTENT is merged over the user's own config) denies all three and switches the
/// tools off.
pub const OPENCODE_LOCKDOWN_CONFIG: &str = r#"{"permission":{"edit":"deny","bash":"deny","webfetch":"deny"},"tools":{"bash":false,"edit":false,"write":false,"patch":false,"webfetch":false}}"#;

static OPENCODE_VERSIONS: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();

#[doc(hidden)]
pub fn clear_opencode_version_cache() {
    OPENCODE_VERSIONS.get_or_init(Default::default).lock().expect("not poisoned").clear();
}

/// The major version `opencode --version` prints, or 1 when it can't be read.
pub async fn opencode_major_version(binary: &str) -> i64 {
    let cache = OPENCODE_VERSIONS.get_or_init(Default::default);
    if let Some(&major) = cache.lock().expect("not poisoned").get(binary) {
        return major;
    }
    let output = tokio::process::Command::new(binary)
        .arg("--version")
        .env_clear()
        .envs(child_env(Some(binary), &[], &[]))
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let major = match tokio::time::timeout(Duration::from_secs(10), output).await {
        Ok(Ok(out)) => {
            static R: OnceLock<Regex> = OnceLock::new();
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            R.get_or_init(|| Regex::new(r"(\d+)\.\d+").expect("valid pattern"))
                .captures(&text)
                .and_then(|c| c[1].parse().ok())
                .unwrap_or(1)
        }
        _ => 1,
    };
    cache.lock().expect("not poisoned").insert(binary.into(), major);
    major
}

/// `opencode run --format json [--standalone] [--model provider/model] PROMPT` in the scratch folder. JSON format
/// is a stream of events; text parts are concatenated. Never passes `--auto`, and OPENCODE_CONFIG_CONTENT denies
/// the bash, edit and webfetch permissions (OpenCode allows them by default and `run` never asks). v2 dropped
/// `--dir` and attaches `run` to an already running background server, which never sees that config, so v2 gets
/// `--standalone` (a private server started with this environment).
pub struct OpenCode;

#[async_trait]
impl CliTool for OpenCode {
    fn runtime_id(&self) -> &str {
        "opencode"
    }
    fn auth_hint(&self) -> Option<&str> {
        Some("OpenCode has no provider configured: run `opencode auth login`.")
    }
    fn extra_env(&self) -> Vec<(&'static str, &'static str)> {
        vec![("OPENCODE_CONFIG_CONTENT", OPENCODE_LOCKDOWN_CONFIG)]
    }
    async fn build(&self, binary: &str, request: &AdapterRequest, _workdir: &Path) -> (Vec<String>, Option<String>) {
        let mut argv = strings(&[binary, "run", "--format", "json"]);
        if opencode_major_version(binary).await >= 2 {
            argv.push("--standalone".into());
        }
        let mut argv = with_model(argv, "--model", request);
        argv.push(request.combined());
        (argv, None)
    }
}

/// `copilot -s --no-ask-user --deny-tool shell --deny-tool write [--model M] -p PROMPT`. `-s` prints only the
/// agent's response as plain text.
pub struct Copilot;

#[async_trait]
impl CliTool for Copilot {
    fn runtime_id(&self) -> &str {
        "copilot"
    }
    fn auth_hint(&self) -> Option<&str> {
        Some("GitHub Copilot CLI is not signed in: run `copilot` and use /login, or set GH_TOKEN.")
    }
    fn json_output(&self) -> bool {
        false
    }
    fn passthrough_env(&self) -> &[&str] {
        &["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN", "COPILOT_HOME"]
    }
    async fn build(&self, binary: &str, request: &AdapterRequest, _workdir: &Path) -> (Vec<String>, Option<String>) {
        let argv = strings(&[binary, "-s", "--no-ask-user", "--deny-tool", "shell", "--deny-tool", "write"]);
        let mut argv = with_model(argv, "--model", request);
        argv.extend(["-p".into(), request.combined()]);
        (argv, None)
    }
}

/// `grok --directory <tmp> [--model M] --prompt PROMPT` (community grok-cli; not an official xAI tool).
/// Plain-text output.
///
/// Unsafe: grok-cli's headless agent loop has bash and file-edit tools, runs them without asking, and has no
/// documented switch to turn them off. It only runs when TELLFOLD_ALLOW_UNSAFE_RUNTIMES lists `grok`.
pub struct Grok;

#[async_trait]
impl CliTool for Grok {
    fn runtime_id(&self) -> &str {
        "grok"
    }
    fn auth_hint(&self) -> Option<&str> {
        Some("Grok CLI needs an API key: set GROK_API_KEY.")
    }
    fn json_output(&self) -> bool {
        false
    }
    fn passthrough_env(&self) -> &[&str] {
        &["GROK_API_KEY", "GROK_BASE_URL"]
    }
    fn is_unsafe(&self) -> bool {
        true
    }
    async fn build(&self, binary: &str, request: &AdapterRequest, workdir: &Path) -> (Vec<String>, Option<String>) {
        let argv = vec![binary.to_string(), "--directory".into(), path_str(workdir)];
        let mut argv = with_model(argv, "--model", request);
        argv.extend(["--prompt".into(), request.combined()]);
        (argv, None)
    }
}

/// `droid exec --output-format json --cwd <tmp> [--model M] PROMPT`. Without `--auto` droid exec is read-only.
/// The JSON result's text field is undocumented; the generic parser looks for `result`/`response`.
pub struct Droid;

#[async_trait]
impl CliTool for Droid {
    fn runtime_id(&self) -> &str {
        "droid"
    }
    fn auth_hint(&self) -> Option<&str> {
        Some("Droid is not signed in: run `droid` once to log in, or set FACTORY_API_KEY.")
    }
    fn passthrough_env(&self) -> &[&str] {
        &["FACTORY_API_KEY"]
    }
    async fn build(&self, binary: &str, request: &AdapterRequest, workdir: &Path) -> (Vec<String>, Option<String>) {
        let mut argv = strings(&[binary, "exec", "--output-format", "json", "--cwd"]);
        argv.push(path_str(workdir));
        let mut argv = with_model(argv, "--model", request);
        argv.push(request.combined());
        (argv, None)
    }
}
