//! Port of the AdapterLLM tests in tests/test_runtimes.py and the retry/repair/log tests in tests/test_llm.py,
//! run against a scripted adapter (no process, no network).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tellfold_llm::adapter::{Adapter, AdapterError, AdapterLlm, AdapterReply, AdapterRequest};
use tellfold_llm::{json_with, Llm, LlmError, Schema};
use tellfold_models::lax;

#[derive(Debug, PartialEq, Deserialize)]
struct Pick {
    color: String,
    #[serde(deserialize_with = "lax::int")]
    n: i64,
}

fn pick_schema() -> Schema {
    Schema::from_value::<Pick>(json!({
        "properties": {"color": {"title": "Color", "type": "string"}, "n": {"title": "N", "type": "integer"}},
        "required": ["color", "n"], "title": "Pick", "type": "object"
    }))
}

enum Step {
    Reply(AdapterReply),
    Fail(AdapterError),
    Slow(Duration, AdapterReply),
}

fn text(s: &str) -> Step {
    Step::Reply(AdapterReply::text(s))
}

#[derive(Default)]
struct Scripted {
    script: Mutex<VecDeque<Step>>,
    requests: Mutex<Vec<AdapterRequest>>,
    active: AtomicUsize,
    peak: AtomicUsize,
}

impl Scripted {
    fn new(steps: Vec<Step>) -> Self {
        Scripted { script: Mutex::new(steps.into()), ..Default::default() }
    }
}

#[async_trait]
impl Adapter for Scripted {
    fn runtime_id(&self) -> &str {
        "scripted"
    }

    async fn send(&self, request: &AdapterRequest) -> Result<AdapterReply, AdapterError> {
        self.requests.lock().unwrap().push(request.clone());
        let step = self.script.lock().unwrap().pop_front().expect("script has a step for every call");
        match step {
            Step::Reply(reply) => Ok(reply),
            Step::Fail(e) => Err(e),
            Step::Slow(wait, reply) => {
                let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(wait).await;
                self.active.fetch_sub(1, Ordering::SeqCst);
                Ok(reply)
            }
        }
    }
}

fn llm(steps: Vec<Step>, log: Option<std::path::PathBuf>) -> (AdapterLlm<Arc<Scripted>>, Arc<Scripted>) {
    let scripted = Arc::new(Scripted::new(steps));
    (AdapterLlm::new(scripted.clone(), 4, log).without_backoff(), scripted)
}

fn log_lines(path: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(path).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

#[tokio::test]
async fn text_retries_transient_then_succeeds_and_logs_no_prompt() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("calls.jsonl");
    let (llm, _) = llm(
        vec![
            Step::Fail(AdapterError::transient("boom")),
            Step::Fail(AdapterError::rate_limited("slow")),
            text("  hi  "),
        ],
        Some(log.clone()),
    );
    assert_eq!(llm.text("SECRET PROMPT TEXT", "sys", "m", "chat.ask").await.unwrap(), "hi");
    let entry = &log_lines(&log)[0];
    assert_eq!(
        (entry["ok"].clone(), entry["attempts"].clone(), entry["runtime"].clone()),
        (json!(true), json!(3), json!("scripted"))
    );
    assert_eq!(entry["prompt_chars"], json!("SECRET PROMPT TEXT".len() + 3));
    assert_eq!(entry["output_chars"], json!(2));
    assert!(!std::fs::read_to_string(&log).unwrap().contains("SECRET PROMPT"));
    let keys: Vec<&String> = entry.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "ts",
            "purpose",
            "model",
            "runtime",
            "mode",
            "seconds",
            "attempts",
            "ok",
            "error",
            "prompt_chars",
            "output_chars",
            "output_tokens"
        ]
    );
    assert!(entry["ts"].as_str().unwrap().ends_with("+00:00"));
}

