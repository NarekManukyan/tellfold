//! The Claude Code adapter (port of the `ClaudeLocal` cases in `tests/test_llm.py`): the CLI stream is scripted,
//! as Python replaces `claude_agent_sdk.query`. The last tests run the real transport against a fake `claude`
//! shell script.

mod common;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use common::{block_on, make_script, pick_schema, Pick};
use serde_json::{json, Value};
use tellfold_llm::{json_with, Llm};
use tellfold_models::Report;
use tellfold_runtimes::claude::{ClaudeCode, ClaudeOptions, ClaudeTransport, QueryError, QueryOutput};

fn result(text: &str) -> Value {
    json!({"type": "result", "subtype": "success", "duration_ms": 5, "duration_api_ms": 4, "is_error": false,
           "num_turns": 1, "session_id": "s", "result": text, "structured_output": null,
           "api_error_status": null, "terminal_reason": null})
}

fn structured(value: Value) -> Value {
    let mut r = result("");
    r["structured_output"] = value;
    r
}

fn error_result(text: &str, status: Option<i64>, reason: Option<&str>) -> Value {
    let mut r = result(text);
    r["is_error"] = json!(true);
    r["api_error_status"] = json!(status);
    r["terminal_reason"] = json!(reason);
    r
}

fn assistant(text: &str) -> Value {
    json!({"type": "assistant", "message": {"content": [{"type": "text", "text": text}], "model": "haiku"},
           "parent_tool_use_id": null})
}

type Step = Result<Vec<Value>, QueryError>;

/// Stands in for the CLI. Each call consumes one script entry: the messages to stream, or an error.
#[derive(Default)]
struct Scripted {
    script: Mutex<Vec<Step>>,
    calls: Mutex<Vec<(String, ClaudeOptions)>>,
}

impl Scripted {
    fn new(script: Vec<Step>) -> Arc<Scripted> {
        Arc::new(Scripted { script: Mutex::new(script), calls: Mutex::default() })
    }

    fn calls(&self) -> Vec<(String, ClaudeOptions)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl ClaudeTransport for Scripted {
    async fn query(&self, prompt: &str, options: &ClaudeOptions) -> Result<QueryOutput, QueryError> {
        self.calls.lock().unwrap().push((prompt.into(), options.clone()));
        let step = self.script.lock().unwrap().remove(0);
        step.map(|messages| QueryOutput { messages, first_message: Some(Instant::now()) })
    }
}

fn failed(message: &str) -> Step {
    Err(QueryError::Failed { kind: "ProcessError".into(), message: message.into() })
}

fn llm(log: Option<PathBuf>, transport: &Arc<Scripted>) -> ClaudeCode {
    let mut llm = ClaudeCode::new(2, log, Some("/bin/claude-test".into())).with_transport(transport.clone());
    llm.retries = 2;
    llm.backoff_base_s = 0.0;
    llm.rate_limit_backoff_s = 0.0;
    llm
}

// ---------- text() ----------

#[test]
fn text_uses_isolated_options() {
    let fake = Scripted::new(vec![Ok(vec![assistant("draft"), result("hello")])]);
    assert_eq!(block_on(llm(None, &fake).text("Say hi", "Be terse.", "haiku", "t.basic")).unwrap(), "hello");
    let (prompt, opts) = &fake.calls()[0];
    assert_eq!(prompt, "Say hi");
    assert_eq!(opts.system_prompt, "Be terse.");
    assert_eq!(opts.model, "haiku");
    assert_eq!(opts.max_turns, 1);
    assert_eq!(opts.cli_path.as_deref(), Some("/bin/claude-test"));
    assert!(opts.json_schema.is_none());
    let argv = opts.argv("/bin/claude-test");
    let at = argv.iter().position(|a| a == "--tools").unwrap();
    assert_eq!(argv[at + 1], ""); // no tools at all
    assert!(argv.contains(&"--strict-mcp-config".to_string()) && argv.contains(&"--setting-sources=".to_string()));
}

#[test]
fn thinking_is_off_except_for_think_purposes() {
    let fake = Scripted::new(vec![
        Ok(vec![result("a")]),
        Ok(vec![result("b")]),
        Ok(vec![structured(json!({"color": "red", "n": 1}))]),
    ]);
    let client = llm(None, &fake).with_think_purposes(&["report."]);
    block_on(client.text("x", "s", "haiku", "engine.round")).unwrap();
    block_on(client.text("x", "s", "sonnet", "report.write")).unwrap();
    let _: Pick = block_on(json_with(&client, "x", "s", "haiku", "graph.extract", &pick_schema())).unwrap();
    let thinking: Vec<bool> = fake.calls().iter().map(|(_, o)| o.thinking_off).collect();
    assert_eq!(thinking, [true, false, true]);
}

#[test]
fn call_log_records_turns_and_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let mut msg = structured(json!({"color": "red", "n": 1}));
    msg["num_turns"] = json!(2);
    msg["duration_api_ms"] = json!(1500);
    msg["usage"] = json!({"output_tokens": 42});
    let fake = Scripted::new(vec![Ok(vec![msg])]);
    let _: Pick = block_on(json_with(
        &llm(Some(dir.path().join("calls.jsonl")), &fake),
        "x",
        "s",
        "haiku",
        "t.log",
        &pick_schema(),
    ))
    .unwrap();
    let text = std::fs::read_to_string(dir.path().join("calls.jsonl")).unwrap();
    let entry: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
    assert_eq!(
        (entry["turns"].clone(), entry["api_seconds"].clone(), entry["output_tokens"].clone()),
        (json!(2), json!(1.5), json!(42))
    );
    let keys: Vec<&String> = entry.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "ts",
            "purpose",
            "model",
            "mode",
            "seconds",
            "attempts",
            "ok",
            "error",
            "prompt_chars",
            "output_chars",
            "turns",
            "api_seconds",
            "startup_seconds",
            "output_tokens"
        ]
    );
    assert_eq!(entry["mode"], "structured");
}

