//! Port of `tests/test_runner.py` (the CLI tests move with the CLI; the report stage is a stub until the report
//! writer is ported).

use std::cell::RefCell;
use std::future::{poll_fn, Future};
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;

use async_trait::async_trait;
use futures::FutureExt;
use serde_json::{json, Value};
use tellfold_engine::runner::{mark_stopped, run_pipeline, PipelineError, ReportStage};
use tellfold_llm::canned::fake_responder;
use tellfold_llm::fake::{FakeLlm, FakeReply};
use tellfold_llm::{Llm, LlmError, Schema};
use tellfold_models::{GodEvent, KnowledgeGraph, Report, RunStage, RunState, RunStatus, SimConfig};
use tellfold_store::files::{read_status, write_status};
use tellfold_store::world::World;

fn example() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/mossbook_pricing.md")
}

fn make_config() -> SimConfig {
    SimConfig {
        num_agents: 6,
        rounds: 3,
        batch_size: 3,
        ..SimConfig::new("Will buyers accept the subscription?", vec![example().to_string_lossy().into()])
    }
}

fn fake(fail_purpose: Option<&'static str>) -> FakeLlm {
    FakeLlm::new(move |purpose: &str, prompt: &str, schema: Option<&Schema>| -> FakeReply {
        if fail_purpose.is_some_and(|f| purpose.starts_with(f)) {
            return LlmError(format!("simulated {purpose} failure")).into();
        }
        fake_responder(purpose, prompt, schema)
    })
}

/// A fake whose every call yields once, so a run can be interrupted between calls.
fn yielding_fake() -> FakeLlm {
    FakeLlm::new_async(|purpose, prompt, schema| {
        let reply = fake_responder(purpose, prompt, schema);
        async move {
            tokio::task::yield_now().await;
            reply
        }
        .boxed()
    })
}

/// The report stage until the report writer is ported: writes report.json, or fails like `report.write` would.
struct StubReport {
    fail: bool,
}

#[async_trait(?Send)]
impl ReportStage for StubReport {
    async fn write_report(
        &self,
        llm: &dyn Llm,
        run_dir: &Path,
        _: &World,
        _: &KnowledgeGraph,
        config: &SimConfig,
    ) -> Result<Report, PipelineError> {
        if self.fail {
            llm.text("write it", "system", &config.planner_model, "report.write").await?;
            return Err(LlmError("report.write: simulated report.write failure".into()).into());
        }
        let report: Report =
            serde_json::from_value(json!({"title": "t", "summary": "s", "prediction": "p", "confidence": "low"}))
                .unwrap();
        std::fs::write(run_dir.join("report.json"), serde_json::to_string(&report).unwrap())?;
        Ok(report)
    }
}

const REPORT: StubReport = StubReport { fail: false };

#[derive(Default)]
struct Recorder {
    events: RefCell<Vec<(String, Value)>>,
}

impl Recorder {
    fn record(&self, name: &str, payload: &Value) {
        assert!(payload.is_object()); // everything must be forwardable over SSE as is
        self.events.borrow_mut().push((name.to_string(), payload.clone()));
    }
    fn names(&self) -> Vec<String> {
        self.events.borrow().iter().map(|(n, _)| n.clone()).collect()
    }
    fn of(&self, name: &str) -> Vec<Value> {
        self.events.borrow().iter().filter(|(n, _)| n == name).map(|(_, p)| p.clone()).collect()
    }
}

fn run_dir() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("runs").join("20260101-000000-test");
    std::fs::create_dir_all(&path).unwrap();
    (tmp, path)
}

fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(future)
}