#[tokio::test]
async fn fatal_is_not_retried_and_only_llm_errors_escape() {
    let (l, s) = llm(vec![Step::Fail(AdapterError::Fatal("not logged in".into())), text("never")], None);
    assert_eq!(l.text("p", "s", "m", "x").await.unwrap_err().0, "x: scripted: not logged in");
    assert_eq!(s.requests.lock().unwrap().len(), 1);

    let bug = AdapterError::Other { kind: "KeyError".into(), message: "'adapter bug'".into() };
    let (l, _) = llm(vec![Step::Fail(bug)], None);
    assert_eq!(l.text("p", "s", "m", "x").await.unwrap_err().0, "x: scripted: KeyError: 'adapter bug'");

    let (l, _) = llm((0..3).map(|i| Step::Fail(AdapterError::transient(format!("t{i}")))).collect(), None);
    assert_eq!(l.text("p", "s", "m", "x").await.unwrap_err().0, "x: scripted failed after 3 attempts: t2");

    let (l, _) = llm(vec![text("   ")], None);
    assert_eq!(l.text("p", "s", "m", "x").await.unwrap_err().0, "x: unusable reply from scripted: empty reply");

    let (l, _) =
        llm(vec![Step::Reply(AdapterReply { structured: Some(json!({"a": "é"})), ..Default::default() })], None);
    assert_eq!(l.text("p", "s", "m", "x").await.unwrap(), r#"{"a": "é"}"#);
}

#[tokio::test]
async fn json_validates_repairs_once_then_fails() {
    let schema = pick_schema();
    let (l, s) = llm(vec![text("Sure! ```json\n{\"color\": \"teal\", \"n\": 3}\n```")], None);
    let pick: Pick = json_with(&l, "p", "s", "m", "x", &schema).await.unwrap();
    assert_eq!(pick, Pick { color: "teal".into(), n: 3 });
    let first = s.requests.lock().unwrap()[0].clone();
    assert!(first.system.contains("JSON Schema") && first.json_schema.is_some());
    assert_eq!(first.schema_name, "Pick");

    let (l, s) = llm(vec![text(r#"{"color": "teal"}"#), text(r#"{"color": "teal", "n": "1"}"#)], None);
    let pick: Pick = json_with(&l, "p", "s", "m", "x", &schema).await.unwrap();
    assert_eq!(pick.n, 1);
    let repair = s.requests.lock().unwrap()[1].prompt.clone();
    assert!(repair.starts_with("p\n\n---\n") && repair.contains("could not be used"));
    assert!(repair.contains(r#"{"color": "teal"}"#) && repair.contains("missing field `n`"));

    let (l, s) = llm(vec![text("no json"), text("still none"), text("unused")], None);
    let err = json_with::<Pick>(&l, "p", "s", "m", "x", &schema).await.unwrap_err();
    assert_eq!(err.0, "x: invalid JSON after repair: no JSON value found in the reply");
    assert_eq!(s.requests.lock().unwrap().len(), 2);

    let structured = AdapterReply { structured: Some(json!({"color": "red", "n": 2})), ..Default::default() };
    let (l, _) = llm(vec![Step::Reply(structured)], None);
    assert_eq!(json_with::<Pick>(&l, "p", "s", "m", "x", &schema).await.unwrap().color, "red");

    // structured output given as a string is parsed like text
    let as_string = AdapterReply { structured: Some(json!(r#"{"color": "x", "n": 0}"#)), ..Default::default() };
    let (l, _) = llm(vec![Step::Reply(as_string)], None);
    assert_eq!(json_with::<Pick>(&l, "p", "s", "m", "x", &schema).await.unwrap().n, 0);

    // a bad answer from the adapter itself goes to repair too
    let (l, _) =
        llm(vec![Step::Fail(AdapterError::bad_output("max turns", "")), text(r#"{"color": "x", "n": 0}"#)], None);
    assert!(json_with::<Pick>(&l, "p", "s", "m", "x", &schema).await.is_ok());
}

#[tokio::test]
async fn rate_limits_use_the_rate_limit_backoff() {
    let scripted = Arc::new(Scripted::new(vec![Step::Fail(AdapterError::rate_limited("429")), text("ok")]));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut l = AdapterLlm::new(scripted, 2, None);
    let record = seen.clone();
    l.backoff_hook = Some(Box::new(move |attempt, rate_limited| {
        record.lock().unwrap().push((attempt, rate_limited));
        0.0
    }));
    assert_eq!(l.text("x", "s", "haiku", "r.429").await.unwrap(), "ok");
    assert_eq!(*seen.lock().unwrap(), [(0, true)]);
}

#[tokio::test]
async fn timeout_per_attempt() {
    let (l, _) = llm(vec![Step::Slow(Duration::from_secs(5), AdapterReply::text("late"))], None);
    let l = l.with_timeout(Duration::from_millis(50)).with_retries(0);
    assert_eq!(
        l.text("p", "s", "m", "x").await.unwrap_err().0,
        "x: scripted failed after 1 attempts: timed out after 0s"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrency_is_bounded() {
    let scripted = Arc::new(Scripted::new(
        (0..6).map(|_| Step::Slow(Duration::from_millis(30), AdapterReply::text("ok"))).collect(),
    ));
    let l = Arc::new(AdapterLlm::new(scripted.clone(), 2, None));
    let tasks: Vec<_> = (0..6)
        .map(|i| {
            let l = l.clone();
            tokio::spawn(async move { l.text(&format!("p{i}"), "s", "m", "c").await })
        })
        .collect();
    for t in tasks {
        t.await.unwrap().unwrap();
    }
    assert_eq!(scripted.peak.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn call_log_records_failures_and_cancellations() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("calls.jsonl");
    let mut steps = vec![text("answer")];
    steps.extend((0..3).map(|_| Step::Fail(AdapterError::transient("x"))));
    steps.push(Step::Slow(Duration::from_secs(5), AdapterReply::text("never")));
    let (l, _) = llm(steps, Some(log.clone()));
    l.text("SECRET-PROMPT-TEXT", "SECRET-SYSTEM", "haiku", "log.ok").await.unwrap();
    let err: LlmError = l.text("SECRET-PROMPT-TEXT", "s", "sonnet", "log.fail").await.unwrap_err();
    let cancelled = tokio::time::timeout(Duration::from_millis(30), l.text("p", "s", "m", "log.cancel")).await;
    assert!(cancelled.is_err());

    let raw = std::fs::read_to_string(&log).unwrap();
    assert!(!raw.contains("SECRET"));
    let lines = log_lines(&log);
    let (ok, failed, gone) = (&lines[0], &lines[1], &lines[2]);
    assert_eq!((ok["ok"].clone(), ok["error"].clone()), (json!(true), Value::Null));
    assert_eq!(ok["prompt_chars"], json!("SECRET-PROMPT-TEXT".len() + "SECRET-SYSTEM".len()));
    assert_eq!(ok["output_chars"], json!("answer".len()));
    assert_eq!(
        (failed["ok"].clone(), failed["attempts"].clone(), failed["model"].clone()),
        (json!(false), json!(3), json!("sonnet"))
    );
    assert_eq!(failed["error"], json!(format!("LLMError: {}", err.0)));
    assert_eq!((gone["purpose"].clone(), gone["error"].clone()), (json!("log.cancel"), json!("CancelledError: ")));
}