#[test]
fn text_falls_back_to_assistant_blocks() {
    let fake = Scripted::new(vec![Ok(vec![assistant("part one"), assistant("part two"), result("")])]);
    assert_eq!(block_on(llm(None, &fake).text("x", "s", "sonnet", "t.blocks")).unwrap(), "part one\npart two");
}

#[test]
fn text_empty_reply_is_llm_error() {
    let fake = Scripted::new(vec![Ok(vec![result("   ")])]);
    let err = block_on(llm(None, &fake).text("x", "s", "haiku", "t.empty")).unwrap_err();
    assert!(err.0.contains("empty reply"), "{err}");
}

// ---------- json(): structured output ----------

#[test]
fn json_structured_output() {
    let fake = Scripted::new(vec![Ok(vec![structured(json!({"color": "teal", "n": 4}))])]);
    let pick: Pick =
        block_on(json_with(&llm(None, &fake), "Pick", "Sys", "haiku", "j.struct", &pick_schema())).unwrap();
    assert_eq!(pick, Pick { color: "teal".into(), n: 4 });
    let opts = &fake.calls()[0].1;
    assert_eq!(opts.json_schema.as_ref(), Some(&pick_schema().value));
    assert!(opts.max_turns > 1);
    assert!(opts.system_prompt.starts_with("Sys"));
}

#[test]
fn json_structured_output_as_string() {
    let fake = Scripted::new(vec![Ok(vec![structured(json!("{\"color\": \"red\", \"n\": 1}"))])]);
    let pick: Pick = block_on(json_with(&llm(None, &fake), "p", "s", "haiku", "j.str", &pick_schema())).unwrap();
    assert_eq!(pick, Pick { color: "red".into(), n: 1 });
}

#[test]
fn json_nested_schema_round_trips() {
    let report = json!({"title": "T", "summary": "S", "prediction": "P", "confidence": "medium",
                        "sections": [{"title": "a", "body_markdown": "b"}]});
    let fake = Scripted::new(vec![Ok(vec![structured(report)])]);
    let out: Report = block_on(tellfold_llm::json(&llm(None, &fake), "p", "s", "sonnet", "j.report")).unwrap();
    assert_eq!(out.sections[0].title, "a");
}

// ---------- json(): text extraction mode ----------