#[test]
fn status_transitions_through_every_stage() {
    let (_tmp, run_dir) = run_dir();
    let rec = Recorder::default();
    let on_event = |n: &str, p: &Value| rec.record(n, p);
    let report = block_on(run_pipeline(
        &fake(None),
        &run_dir,
        &make_config(),
        &[example()],
        Some(&on_event),
        None,
        None,
        &REPORT,
    ));
    assert!(report.unwrap().is_some());

    let statuses: Vec<RunStatus> = rec.of("status").into_iter().map(|s| serde_json::from_value(s).unwrap()).collect();
    let mut stages: Vec<RunStage> = Vec::new();
    for s in &statuses {
        if !stages.contains(&s.stage) {
            stages.push(s.stage);
        }
    }
    use RunStage::*;
    assert_eq!(stages, [Reading, Graph, Personas, Seeding, Rounds, Report, Finished]);
    assert!(statuses[..statuses.len() - 1].iter().all(|s| s.state == RunState::Running));
    let rounds: Vec<i64> =
        statuses.iter().filter(|s| s.detail.ends_with("done") && s.detail.contains("round")).map(|s| s.round).collect();
    assert_eq!(rounds, [1, 2, 3]);

    let last = statuses.last().unwrap();
    let final_status = read_status(&run_dir).unwrap();
    assert_eq!(&final_status, last);
    assert_eq!(
        (final_status.state, final_status.stage, final_status.round, final_status.total_rounds),
        (RunState::Done, RunStage::Finished, 3, 3)
    );
    assert_eq!(final_status.pid, Some(i64::from(std::process::id())));
    assert!(final_status.error.is_none());
    assert_eq!(final_status.started_at, statuses[0].started_at);
    assert!(final_status.updated_at >= final_status.started_at);

    let stage_events = rec.of("stage");
    let values = ["reading", "graph", "personas", "seeding", "rounds", "report", "finished"];
    assert!(stage_events.iter().all(|e| values.contains(&e["stage"].as_str().unwrap())));
    assert!(stage_events.iter().any(|e| e.get("step") == Some(&json!("follow_graph")))); // engine steps become SEEDING details
    let names = rec.names();
    assert_eq!(names.last().unwrap(), "run_done");
    assert!(!names.contains(&"run_failed".into()) && !names.contains(&"run_stopped".into()));
    assert_eq!(names.iter().filter(|n| *n == "round_start").count(), 3);
    assert_eq!(names.iter().filter(|n| *n == "round_done").count(), 4); // round 0 + 3 rounds
    let index = |name: &str| names.iter().position(|n| n == name).unwrap();
    assert!(index("posts_added") < index("round_done"));
    for name in ["config.json", "graph.json", "personas.json", "world.db", "report.json"] {
        assert!(run_dir.join(name).exists(), "{name}");
    }
    let leftovers = std::fs::read_dir(&run_dir)
        .unwrap()
        .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with(".status-"));
    assert_eq!(leftovers.count(), 0); // atomic writes leave no temp files
}

#[test]
fn stop_between_rounds_still_writes_report() {
    let (_tmp, run_dir) = run_dir();
    let stop = AtomicBool::new(false);
    let rec = Recorder::default();
    let on_event = |n: &str, p: &Value| {
        rec.record(n, p);
        if n == "round_start" && p["round"] == 2 {
            stop.store(true, Ordering::SeqCst); // set mid-round: round 2 must still finish
        }
    };
    let config = SimConfig { rounds: 5, ..make_config() };
    let report = block_on(run_pipeline(
        &fake(None),
        &run_dir,
        &config,
        &[example()],
        Some(&on_event),
        Some(&stop),
        None,
        &REPORT,
    ));
    assert!(report.unwrap().is_some() && run_dir.join("report.json").exists());
    let status = read_status(&run_dir).unwrap();
    assert_eq!((status.state, status.stage, status.round), (RunState::Stopped, RunStage::Finished, 2));
    assert_eq!(
        rec.of("round_done").iter().map(|p| p["round"].clone()).collect::<Vec<_>>(),
        [json!(0), json!(1), json!(2)]
    );
    let names = rec.names();
    assert_eq!(names.last().unwrap(), "run_stopped");
    assert!(!names.contains(&"run_done".into()));
    let world = World::open(&run_dir.join("world.db"), 7).unwrap();
    assert_eq!(world.actions(None).unwrap().iter().map(|(r, _)| *r).max(), Some(2));
}

