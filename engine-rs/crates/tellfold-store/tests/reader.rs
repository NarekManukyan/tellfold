//! Port of the RunReader tests in tests/test_server.py: calls by role, derived states, tolerance of partial runs.

use std::fs;
use std::path::Path;

use tellfold_models::{Action, ActionType, Persona, RunStage, RunState, RunStatus, SimConfig};
use tellfold_store::files::{now_iso, write_json, write_json_list, write_status};
use tellfold_store::reader::pid_alive;
use tellfold_store::{RunReader, World};

#[test]
fn reader_splits_calls_by_role() {
    let tmp = tempfile::tempdir().unwrap();
    let lines = [
        r#"{"purpose": "graph.extract", "ok": true, "seconds": 10.5}"#,
        r#"{"purpose": "personas.batch", "ok": false, "seconds": 2}"#,
        r#"{"purpose": "engine.round", "ok": true, "seconds": 3.25}"#,
        r#"{"purpose": "engine.sentiment", "ok": true, "seconds": 1}"#,
        r#"{"purpose": "chat.interview", "ok": true, "seconds": null}"#,
        r#"{"purpose": "report.write", "ok": true, "seconds": 20}"#,
    ];
    fs::write(tmp.path().join("calls.jsonl"), lines.join("\n") + "\n{\"purpose\": \"engine.ro").unwrap();
    let calls = RunReader::new(tmp.path(), None, false).calls();
    assert_eq!((calls.count, calls.failed, calls.seconds), (6, 1, 36.75));
    assert_eq!((calls.agent.count, calls.agent.failed, calls.agent.seconds), (3, 0, 4.25));
    assert_eq!((calls.planner.count, calls.planner.failed, calls.planner.seconds), (3, 1, 32.5));
    let empty = RunReader::new(&tmp.path().join("missing"), None, false).calls();
    assert_eq!((empty.count, empty.agent.count), (0, 0));
}

fn running_status(pid: Option<i64>) -> RunStatus {
    RunStatus {
        state: RunState::Running,
        stage: RunStage::Rounds,
        round: 0,
        total_rounds: 2,
        detail: String::new(),
        error: None,
        pid,
        started_at: now_iso(),
        updated_at: now_iso(),
        failed_batches: vec![],
    }
}

fn make_external_run(runs: &Path, id: &str, with_report: bool, status: Option<RunStatus>) -> RunReader {
    let dir = runs.join(id);
    fs::create_dir_all(&dir).unwrap();
    let mut config = SimConfig::new("q?", vec![]);
    config.rounds = 2;
    write_json(&dir.join("config.json"), &config).unwrap();
    let people =
        vec![Persona::new("a01", "ann", "Ann", "b", "bg", "s"), Persona::new("a02", "bob", "Bob", "b", "bg", "s")];
    write_json_list(&dir.join("personas.json"), &people).unwrap();
    let world = World::open(&dir.join("world.db"), 7).unwrap();
    world.add_personas(&people).unwrap();
    world.apply(0, &Action::new("a01", ActionType::Post).content("hello")).unwrap();
    world.close().unwrap();
    if with_report {
        fs::write(dir.join("report.json"), r#"{"title": "t", "summary": "s", "prediction": "p", "confidence": "low"}"#)
            .unwrap();
    }
    if let Some(status) = status {
        write_status(&dir, &status).unwrap();
    }
    RunReader::new(&dir, None, false)
}

fn dead_pid() -> i64 {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id() as i64;
    child.wait().unwrap();
    pid
}

#[test]
fn older_and_stale_runs_get_a_derived_state() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path();
    let parent = std::os::unix::process::parent_id() as i64;
    let cases = [
        (make_external_run(runs, "20260101-000001-old-done", true, None), "done", "finished"),
        (make_external_run(runs, "20260101-000002-old-partial", false, None), "stopped", "personas"),
        (
            make_external_run(runs, "20260101-000003-dead-cli", false, Some(running_status(Some(dead_pid())))),
            "stopped",
            "rounds",
        ),
        (
            make_external_run(runs, "20260101-000004-alive-cli", false, Some(running_status(Some(parent)))),
            "running",
            "rounds",
        ),
        (
            make_external_run(
                runs,
                "20260101-000005-ours",
                false,
                Some(running_status(Some(std::process::id() as i64))),
            ),
            "stopped",
            "rounds",
        ),
    ];
    for (reader, state, stage) in &cases {
        let summary = reader.summary(false).unwrap();
        assert_eq!((summary.state.as_str(), summary.stage.as_str()), (*state, *stage), "{}", reader.id);
    }
    let partial = cases[1].0.detail(false).unwrap();
    let status = partial.status.unwrap();
    assert_eq!((status.state, status.detail.as_str()), (RunState::Stopped, "no status file (older run)"));
    assert!(partial.report.is_none() && !partial.live);
    // a live run of this server is running whatever its status file says
    assert_eq!(cases[4].0.status(true).unwrap().state, RunState::Running);
    assert!(RunReader::new(&runs.join("not-a-run"), None, false).summary(false).is_none());
}

#[test]
fn half_written_runs_read_as_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("r");
    fs::create_dir_all(&dir).unwrap();
    let reader = RunReader::new(&dir, None, false);
    assert!(!reader.is_run());
    assert!(reader.posts(0).is_empty() && reader.round_stats().is_empty() && reader.personas().is_empty());
    assert_eq!(reader.max_post_id(), 0);
    fs::write(dir.join("config.json"), r#"{"question": "q", "seed_files": []}"#).unwrap();
    fs::write(dir.join("world.db"), "not a database").unwrap();
    fs::write(dir.join("report.json"), r#"{"title": "#).unwrap();
    let detail = reader.detail(false).unwrap();
    assert!(detail.report.is_none() && detail.stats.is_empty() && detail.follows.is_empty());
    assert_eq!(detail.status.unwrap().state, RunState::Done); // report.json exists, even half written
}

#[test]
fn personas_fall_back_to_the_database() {
    let tmp = tempfile::tempdir().unwrap();
    let reader = make_external_run(tmp.path(), "r", false, None);
    fs::remove_file(reader.run_dir.join("personas.json")).unwrap();
    let reader = RunReader::new(&reader.run_dir, None, false);
    assert_eq!(reader.personas().len(), 2);
    let posts = reader.posts(0);
    assert_eq!((posts[0].author_handle.as_str(), posts[0].sentiment), ("ann", None));
    let agent = reader.agent("a01").unwrap();
    assert_eq!(agent.posts.len(), 1);
    assert!(agent.memory.is_empty() && agent.sentiment_by_round.is_empty());
}

#[test]
fn pids() {
    assert!(pid_alive(Some(std::process::id() as i64)));
    assert!(!pid_alive(None) && !pid_alive(Some(0)) && !pid_alive(Some(-4)));
    assert!(!pid_alive(Some(dead_pid())));
}