#[test]
fn json_text_mode_extracts_from_prose_and_fences() {
    let reply = "Sure! Here it is:\n```json\n{\"color\": \"blue\", \"n\": 7}\n```\nAnything else?";
    let fake = Scripted::new(vec![Ok(vec![result(reply)])]);
    let mut client = llm(None, &fake);
    client.structured_output = false;
    let pick: Pick = block_on(json_with(&client, "p", "s", "haiku", "j.text", &pick_schema())).unwrap();
    assert_eq!(pick, Pick { color: "blue".into(), n: 7 });
    let opts = &fake.calls()[0].1;
    assert!(opts.json_schema.is_none());
    assert!(opts.system_prompt.contains("\"color\"")); // the schema is spelled out when the CLI cannot enforce it
}

#[test]
fn json_repair_retry_after_invalid_output() {
    let fake = Scripted::new(vec![
        Ok(vec![structured(json!({"color": "green", "n": "many"}))]),
        Ok(vec![result("{\"color\": \"green\", \"n\": 3}")]),
    ]);
    let pick: Pick =
        block_on(json_with(&llm(None, &fake), "Pick one", "s", "haiku", "j.repair", &pick_schema())).unwrap();
    assert_eq!(pick, Pick { color: "green".into(), n: 3 });
    let (repair_prompt, repair_opts) = &fake.calls()[1];
    assert!(repair_prompt.starts_with("Pick one"));
    assert!(repair_prompt.contains("could not be used") && repair_prompt.contains("many"));
    assert!(repair_opts.json_schema.is_none());
}

#[test]
fn json_gives_up_after_one_repair() {
    let fake = Scripted::new(vec![Ok(vec![result("no json here")]), Ok(vec![result("{\"color\": 5}")])]);
    let err = block_on(json_with::<Pick>(&llm(None, &fake), "p", "s", "haiku", "j.bad", &pick_schema())).unwrap_err();
    assert!(err.0.contains("after repair"), "{err}");
    assert_eq!(fake.calls().len(), 2);
}

#[test]
fn json_non_api_error_result_goes_to_repair() {
    let fake = Scripted::new(vec![
        Ok(vec![error_result("structured output retries exhausted", None, None)]),
        Ok(vec![result("{\"color\": \"x\", \"n\": 0}")]),
    ]);
    let pick: Pick = block_on(json_with(&llm(None, &fake), "p", "s", "haiku", "j.maxturns", &pick_schema())).unwrap();
    assert_eq!(pick, Pick { color: "x".into(), n: 0 });
}

// ---------- retries, errors, timeout ----------

#[test]
fn retries_process_errors_then_succeeds() {
    let fake = Scripted::new(vec![failed("boom"), failed("boom"), Ok(vec![result("ok")])]);
    assert_eq!(block_on(llm(None, &fake).text("x", "s", "haiku", "r.ok")).unwrap(), "ok");
    assert_eq!(fake.calls().len(), 3);
}

#[test]
fn retries_exhausted_raise_llm_error() {
    let fake = Scripted::new(vec![failed("boom"), failed("boom"), failed("boom")]);
    let err = block_on(llm(None, &fake).text("x", "s", "haiku", "r.fail")).unwrap_err();
    assert!(err.0.contains("failed after 3 attempts"), "{err}");
}

#[test]
fn plain_stream_failure_exhausted_names_its_kind() {
    let reader = || Err(QueryError::Failed { kind: "RuntimeError".into(), message: "reader died".into() });
    let fake = Scripted::new(vec![reader(), reader(), reader()]);
    let err = block_on(llm(None, &fake).text("x", "s", "haiku", "r.plain")).unwrap_err();
    assert!(err.0.contains("RuntimeError: reader died"), "{err}");
}

#[test]
fn stream_without_result_is_retried() {
    let fake = Scripted::new(vec![Ok(vec![assistant("cut off")]), Ok(vec![result("ok")])]);
    assert_eq!(block_on(llm(None, &fake).text("x", "s", "haiku", "r.cut")).unwrap(), "ok");
    assert_eq!(fake.calls().len(), 2);
}

#[test]
fn rate_limit_uses_rate_limit_backoff() {
    let fake = Scripted::new(vec![
        Ok(vec![error_result("API Error: 429", Some(429), Some("api_error"))]),
        Ok(vec![result("ok")]),
    ]);
    let seen: Arc<Mutex<Vec<bool>>> = Arc::default();
    let mut client = llm(None, &fake);
    let record = seen.clone();
    client.backoff_hook = Some(Box::new(move |_, rate_limited| {
        record.lock().unwrap().push(rate_limited);
        0.0
    }));
    assert_eq!(block_on(client.text("x", "s", "haiku", "r.429")).unwrap(), "ok");
    assert_eq!(*seen.lock().unwrap(), [true]);
}

