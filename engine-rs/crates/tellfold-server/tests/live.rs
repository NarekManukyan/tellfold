//! Ports of `tests/test_server.py`: live-run control (event injection, stop, shutdown) with a gate that holds a run
//! inside round 1, the SSE stream, the watcher for runs written by another process, uploads and the local-only
//! guard. Every model call goes through the canned FakeLlm.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::FutureExt;
use serde_json::{json, Value};

use tellfold_llm::canned::fake_responder;
use tellfold_llm::fake::{FakeLlm, FakeReply};
use tellfold_llm::{Llm, LlmError};
use tellfold_models::{Action, ActionType, Persona, RoundStats, RunStage, RunState, RunStatus, SimConfig};
use tellfold_server::upload::safe_filename;
use tellfold_server::{create_app, AppOptions, LlmFactory, ManagerOptions, ManagerThread};
use tellfold_store::files::{now_iso, write_json, write_status};
use tellfold_store::World;

const SEED_TEXT: &str = "# Harbour Town votes on a new ferry fare\n\nThe council of Harbour Town proposes raising the \
ferry fare from 2 to 3 euros to fund a new electric ferry. The commuters' association opposes the rise; local shops \
support the electric ferry. The mayor, Ana Ruiz, will decide after a public consultation.\n\nStudents say the fare is \
already too high. The ferry operator says the old boat is unreliable.\n";
const TIMEOUT: Duration = Duration::from_secs(30);

/// Holds `engine.round` calls while closed.
#[derive(Default)]
struct Gate {
    closed: AtomicBool,
    waiting: AtomicBool,
}

struct Factory {
    gate: Arc<Gate>,
    purposes: Arc<Mutex<Vec<String>>>,
    fail_reports: Arc<AtomicBool>,
}

impl Factory {
    fn new() -> Self {
        Factory { gate: Arc::default(), purposes: Arc::default(), fail_reports: Arc::default() }
    }