#[test]
fn stop_before_any_round_returns_none() {
    let (_tmp, run_dir) = run_dir();
    let stop = AtomicBool::new(false);
    let rec = Recorder::default();
    let on_event = |n: &str, p: &Value| {
        rec.record(n, p);
        if n == "stage" && p["stage"] == "seeding" {
            stop.store(true, Ordering::SeqCst);
        }
    };
    let result = block_on(run_pipeline(
        &fake(None),
        &run_dir,
        &make_config(),
        &[example()],
        Some(&on_event),
        Some(&stop),
        None,
        &REPORT,
    ));
    assert!(result.unwrap().is_none() && !run_dir.join("report.json").exists());
    let status = read_status(&run_dir).unwrap();
    assert_eq!((status.state, status.round), (RunState::Stopped, 0));
    let names = rec.names();
    assert_eq!(names.last().unwrap(), "run_stopped");
    assert!(!names.contains(&"round_start".into()));
}

#[test]
fn event_injected_mid_run() {
    let (_tmp, run_dir) = run_dir();
    let config = SimConfig { events: vec![GodEvent::new(3, "Planned news")], ..make_config() };
    let llm = fake(None);
    let rec = Recorder::default();
    let on_event = |n: &str, p: &Value| {
        rec.record(n, p);
        if n == "round_done" && p["round"] == 1 {
            let ui = World::open(&run_dir.join("world.db"), 7).unwrap(); // a second connection, like the server's
            ui.add_event(&GodEvent::new(2, "A rival halves its price")).unwrap();
            ui.close().unwrap();
        }
    };
    block_on(run_pipeline(&llm, &run_dir, &config, &[example()], Some(&on_event), None, None, &REPORT)).unwrap();

    assert_eq!(rec.of("event_injected"), [json!({"round": 2, "content": "A rival halves its price"})]);
    let calls = llm.calls.lock().unwrap().clone();
    let round_prompts: Vec<&String> = calls.iter().filter(|c| c.0 == "engine.round").map(|c| &c.2).collect();
    let first_batches = rec.of("round_start")[0]["batches"].as_u64().unwrap() as usize;
    let round2 = round_prompts[first_batches];
    assert!(round2.contains("BREAKING NEWS") && round2.contains("A rival halves its price"));
    let world = World::open(&run_dir.join("world.db"), 7).unwrap();
    let contents: std::collections::HashSet<String> = world.events().unwrap().into_iter().map(|e| e.content).collect();
    assert_eq!(contents, ["Planned news".to_string(), "A rival halves its price".to_string()].into());
}

#[test]
fn per_post_sentiment_stored() {
    let (_tmp, run_dir) = run_dir();
    let config = SimConfig { rounds: 2, ..make_config() };
    block_on(run_pipeline(&fake(None), &run_dir, &config, &[example()], None, None, None, &REPORT)).unwrap();
    let world = World::open(&run_dir.join("world.db"), 7).unwrap();
    let scores = world.post_sentiments().unwrap();
    let posts = world.posts(None).unwrap();
    assert!(!scores.is_empty() && scores.keys().all(|id| posts.iter().any(|p| p.id == *id))); // a subset is scored
    let by_round: std::collections::HashSet<i64> =
        posts.iter().filter(|p| scores.contains_key(&p.id)).map(|p| p.round).collect();
    assert!(by_round.contains(&0) && (by_round.contains(&1) || by_round.contains(&2)));
    assert!(scores.values().all(|v| (-1.0..=1.0).contains(v)));
}