#[test]
fn overloaded_result_error_is_retried() {
    let overloaded = Err(QueryError::Result {
        text: "Overloaded".into(),
        api_error_status: Some(529),
        terminal_reason: Some("api_error".into()),
    });
    let fake = Scripted::new(vec![overloaded, Ok(vec![result("fine")])]);
    assert_eq!(block_on(llm(None, &fake).text("x", "s", "haiku", "r.529")).unwrap(), "fine");
    assert_eq!(fake.calls().len(), 2);
}

#[test]
fn auth_failure_is_not_retried() {
    let fake = Scripted::new(vec![Ok(vec![error_result(
        "Failed to authenticate: OAuth session expired",
        None,
        Some("api_error"),
    )])]);
    let err = block_on(llm(None, &fake).text("x", "s", "haiku", "r.auth")).unwrap_err();
    assert!(err.0.contains("/login"), "{err}");
    assert_eq!(fake.calls().len(), 1);
}

#[test]
fn missing_cli_is_not_retried() {
    let fake = Scripted::new(vec![Err(QueryError::NotFound("Claude Code not found".into()))]);
    let err = block_on(llm(None, &fake).text("x", "s", "haiku", "r.cli")).unwrap_err();
    assert!(err.0.contains("not found"), "{err}");
    assert_eq!(fake.calls().len(), 1);
}

struct Slow {
    active: AtomicUsize,
    peak: AtomicUsize,
    delay: Duration,
}

#[async_trait]
impl ClaudeTransport for Slow {
    async fn query(&self, prompt: &str, _options: &ClaudeOptions) -> Result<QueryOutput, QueryError> {
        let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(QueryOutput { messages: vec![result(prompt)], first_message: None })
    }
}

#[test]
fn timeout_per_call() {
    let slow = Arc::new(Slow { active: 0.into(), peak: 0.into(), delay: Duration::from_secs(10) });
    let mut client = ClaudeCode::new(4, None, None).with_transport(slow);
    client.timeout = Duration::from_millis(50);
    client.retries = 0;
    let err = block_on(client.text("x", "s", "haiku", "r.timeout")).unwrap_err();
    assert!(err.0.contains("timed out"), "{err}");
}

#[test]
fn concurrency_is_bounded() {
    let slow = Arc::new(Slow { active: 0.into(), peak: 0.into(), delay: Duration::from_millis(20) });
    let client = Arc::new(ClaudeCode::new(2, None, None).with_transport(slow.clone()));
    block_on(async {
        let calls = (0..6).map(|i| {
            let client = client.clone();
            tokio::spawn(async move { client.text(&format!("p{i}"), "s", "haiku", "c").await })
        });
        for call in futures::future::join_all(calls).await {
            call.unwrap().unwrap();
        }
    });
    assert_eq!(slow.peak.load(Ordering::SeqCst), 2);
}

// ---------- call log ----------

#[test]
fn call_log_records_lengths_not_prompts() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("calls.jsonl");
    let fake = Scripted::new(vec![Ok(vec![result("answer")]), failed("x"), failed("x"), failed("x")]);
    let client = llm(Some(log.clone()), &fake);
    block_on(client.text("SECRET-PROMPT-TEXT", "SECRET-SYSTEM", "haiku", "log.ok")).unwrap();
    assert!(block_on(client.text("SECRET-PROMPT-TEXT", "s", "sonnet", "log.fail")).is_err());
    let raw = std::fs::read_to_string(&log).unwrap();
    assert!(!raw.contains("SECRET"));
    let entries: Vec<Value> = raw.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let (ok, failed) = (&entries[0], &entries[1]);
    assert_eq!(ok["ok"], json!(true));
    assert!(ok["error"].is_null());
    assert_eq!(ok["prompt_chars"], json!("SECRET-PROMPT-TEXT".len() + "SECRET-SYSTEM".len()));
    assert_eq!(ok["output_chars"], json!("answer".len()));
    assert_eq!(failed["ok"], json!(false));
    assert_eq!(failed["attempts"], json!(3));
    assert_eq!(failed["model"], "sonnet");
    assert!(failed["error"].as_str().unwrap().starts_with("LLMError: log.fail: failed after 3 attempts"));
}