    fn llm_factory(&self) -> LlmFactory {
        let (gate, purposes, fail) = (self.gate.clone(), self.purposes.clone(), self.fail_reports.clone());
        Arc::new(move |_, _, _| {
            let (gate, purposes, fail) = (gate.clone(), purposes.clone(), fail.clone());
            let llm = FakeLlm::new_async(move |purpose, prompt, schema| {
                purposes.lock().unwrap().push(purpose.to_string());
                let reply = if fail.load(Ordering::SeqCst) && purpose.starts_with("report.") {
                    FakeReply::Error(LlmError::new("the model is down"))
                } else {
                    fake_responder(purpose, prompt, schema)
                };
                let held = purpose == "engine.round";
                let gate = gate.clone();
                async move {
                    while held && gate.closed.load(Ordering::SeqCst) {
                        gate.waiting.store(true, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    reply
                }
                .boxed()
            });
            Ok(Arc::new(llm) as Arc<dyn Llm>)
        })
    }

    fn close(&self) {
        self.gate.waiting.store(false, Ordering::SeqCst);
        self.gate.closed.store(true, Ordering::SeqCst);
    }

    fn open(&self) {
        self.gate.closed.store(false, Ordering::SeqCst);
    }

    fn count(&self, purpose: &str) -> usize {
        self.purposes.lock().unwrap().iter().filter(|p| *p == purpose).count()
    }
}

struct Server {
    _tmp: tempfile::TempDir,
    runs_dir: PathBuf,
    base: String,
    http: reqwest::Client,
    manager: Option<ManagerThread>,
    server: tokio::task::JoinHandle<()>,
    factory: Factory,
}

impl Server {
    async fn start() -> Self {
        Server::with(Factory::new(), Duration::from_millis(500), |_| ()).await
    }

    async fn with(factory: Factory, grace: Duration, prepare: impl FnOnce(&Path)) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let runs_dir = tmp.path().join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        prepare(&runs_dir);
        let mut options = ManagerOptions::new(&runs_dir, factory.llm_factory());
        options.poll_interval = Duration::from_millis(50);
        options.shutdown_grace = grace;
        options.settings_path = Some(tmp.path().join("settings.json"));
        let (router, manager) = create_app(options, AppOptions { fake_llm: true, ..Default::default() });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Server {
            _tmp: tmp,
            runs_dir,
            base: format!("http://127.0.0.1:{port}"),
            http: reqwest::Client::new(),
            manager: Some(manager),
            server,
            factory,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        let res = self.http.get(self.url(path)).send().await.unwrap();
        (res.status().as_u16(), res.json().await.unwrap_or(Value::Null))
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let res = self.http.post(self.url(path)).json(&body).send().await.unwrap();
        (res.status().as_u16(), res.json().await.unwrap_or(Value::Null))
    }

    async fn post_empty(&self, path: &str) -> (u16, Value) {
        let res = self.http.post(self.url(path)).send().await.unwrap();
        (res.status().as_u16(), res.json().await.unwrap_or(Value::Null))
    }

    async fn detail(&self, run_id: &str) -> Value {
        let (status, body) = self.get(&format!("/api/runs/{run_id}")).await;
        assert_eq!(status, 200, "{body}");
        body
    }

    async fn start_run(&self, rounds: i64) -> String {
        let form = reqwest::multipart::Form::new()
            .text("question", "Will Harbour Town accept the ferry fare rise?")
            .text("agents", "4")
            .text("rounds", rounds.to_string())
            .text("batch", "2")
            .text("concurrency", "2")
            .part("files[]", reqwest::multipart::Part::bytes(SEED_TEXT.as_bytes()).file_name("harbour.md"));
        let res = self.http.post(self.url("/api/runs")).multipart(form).send().await.unwrap();
        assert_eq!(res.status().as_u16(), 200);
        res.json::<Value>().await.unwrap()["id"].as_str().unwrap().to_string()
    }

    async fn wait_state(&self, run_id: &str, states: &[&str]) -> Value {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let d = self.detail(run_id).await;
            if d["live"] == false && states.contains(&d["status"]["state"].as_str().unwrap_or("")) {
                return d;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {run_id} to reach {states:?}");
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    async fn wait_round_blocked(&self, run_id: &str) {
        wait_for("round 1 to reach the gate", || async { self.factory.gate.waiting.load(Ordering::SeqCst) }).await;
        assert_eq!(self.detail(run_id).await["live"], true);
    }

    async fn subscribers(&self, run_id: &str) -> usize {
        let id = run_id.to_string();
        let manager = self.manager.as_ref().unwrap();
        manager.handle.call(move |m| async move { m.subscriber_count(&id) }).await.unwrap()
    }

    /// Stops the server; the folder stays until the returned guard is dropped.
    async fn shutdown(mut self) -> (tempfile::TempDir, PathBuf) {
        if let Some(manager) = self.manager.take() {
            manager.shutdown().await;
        }
        self.server.abort();
        (self._tmp, self.runs_dir)
    }
}

async fn wait_for<F, Fut>(what: &str, mut predicate: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + TIMEOUT;
    while !predicate().await {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

/// Reads a text/event-stream in the background so a test can wait with a timeout.
struct SseReader {
    events: Arc<Mutex<Vec<(String, Value)>>>,
    task: tokio::task::JoinHandle<()>,
}

impl SseReader {
    async fn open(server: &Server, run_id: &str) -> Self {
        let mut response = server.http.get(server.url(&format!("/api/runs/{run_id}/stream"))).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 200);
        let events: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
        let sink = events.clone();
        let task = tokio::spawn(async move {
            let mut buffer = String::new();
            while let Ok(Some(chunk)) = response.chunk().await {
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(end) = buffer.find("\n\n") {
                    let block: String = buffer.drain(..end + 2).collect();
                    let mut name = None;
                    for line in block.lines() {
                        if let Some(n) = line.strip_prefix("event: ") {
                            name = Some(n.to_string());
                        } else if let (Some(data), Some(n)) = (line.strip_prefix("data: "), name.take()) {
                            sink.lock().unwrap().push((n, serde_json::from_str(data).unwrap()));
                        }
                    }
                }
            }
        });
        SseReader { events, task }
    }

    fn names(&self) -> Vec<String> {
        self.events.lock().unwrap().iter().map(|(n, _)| n.clone()).collect()
    }

    async fn wait(&self, name: &str) -> Value {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some((_, data)) = self.events.lock().unwrap().iter().find(|(n, _)| n == name) {
                return data.clone();
            }
            assert!(Instant::now() < deadline, "timed out waiting for SSE event {name}; got {:?}", self.names());
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    fn close(self) {
        self.task.abort();
    }
}

fn make_external_run(runs_dir: &Path, run_id: &str, with_report: bool, status: Option<RunStatus>) -> String {
    let run_dir = runs_dir.join(run_id);
    std::fs::create_dir_all(&run_dir).unwrap();
    let config =
        SimConfig { num_agents: 2, rounds: 2, ..SimConfig::new("Will the CLI run show up?", vec!["x.md".into()]) };
    write_json(&run_dir.join("config.json"), &config).unwrap();
    let people: Vec<Persona> = [1, 2]
        .iter()
        .map(|i| Persona {
            id: format!("a0{i}"),
            handle: format!("user{i}"),
            name: format!("User {i}"),
            bio: "b".into(),
            background: "bg".into(),
            stance: "s".into(),
            traits: Vec::new(),
            activity: 0.6,
            influence: 0.5,
            entity_id: None,
            camp: None,
            source: None,
        })
        .collect();
    write_json(&run_dir.join("personas.json"), &people).unwrap();
    let world = World::open(&run_dir.join("world.db"), 7).unwrap();
    world.add_personas(&people).unwrap();
    world.apply(0, &Action { content: "Opening post".into(), ..Action::new("a01", ActionType::Post) }).unwrap();
    world.save_round_stats(&RoundStats { round: 0, new_posts: 1, ..Default::default() }).unwrap();
    world.close().unwrap();
    if with_report {
        std::fs::write(
            run_dir.join("report.json"),
            r#"{"title": "T", "summary": "S", "prediction": "P", "confidence": "low"}"#,
        )
        .unwrap();
    }
    if let Some(status) = status {
        write_status(&run_dir, &status).unwrap();
    }
    run_id.to_string()
}

fn running_status(pid: Option<i64>, round: i64) -> RunStatus {
    RunStatus {
        state: RunState::Running,
        stage: RunStage::Rounds,
        round,
        total_rounds: 2,
        detail: String::new(),
        error: None,
        pid,
        started_at: now_iso(),
        updated_at: now_iso(),
        failed_batches: Vec::new(),
        pause: None,
    }
}

fn parent_pid() -> i64 {
    std::os::unix::process::parent_id() as i64
}

fn dead_pid() -> i64 {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    child.wait().unwrap();
    child.id() as i64
}

// ---------------------------------------------------------------- basics

#[test]
fn safe_filename_strips_directories_and_odd_characters() {
    let mut taken = HashSet::new();
    assert_eq!(safe_filename("../../etc/passwd.md", &mut taken), "passwd.md");
    assert_eq!(safe_filename("..\\..\\win.TXT", &mut taken), "win.txt");
    assert_eq!(safe_filename("passwd.md", &mut taken), "passwd-2.md");
    assert_eq!(safe_filename(".hidden.md", &mut taken), "hidden.md");
    assert_eq!(safe_filename("rés umé;rm -rf.pdf", &mut taken), "res ume_rm -rf.pdf");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn path_traversal_and_unknown_runs_rejected() {
    let server = Server::start().await;
    std::fs::create_dir_all(server.runs_dir.parent().unwrap().join("secret")).unwrap();
    std::fs::write(server.runs_dir.parent().unwrap().join("secret/config.json"), "{}").unwrap();
    for raw in ["%2E%2E", "..", "..%2Fsecret", "%2E%2E%2Fsecret", "a..b", ".hidden", "a%5C..%5Cb"] {
        for suffix in ["", "/posts", "/report.html", "/stream", "/agents/a01"] {
            let (status, _) = server.get(&format!("/api/runs/{raw}{suffix}")).await;
            assert!([400, 404].contains(&status), "{raw}{suffix}: {status}");
        }
    }
    assert_eq!(server.get("/api/runs/..foo").await.0, 400);
    assert_eq!(server.get("/api/runs/20990101-000000-nothing").await.0, 404);
    assert_eq!(server.post_empty("/api/runs/20990101-000000-nothing/stop").await.0, 404);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_only_guard_and_token() {
    let server = Server::start().await;
    let res = server.http.get(server.url("/api/health")).header("host", "evil.example").send().await.unwrap();
    assert_eq!(res.status().as_u16(), 403);
    let res = server
        .http
        .post(server.url("/api/runs"))
        .header("origin", "http://evil.example")
        .multipart(reqwest::multipart::Form::new().text("question", "x"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 403);
    let res = server.http.get(server.url("/api/health")).header("host", "127.0.0.1:8765").send().await.unwrap();
    assert_eq!(res.status().as_u16(), 200);
    server.shutdown().await;

    // `serve` adds a bearer token for every /api path, and only for those
    let tmp = tempfile::tempdir().unwrap();
    let options = ManagerOptions::new(&tmp.path().join("runs"), Factory::new().llm_factory());
    let token = "abcdefghijklmnopqrstuvwxyz".to_string();
    let (router, manager) =
        create_app(options, AppOptions { fake_llm: true, token: Some(token.clone()), ..Default::default() });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let task = tokio::spawn(async move { axum::serve(listener, router).await });
    let http = reqwest::Client::new();
    for (auth, path, want) in [
        (None, "/api/health", 401),
        (Some("Bearer wrong"), "/api/health", 401),
        (Some(format!("bearer {token}").leak() as &str), "/api/health", 401),
        (Some(format!("Bearer {token}").leak() as &str), "/api/health", 200),
        (None, "/api", 401),
        (None, "/api/nope", 401),
        (None, "/apis", 200),
        (None, "/", 200),
    ] {
        let mut request = http.get(format!("{base}{path}"));
        if let Some(auth) = auth {
            request = request.header("authorization", auth);
        }
        let res = request.send().await.unwrap();
        assert_eq!(res.status().as_u16(), want, "{auth:?} {path}");
        if want == 401 {
            assert_eq!(res.headers()["www-authenticate"], "Bearer");
            assert_eq!(res.json::<Value>().await.unwrap(), json!({"detail": "unauthorized"}));
        }
    }
    task.abort();
    manager.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upload_validation_leaves_no_run_behind() {
    let server = Server::start().await;
    let post = |fields: Vec<(&'static str, String)>, files: Vec<(&'static str, &'static str, Vec<u8>)>| {
        let server = &server;
        async move {
            let mut form = reqwest::multipart::Form::new()
                .text("question", "Will it work?")
                .text("agents", "4")
                .text("rounds", "2");
            for (k, v) in fields {
                form = form.text(k, v);
            }
            for (k, name, data) in files {
                form = form.part(k, reqwest::multipart::Part::bytes(data).file_name(name));
            }
            server.http.post(server.url("/api/runs")).multipart(form).send().await.unwrap().status().as_u16()
        }
    };
    let good = || vec![("files[]", "a.md", b"hello world".to_vec())];
    assert_eq!(post(vec![], vec![]).await, 400);
    assert_eq!(post(vec![], vec![("files[]", "a.exe", b"MZ".to_vec())]).await, 400);
    assert_eq!(post(vec![], vec![("files[]", "a.md", Vec::new())]).await, 400);
    assert_eq!(post(vec![("question", "  ".into())], good()).await, 400);
    assert_eq!(post(vec![("agents", "1".into())], good()).await, 400);
    assert_eq!(post(vec![("agents", "201".into())], good()).await, 400);
    assert_eq!(post(vec![("rounds", "0".into())], good()).await, 400);
    assert_eq!(post(vec![("rounds", "abc".into())], good()).await, 400);
    assert_eq!(post(vec![("batch", "21".into())], good()).await, 400);
    assert_eq!(post(vec![("agent_model", "haiku; rm -rf /".into())], good()).await, 400);
    assert_eq!(post(vec![("events", "not json".into())], good()).await, 400);
    assert_eq!(post(vec![("events", r#"[{"round": 9, "content": "late"}]"#.into())], good()).await, 400);
    assert_eq!(post(vec![], vec![("files[]", "big.txt", vec![b'x'; 20 * 1024 * 1024 + 1])]).await, 413);
    let too_many: Vec<_> = (0..101).map(|_| ("files[]", "doc.md", b"# doc".to_vec())).collect();
    assert_eq!(post(vec![], too_many).await, 400, "a folder of docs may send up to 100 files");
    assert_eq!(server.get("/api/runs").await.1, json!([]));
    assert_eq!(std::fs::read_dir(&server.runs_dir).unwrap().count(), 0, "failed uploads leave no run directory");
    server.shutdown().await;
}

// ---------------------------------------------------------------- live control

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn event_injection_rules_and_pickup() {
    let server = Server::start().await;
    server.factory.close();
    let run_id = server.start_run(3).await;
    server.wait_round_blocked(&run_id).await;
    let events = format!("/api/runs/{run_id}/events");
    let inject =
        |round: i64| server.post(&events, json!({"round": round, "content": "Breaking: the ferry breaks down"}));
    assert_eq!(inject(1).await.0, 409, "round 1 already started");
    assert_eq!(inject(0).await.0, 400);
    assert_eq!(inject(4).await.0, 400, "beyond the last round");
    assert_eq!(server.post(&events, json!({"round": 2, "content": ""})).await.0, 422);
    assert_eq!(inject(2).await.0, 200);

    let sse = SseReader::open(&server, &run_id).await;
    assert_eq!(sse.wait("status").await["state"], "running");
    assert_eq!(sse.names()[0], "status");
    server.factory.open();
    let picked = sse.wait("event_injected").await;
    assert_eq!(picked, json!({"round": 2, "content": "Breaking: the ferry breaks down"}));
    sse.wait("run_done").await;
    assert!(sse.names().contains(&"posts_added".into()) && sse.names().contains(&"round_done".into()));
    sse.close();
    let d = server.wait_state(&run_id, &["done"]).await;
    assert!(d["events"]
        .as_array()
        .unwrap()
        .contains(&json!({"round": 2, "content": "Breaking: the ferry breaks down"})));
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_live_run() {
    let server = Server::start().await;
    server.factory.close();
    let run_id = server.start_run(4).await;
    server.wait_round_blocked(&run_id).await;
    assert_eq!(server.post_empty(&format!("/api/runs/{run_id}/stop")).await, (200, json!({"ok": true})));
    server.factory.open();
    let d = server.wait_state(&run_id, &["stopped"]).await;
    assert_eq!(d["status"]["round"], 1, "the current round finished, then it stopped");
    assert!(!d["report"].is_null(), "a report is still written after >= 1 round");
    assert_eq!(server.post_empty(&format!("/api/runs/{run_id}/stop")).await.0, 409);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runs_survive_client_disconnect() {
    let server = Server::start().await;
    server.factory.close();
    let run_id = server.start_run(2).await;
    let sse = SseReader::open(&server, &run_id).await;
    sse.wait("status").await;
    assert_eq!(server.subscribers(&run_id).await, 1);
    sse.close();
    wait_for("the SSE subscriber to be released", || async { server.subscribers(&run_id).await == 0 }).await;
    server.factory.open();
    server.wait_state(&run_id, &["done"]).await;
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_stops_live_runs_and_ends_streams() {
    let server = Server::with(Factory::new(), Duration::from_millis(300), |_| ()).await;
    server.factory.close();
    let run_id = server.start_run(3).await;
    server.wait_round_blocked(&run_id).await;
    let sse = SseReader::open(&server, &run_id).await;
    sse.wait("status").await;
    let events = sse.events.clone();
    let reader = sse.task;
    let (_tmp, runs_dir) = server.shutdown().await;
    tokio::time::timeout(Duration::from_secs(5), reader).await.expect("the stream ends on shutdown").unwrap();
    let status: RunStatus =
        serde_json::from_str(&std::fs::read_to_string(runs_dir.join(&run_id).join("status.json")).unwrap()).unwrap();
    assert_eq!(status.state, RunState::Stopped);
    assert!(events.lock().unwrap().iter().any(|(n, _)| n == "run_stopped"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn report_rebuild_state_reports_failure_and_success() {
    let factory = Factory::new();
    factory.fail_reports.store(true, Ordering::SeqCst);
    let mut run_id = String::new();
    let server = Server::with(factory, Duration::from_millis(500), |runs| {
        run_id = make_external_run(runs, "20260101-120000-cli-run", true, None);
    })
    .await;
    assert_eq!(server.detail(&run_id).await["report_rebuild"], json!({"running": false, "error": null}));
    let rebuild = format!("/api/runs/{run_id}/report");
    assert_eq!(server.post_empty(&rebuild).await.0, 200);
    let state = rebuild_finished(&server, &run_id).await;
    assert!(state["error"].as_str().unwrap().contains("the model is down"), "{state}");

    server.factory.fail_reports.store(false, Ordering::SeqCst);
    let before = server.factory.count("report.write");
    assert_eq!(server.post_empty(&rebuild).await.0, 200);
    let state = rebuild_finished(&server, &run_id).await;
    assert_eq!(state, json!({"running": false, "error": null}), "a new rebuild clears the old failure");
    assert!(server.runs_dir.join(&run_id).join("report.html").exists());
    assert_eq!(server.factory.count("report.write"), before + 1);
    server.shutdown().await;
}

async fn rebuild_finished(server: &Server, run_id: &str) -> Value {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let state = server.detail(run_id).await["report_rebuild"].clone();
        if state["running"] == false {
            return state;
        }
        assert!(Instant::now() < deadline, "timed out waiting for the rebuild");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ask_needs_a_report() {
    let mut run_id = String::new();
    let server = Server::with(Factory::new(), Duration::from_millis(500), |runs| {
        run_id = make_external_run(runs, "20260101-120000-cli-run", false, None);
    })
    .await;
    assert_eq!(server.post(&format!("/api/runs/{run_id}/ask"), json!({"question": "Why?"})).await.0, 409);
    server.shutdown().await;
}

// ---------------------------------------------------------------- runs started elsewhere

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn older_and_stale_runs_get_a_derived_state() {
    let server = Server::with(Factory::new(), Duration::from_millis(500), |runs| {
        make_external_run(runs, "20260101-000001-old-done", true, None);
        make_external_run(runs, "20260101-000002-old-partial", false, None);
        make_external_run(runs, "20260101-000003-dead-cli", false, Some(running_status(Some(dead_pid()), 0)));
        make_external_run(runs, "20260101-000004-alive-cli", false, Some(running_status(Some(parent_pid()), 0)));
        std::fs::create_dir_all(runs.join("not-a-run")).unwrap();
    })
    .await;
    let (_, runs) = server.get("/api/runs").await;
    let states: std::collections::BTreeMap<String, String> = runs
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["id"].as_str().unwrap().into(), r["state"].as_str().unwrap().into()))
        .collect();
    let want: std::collections::BTreeMap<String, String> = [
        ("20260101-000001-old-done", "done"),
        ("20260101-000002-old-partial", "stopped"),
        ("20260101-000003-dead-cli", "stopped"),
        ("20260101-000004-alive-cli", "running"),
    ]
    .into_iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(states, want);
    let d = server.detail("20260101-000002-old-partial").await;
    assert_eq!(
        (d["status"]["state"].as_str(), d["report"].is_null(), &d["live"]),
        (Some("stopped"), true, &json!(false))
    );
    assert_eq!(server.detail("20260101-000001-old-done").await["status"]["stage"], "finished");
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn watcher_streams_a_run_written_by_another_process() {
    let mut run_id = String::new();
    let server = Server::with(Factory::new(), Duration::from_millis(500), |runs| {
        run_id = make_external_run(runs, "20260101-120000-cli-run", false, Some(running_status(Some(parent_pid()), 0)));
    })
    .await;
    let run_dir = server.runs_dir.join(&run_id);
    let sse = SseReader::open(&server, &run_id).await;
    let first = sse.wait("status").await;
    assert_eq!((first["state"].as_str(), sse.names()[0].as_str()), (Some("running"), "status"));

    // the "CLI" writes round 1 and a post, then finishes
    let world = World::open(&run_dir.join("world.db"), 7).unwrap();
    let new_id =
        world.apply(1, &Action { content: "Round one post".into(), ..Action::new("a02", ActionType::Post) }).unwrap();
    world.save_round_stats(&RoundStats { round: 1, new_posts: 1, sentiment: Some(0.2), ..Default::default() }).unwrap();
    world.close().unwrap();
    assert_eq!(sse.wait("posts_added").await, json!({"round": 1, "post_ids": [new_id]}));

    write_status(&run_dir, &running_status(Some(parent_pid()), 1)).unwrap();
    assert_eq!(sse.wait("round_done").await["round"], 1);
    wait_for("status round 1", || async {
        sse.events.lock().unwrap().iter().any(|(n, d)| n == "status" && d["round"] == 1)
    })
    .await;

    let done = RunStatus { state: RunState::Done, stage: RunStage::Finished, ..running_status(Some(parent_pid()), 2) };
    write_status(&run_dir, &done).unwrap();
    sse.wait("run_done").await;
    let last = sse.names().last().cloned().unwrap();
    assert!(["status", "run_done"].contains(&last.as_str()));
    sse.close();
    // external runs cannot be controlled from this server
    assert_eq!(server.post_empty(&format!("/api/runs/{run_id}/stop")).await.0, 409);
    assert_eq!(server.post(&format!("/api/runs/{run_id}/events"), json!({"round": 2, "content": "x"})).await.0, 409);
    server.shutdown().await;
}

#[test]
#[should_panic(expected = "an empty token would disable authentication")]
fn an_empty_token_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let options = ManagerOptions::new(&tmp.path().join("runs"), Factory::new().llm_factory());
    let _ = create_app(options, AppOptions { fake_llm: true, token: Some(String::new()), ..Default::default() });
}

impl Server {
    /// A 4-person, 1-round run with extra form fields.
    async fn start_run_with(&self, fields: &[(&str, String)]) -> (u16, Value) {
        let mut form = reqwest::multipart::Form::new()
            .text("question", "Will Harbour Town accept the ferry fare rise?")
            .text("agents", "4")
            .text("rounds", "1")
            .text("batch", "2")
            .part("files[]", reqwest::multipart::Part::bytes(SEED_TEXT.as_bytes()).file_name("harbour.md"));
        for (key, value) in fields {
            form = form.text(key.to_string(), value.clone());
        }
        let res = self.http.post(self.url("/api/runs")).multipart(form).send().await.unwrap();
        (res.status().as_u16(), res.json().await.unwrap_or(Value::Null))
    }

    async fn put(&self, path: &str, body: Value) -> (u16, Value) {
        let res = self.http.put(self.url(path)).json(&body).send().await.unwrap();
        (res.status().as_u16(), res.json().await.unwrap_or(Value::Null))
    }

    async fn wait_paused(&self, run_id: &str) -> Value {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let d = self.detail(run_id).await;
            if d["status"]["pause"] == "personas_ready" {
                return d;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {run_id} to pause: {}", d["status"]);
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn review_the_cast_edit_it_add_a_group_and_continue() {
    let server = Server::start().await;
    let (status, created) = server.start_run_with(&[("review_cast", "1".into()), ("fair_turns", "true".into())]).await;
    assert_eq!(status, 200, "{created}");
    let run_id = created["id"].as_str().unwrap().to_string();
    let cast_url = format!("/api/runs/{run_id}/personas");
    let sse = SseReader::open(&server, &run_id).await;
    let ready = sse.wait("personas_ready").await;
    assert_eq!(ready["personas"].as_array().unwrap().len(), 4);
    let detail = server.wait_paused(&run_id).await;
    assert_eq!((detail["live"].clone(), detail["status"]["stage"].clone()), (json!(true), json!("personas")));

    let (status, cast) = server.get(&cast_url).await;
    assert_eq!((status, cast["waiting"].clone()), (200, json!(true)));
    let mut people = cast["personas"].as_array().unwrap().clone();
    assert!(people.iter().all(|p| p["camp"].is_string() || p["entity_id"].is_string()), "fair casts plan camps");
    // remove the first person, rewrite the next one's stance
    people.remove(0);
    people[0]["stance"] = json!("Now firmly in favour of the electric ferry.");
    people[0]["source"] = json!("edited");
    let (status, edited) = server.put(&cast_url, json!({"personas": people})).await;
    assert_eq!(status, 200, "{edited}");
    let edited = edited["personas"].as_array().unwrap().clone();
    assert_eq!(edited.len(), 3);
    assert_eq!((edited[0]["id"].clone(), edited[0]["source"].clone()), (json!("a01"), json!("edited")));
    assert_eq!(server.put(&cast_url, json!({"personas": []})).await.0, 400);
    assert_eq!(server.put(&cast_url, json!({"people": []})).await.0, 422);
    assert_eq!(server.put(&cast_url, json!({"personas": [{"id": 1}]})).await.0, 422);
    assert_eq!(server.put(&cast_url, json!({"personas": vec![edited[0].clone(); 201]})).await.0, 422, "over MAX_CAST");
    assert_eq!(server.put("/api/runs/20990101-000000-nope/personas", json!({"personas": edited})).await.0, 404);
    // edits stay with the waiting run: personas.json is written once, by the run, after the confirm
    assert_eq!(server.detail(&run_id).await["personas"].as_array().unwrap().len(), 4);
    // a run whose cast is still under review cannot lend it to another run
    let (status, refused) = server.start_run_with(&[("cast_from", run_id.clone())]).await;
    assert_eq!(status, 409, "{refused}");

    let group_url = format!("{cast_url}/group");
    assert_eq!(server.post(&group_url, json!({"description": ""})).await.0, 422);
    assert_eq!(server.post(&group_url, json!({"description": " \n\t "})).await.0, 422);
    assert_eq!(server.post(&group_url, json!({"description": "x", "count": 21})).await.0, 422);
    let (status, grown) = server
        .post(&group_url, json!({"description": "three parents of Year 7 children who take the bus", "count": 3}))
        .await;
    assert_eq!(status, 200, "{grown}");
    let grown = grown["personas"].as_array().unwrap().clone();
    assert_eq!(grown.len(), 6);
    assert!(grown[3..].iter().all(|p| p["source"] == "group" && p["camp"].is_string()));
    assert_eq!(server.factory.count("personas.group"), 1);
    assert_eq!(server.factory.count("engine.round"), 0, "nothing runs while the cast is under review");
    // a full cast refuses another group before the model is asked
    assert_eq!(server.put(&cast_url, json!({"personas": vec![grown[0].clone(); 200]})).await.0, 200);
    let (status, full) = server.post(&group_url, json!({"description": "one more parent", "count": 1})).await;
    assert_eq!(status, 400, "{full}");
    assert_eq!(server.factory.count("personas.group"), 1);
    assert_eq!(server.put(&cast_url, json!({"personas": grown})).await.0, 200);

    assert_eq!(server.post_empty(&format!("{cast_url}/confirm")).await, (200, json!({"ok": true})));
    let done = server.wait_state(&run_id, &["done"]).await;
    assert_eq!(done["personas"].as_array().unwrap().len(), 6);
    assert_eq!(done["config"]["num_agents"], 6);
    assert_eq!(done["config"]["review_cast"], true);
    assert!(done["status"].get("pause").is_none());
    assert_eq!(done["personas"][0]["stance"], "Now firmly in favour of the electric ferry.");
    // past the pause, the cast can be read but not changed
    let (status, cast) = server.get(&cast_url).await;
    assert_eq!((status, cast["waiting"].clone(), cast["personas"].as_array().unwrap().len()), (200, json!(false), 6));
    assert_eq!(server.post_empty(&format!("{cast_url}/confirm")).await.0, 409);
    assert_eq!(server.put(&cast_url, json!({"personas": grown})).await.0, 409);
    // a new version keeps the source's turn rule, whatever the form says
    let (status, version) = server.start_run_with(&[("cast_from", run_id.clone())]).await;
    assert_eq!(status, 200, "{version}");
    let version = server.wait_state(version["id"].as_str().unwrap(), &["done"]).await;
    assert_eq!(version["config"]["fair_turns"], true);
    sse.close();
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_while_the_cast_is_under_review() {
    let server = Server::start().await;
    let (_, created) = server.start_run_with(&[("review_cast", "on".into())]).await;
    let run_id = created["id"].as_str().unwrap().to_string();
    server.wait_paused(&run_id).await;
    assert_eq!(server.post_empty(&format!("/api/runs/{run_id}/stop")).await, (200, json!({"ok": true})));
    let d = server.wait_state(&run_id, &["stopped"]).await;
    assert!(d["report"].is_null());
    assert!(d["status"].get("pause").is_none());
    assert_eq!(server.factory.count("engine.round"), 0);
    // its source ran without fair turns, so a version of it does too
    let form = [("cast_from", run_id.clone()), ("fair_turns", "true".into())];
    let (status, version) = server.start_run_with(&form).await;
    assert_eq!(status, 200, "{version}");
    let version = server.wait_state(version["id"].as_str().unwrap(), &["done"]).await;
    assert!(version["config"].get("fair_turns").is_none(), "{}", version["config"]);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_saved_audience_skips_casting() {
    let server = Server::start().await;
    let audience = json!({
        "name": "  Bus   parents ",
        "personas": [
            {"id": "x", "handle": "parent_one", "name": "Parent One", "bio": "b", "background": "bg",
             "stance": "worried about the fare", "camp": "lean_critical"},
            {"id": "y", "handle": "parent_one", "name": "Parent Two", "bio": "b", "background": "bg",
             "stance": "keen on the ferry", "camp": "strong_support"},
        ],
        "graph": {"entities": [], "relations": []},
    });
    assert_eq!(server.start_run_with(&[("audience", "{not json".into())]).await.0, 400);
    assert_eq!(server.start_run_with(&[("audience", json!({"name": "x", "personas": []}).to_string())]).await.0, 400);
    let (status, created) = server.start_run_with(&[("audience", audience.to_string())]).await;
    assert_eq!(status, 200, "{created}");
    let run_id = created["id"].as_str().unwrap().to_string();
    let d = server.wait_state(&run_id, &["done"]).await;
    assert_eq!(d["config"]["audience"], "Bus parents");
    assert_eq!(d["config"]["num_agents"], 2);
    let handles: Vec<&str> = d["personas"].as_array().unwrap().iter().map(|p| p["handle"].as_str().unwrap()).collect();
    assert_eq!(handles, ["parent_one", "parent_one_2"]);
    assert_eq!(server.factory.count("personas.batch") + server.factory.count("graph.extract"), 0);
    server.shutdown().await;
}