#[test]
fn failure_marks_run_failed_and_returns_the_error() {
    let (_tmp, run_dir) = run_dir();
    let rec = Recorder::default();
    let on_event = |n: &str, p: &Value| rec.record(n, p);
    let config = SimConfig { rounds: 1, ..make_config() };
    let err = block_on(run_pipeline(
        &fake(Some("report")),
        &run_dir,
        &config,
        &[example()],
        Some(&on_event),
        None,
        None,
        &StubReport { fail: true },
    ))
    .unwrap_err();
    assert!(err.to_string().contains("report.write"));
    let status = read_status(&run_dir).unwrap();
    assert_eq!(status.state, RunState::Failed);
    assert!(status.error.unwrap().contains("report.write"));
    assert_eq!((status.stage, status.round), (RunStage::Report, 1));
    assert_eq!(rec.names().last().unwrap(), "run_failed");
    assert!(rec.of("run_failed")[0]["error"].as_str().unwrap().contains("report.write"));
}

#[test]
fn failed_batches_persist_in_status() {
    // A failed batch is announced over SSE and also kept in status.json, so a reload still shows it.
    let (_tmp, run_dir) = run_dir();
    let rec = Recorder::default();
    let on_event = |n: &str, p: &Value| rec.record(n, p);
    let config = SimConfig { rounds: 2, ..make_config() };
    block_on(run_pipeline(
        &fake(Some("engine.round")),
        &run_dir,
        &config,
        &[example()],
        Some(&on_event),
        None,
        None,
        &REPORT,
    ))
    .unwrap();
    let announced = rec.of("batch_failed");
    assert!(!announced.is_empty(), "every round batch fails with this responder");
    let status = read_status(&run_dir).unwrap();
    assert_eq!(status.state, RunState::Done); // one failed batch never stops the run
    let kept: Vec<Value> = status
        .failed_batches
        .iter()
        .map(|b| json!([b.round, serde_json::to_value(&b.batch).unwrap(), b.agents]))
        .collect();
    let want: Vec<Value> = announced.iter().map(|p| json!([p["round"], p["batch"], p["agents"]])).collect();
    assert_eq!(kept, want);
    let rounds: std::collections::HashSet<i64> = status.failed_batches.iter().map(|b| b.round).collect();
    assert_eq!(rounds, [1, 2].into());
    assert!(status.failed_batches.iter().all(|b| b.error.contains("engine.round") && b.error.chars().count() <= 500));
    // the JSON on disk round-trips, and older status files without the field still load
    let mut raw: Value = serde_json::from_str(&std::fs::read_to_string(run_dir.join("status.json")).unwrap()).unwrap();
    assert!(!raw["failed_batches"][0]["agents"].as_array().unwrap().is_empty());
    raw.as_object_mut().unwrap().remove("failed_batches");
    assert!(serde_json::from_value::<RunStatus>(raw).unwrap().failed_batches.is_empty());
}

#[test]
fn unreadable_seeds_fail_with_run_error() {
    let (tmp, run_dir) = run_dir();
    let empty = tmp.path().join("empty.md");
    std::fs::write(&empty, "   \n").unwrap();
    let err =
        block_on(run_pipeline(&fake(None), &run_dir, &make_config(), &[empty], None, None, None, &REPORT)).unwrap_err();
    assert!(matches!(err, PipelineError::Run(_)), "{err:?}");
    assert_eq!(read_status(&run_dir).unwrap().state, RunState::Failed);
}

#[test]
fn cancellation_marks_run_stopped() {
    let (_tmp, run_dir) = run_dir();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let on_event = move |n: &str, p: &Value| {
        if n == "round_done" && p["round"] == 1 {
            flag.store(true, Ordering::SeqCst);
        }
    };
    let llm = yielding_fake();
    let config = make_config();
    let seeds = [example()];
    block_on(async {
        let mut run = pin!(run_pipeline(&llm, &run_dir, &config, &seeds, Some(&on_event), None, None, &REPORT));
        // drop the run (like a cancelled task) as soon as round 1 is done
        poll_fn(|cx| {
            if cancel.load(Ordering::SeqCst) {
                return Poll::Ready(());
            }
            match run.as_mut().poll(cx) {
                Poll::Ready(_) => panic!("the run finished before it was cancelled"),
                Poll::Pending => Poll::Pending,
            }
        })
        .await;
    });
    let status = read_status(&run_dir).unwrap();
    assert_eq!((status.state, status.round), (RunState::Stopped, 1));
}