// ---------- the real transport against a fake `claude` ----------

/// A `claude` stand-in: saves its arguments, stdin and working directory, then streams `stream` (one JSON message
/// per line) and exits with `code`.
fn fake_claude(dir: &std::path::Path, stream: &[Value], code: i32) -> String {
    let lines: String = stream.iter().map(|m| format!("{}\n", serde_json::to_string(m).unwrap())).collect();
    std::fs::write(dir.join("stream.jsonl"), lines).unwrap();
    let body = format!(
        "printf '%s\\n' \"$@\" > '{d}/argv.txt'\npwd -P > '{d}/cwd.txt'\ncat > '{d}/stdin.txt'\n\
         env > '{d}/env.txt'\ncat '{d}/stream.jsonl'\necho 'oops' >&2\nexit {code}",
        d = dir.display()
    );
    make_script(dir, "claude", &body).to_string_lossy().into_owned()
}

#[test]
fn cli_transport_streams_one_verbatim_message_and_reads_the_result() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().canonicalize().unwrap();
    let mut done = result("hello there");
    done["usage"] = json!({"output_tokens": 3});
    let cli = fake_claude(&dir, &[json!({"type": "system", "subtype": "init"}), assistant("hello"), done], 0);
    let client = ClaudeCode::isolated(1, Some(dir.join("calls.jsonl")), Some(cli));
    let prompt = "Seed text with @/etc/hosts and /command";
    assert_eq!(block_on(client.text(prompt, "Be terse.", "haiku", "engine.round")).unwrap(), "hello there");

    let stdin: Value = serde_json::from_str(std::fs::read_to_string(dir.join("stdin.txt")).unwrap().trim()).unwrap();
    assert_eq!(stdin["type"], "user");
    assert_eq!(stdin["message"], json!({"role": "user", "content": prompt}));
    assert_eq!(stdin["client_composed"], json!(true));
    let argv = std::fs::read_to_string(dir.join("argv.txt")).unwrap();
    assert!(argv.contains("--input-format\nstream-json\n") && argv.contains("--thinking\ndisabled\n"));
    let cwd = std::fs::read_to_string(dir.join("cwd.txt")).unwrap();
    assert!(cwd.trim().ends_with("tellfold-runtime-cwd"), "{cwd}");
    let env = std::fs::read_to_string(dir.join("env.txt")).unwrap();
    assert!(env.lines().any(|l| l == "ANTHROPIC_API_KEY="), "secrets are blanked, not inherited");
    let entry: Value = serde_json::from_str(std::fs::read_to_string(dir.join("calls.jsonl")).unwrap().trim()).unwrap();
    assert_eq!(entry["output_tokens"], json!(3));
    assert_eq!(entry["ok"], json!(true));
}

#[test]
fn cli_transport_failures_are_retried_then_reported() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().canonicalize().unwrap();
    let cli = fake_claude(&dir, &[], 1);
    let mut client = ClaudeCode::isolated(1, None, Some(cli));
    client.retries = 1;
    client.backoff_base_s = 0.0;
    let err = block_on(client.text("x", "s", "haiku", "r.exit")).unwrap_err();
    assert!(err.0.contains("failed after 2 attempts: ProcessError: Command failed with exit code 1"), "{err}");

    let missing = ClaudeCode::isolated(1, None, Some(dir.join("nope").to_string_lossy().into_owned()));
    let err = block_on(missing.text("x", "s", "haiku", "r.missing")).unwrap_err();
    assert!(err.0.starts_with("Claude Code CLI not found"), "{err}");
    let unset = ClaudeCode::isolated(1, None, None);
    assert!(block_on(unset.text("x", "s", "haiku", "r.unset")).unwrap_err().0.contains("not found"));
}

#[test]
fn cli_transport_error_results_are_classified() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().canonicalize().unwrap();
    let cli = fake_claude(&dir, &[error_result("Not logged in · Please run /login", None, None)], 1);
    let client = ClaudeCode::isolated(1, None, Some(cli));
    let err = block_on(client.text("x", "s", "haiku", "r.login")).unwrap_err();
    assert!(err.0.starts_with("Claude Code is not logged in"), "{err}");
}