#[test]
fn broken_listener_does_not_break_the_run() {
    let (_tmp, run_dir) = run_dir();
    let explode = |_: &str, _: &Value| panic!("listener bug");
    let config = SimConfig { rounds: 1, ..make_config() };
    let report =
        block_on(run_pipeline(&fake(None), &run_dir, &config, &[example()], Some(&explode), None, None, &REPORT));
    assert!(report.unwrap().is_some());
    assert_eq!(read_status(&run_dir).unwrap().state, RunState::Done);
}

#[test]
fn read_status_tolerates_missing_and_garbage() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(read_status(tmp.path()).is_none());
    std::fs::write(tmp.path().join("status.json"), "{\"state\": \"runn").unwrap();
    assert!(read_status(tmp.path()).is_none());
}

#[test]
fn mark_stopped_only_touches_live_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let live = RunStatus {
        state: RunState::Running,
        stage: RunStage::Rounds,
        round: 2,
        total_rounds: 4,
        detail: String::new(),
        error: None,
        pid: None,
        started_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        failed_batches: vec![],
    };
    write_status(tmp.path(), &live).unwrap();
    mark_stopped(tmp.path()).unwrap();
    let stopped = read_status(tmp.path()).unwrap();
    assert_eq!(stopped.state, RunState::Stopped);
    assert_eq!(stopped.detail, "stopped after round 2");
    write_status(tmp.path(), &RunStatus { state: RunState::Done, ..live }).unwrap();
    mark_stopped(tmp.path()).unwrap();
    assert_eq!(read_status(tmp.path()).unwrap().state, RunState::Done);
}

/// Records the system prompt of the first call of each purpose.
struct Spy {
    inner: FakeLlm,
    systems: Mutex<Vec<(String, String)>>,
}

#[async_trait]
impl Llm for Spy {
    async fn text(&self, prompt: &str, system: &str, model: &str, purpose: &str) -> Result<String, LlmError> {
        self.systems.lock().unwrap().push((purpose.into(), system.into()));
        self.inner.text(prompt, system, model, purpose).await
    }
    async fn json_value(
        &self,
        prompt: &str,
        system: &str,
        model: &str,
        purpose: &str,
        schema: &Schema,
    ) -> Result<Value, LlmError> {
        self.systems.lock().unwrap().push((purpose.into(), system.into()));
        self.inner.json_value(prompt, system, model, purpose, schema).await
    }
}

#[test]
fn language_reaches_every_writing_prompt() {
    // A French run asks for French in the persona, opening and round prompts (the report has its own tests).
    let (_tmp, run_dir) = run_dir();
    let llm = Spy { inner: fake(None), systems: Mutex::new(Vec::new()) };
    let config = SimConfig { language: "French".into(), ..make_config() };
    block_on(run_pipeline(&llm, &run_dir, &config, &[example()], None, None, None, &REPORT)).unwrap();
    let calls = llm.inner.calls.lock().unwrap().clone();
    let persona_prompts: Vec<&String> = calls.iter().filter(|c| c.0 == "personas.batch").map(|c| &c.2).collect();
    assert!(!persona_prompts.is_empty() && persona_prompts.iter().all(|p| p.contains("in French")));
    let systems = llm.systems.lock().unwrap().clone();
    assert!(systems.iter().any(|(purpose, s)| purpose.starts_with("engine.") && s.contains("in French")));
}
