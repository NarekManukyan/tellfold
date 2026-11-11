//! Ports of the server parts of `tests/test_check.py`, `test_examples.py` and `test_runtimes_server.py`: "Check
//! it" series, examples without an AI tool, settings and the runtime fields of a new run.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::FutureExt;
use serde_json::{json, Value};

use tellfold_llm::canned::fake_responder;
use tellfold_llm::fake::{FakeLlm, FakeReply};
use tellfold_llm::{Llm, LlmError};
use tellfold_server::{
    create_app, AppOptions, BundleHook, BundleStage, LlmFactory, ManagerOptions, ManagerThread, RuntimeReady,
};

const SEED: &[u8] =
    b"# Harbour Town votes on a new ferry fare\n\nThe council proposes raising the ferry fare from 2 to \
3 euros. Commuters oppose it; shops support the new electric ferry.\n";
const TIMEOUT: Duration = Duration::from_secs(60);

/// (run folder, purpose) of every call.
type Calls = Arc<Mutex<Vec<(Option<PathBuf>, String)>>>;

struct Server {
    tmp: tempfile::TempDir,
    runs: PathBuf,
    base: String,
    http: reqwest::Client,
    manager: Option<ManagerThread>,
    task: tokio::task::JoinHandle<()>,
    calls: Calls,
    /// calls for run folders other than this one fail
    only: Arc<Mutex<Option<PathBuf>>>,
}

impl Server {
    async fn start(configure: impl FnOnce(&mut ManagerOptions, &Path)) -> Server {
        Server::start_with(configure, AppOptions { fake_llm: true, ..Default::default() }).await
    }

    async fn start_with(configure: impl FnOnce(&mut ManagerOptions, &Path), app: AppOptions) -> Server {
        let tmp = tempfile::tempdir().unwrap();
        let runs = tmp.path().join("runs");
        let calls: Calls = Arc::default();
        let only: Arc<Mutex<Option<PathBuf>>> = Arc::default();
        let (record, gate) = (calls.clone(), only.clone());
        let factory: LlmFactory = Arc::new(move |run_dir, _, _| {
            let run_dir = run_dir.map(Path::to_path_buf);
            let (record, gate) = (record.clone(), gate.clone());
            let llm = FakeLlm::new_async(move |purpose, prompt, schema| {
                record.lock().unwrap().push((run_dir.clone(), purpose.to_string()));
                let broken = match (&*gate.lock().unwrap(), &run_dir) {
                    (Some(only), Some(dir)) => dir != only && !dir.to_string_lossy().contains("/studies/"),
                    _ => false,
                };
                let reply = if broken {
                    FakeReply::Error(LlmError::new("the model is gone"))
                } else {
                    fake_responder(purpose, prompt, schema)
                };
                async move { reply }.boxed()
            });
            Ok(Arc::new(llm) as Arc<dyn Llm>)
        });
        let mut options = ManagerOptions::new(&runs, factory);
        options.poll_interval = Duration::from_millis(50);
        options.shutdown_grace = Duration::from_millis(500);
        options.settings_path = Some(tmp.path().join("settings.json"));
        configure(&mut options, tmp.path());
        let (router, manager) = create_app(options, app);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Server { tmp, runs, base, http: reqwest::Client::new(), manager: Some(manager), task, calls, only }
    }

    async fn get(&self, path: &str) -> Value {
        self.http.get(format!("{}{path}", self.base)).send().await.unwrap().json().await.unwrap()
    }

    async fn send(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut request = self.http.request(method, format!("{}{path}", self.base));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let res = request.send().await.unwrap();
        (res.status().as_u16(), res.json().await.unwrap_or(Value::Null))
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        self.send(reqwest::Method::POST, path, Some(body)).await
    }

    async fn post_run(&self, extra: &[(&str, &str)]) -> (u16, Value) {
        let mut form = reqwest::multipart::Form::new()
            .text("question", "Will Harbour Town accept the fare?")
            .text("agents", "4")
            .text("rounds", "2")
            .text("batch", "2");
        for (k, v) in extra {
            form = form.text(k.to_string(), v.to_string());
        }
        form = form.part("files[]", reqwest::multipart::Part::bytes(SEED).file_name("fare.md"));
        let res = self.http.post(format!("{}/api/runs", self.base)).multipart(form).send().await.unwrap();
        (res.status().as_u16(), res.json().await.unwrap())
    }

    /// A finished run.
    async fn finished_run(&self) -> String {
        let (status, body) = self.post_run(&[]).await;
        assert_eq!(status, 200, "{body}");
        let id = body["id"].as_str().unwrap().to_string();
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let d = self.get(&format!("/api/runs/{id}")).await;
            if d["status"]["state"] == "done" && d["live"] == false {
                return id;
            }
            assert!(Instant::now() < deadline, "timed out waiting for the first run");
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    async fn study_when(&self, study_id: &str, what: &str, done: fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let study = self.get(&format!("/api/studies/{study_id}")).await;
            if done(&study) {
                return study;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    async fn stop(mut self) {
        if let Some(manager) = self.manager.take() {
            manager.shutdown().await;
        }
        self.task.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn check_it_runs_one_after_another_and_writes_the_agreement() {
    let server = Server::start(|_, _| ()).await;
    let source = server.finished_run().await;
    let (status, body) = server.post(&format!("/api/runs/{source}/check"), json!({"runs": 3})).await;
    assert_eq!(status, 200, "{body}");
    let study_id = body["study_id"].as_str().unwrap().to_string();
    let first = server.get(&format!("/api/studies/{study_id}")).await;
    assert_eq!((first["kind"].as_str(), first["planned"].as_i64()), (Some("repeat"), Some(3)));

    let study = server.study_when(&study_id, "the agreement", |s| !s["agreement"].is_null()).await;
    let labels: Vec<(String, String)> = study["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["variant"].as_str().unwrap().into(), r["letter"].as_str().unwrap().into()))
        .collect();
    assert_eq!(labels, [("Run 1".into(), "A".into()), ("Run 2".into(), "B".into()), ("Run 3".into(), "C".into())]);
    assert_eq!((&study["repeating"], &study["compare"]), (&json!(false), &Value::Null));
    let ids: Vec<String> = study["runs"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().into()).collect();
    let mut configs = Vec::new();
    let mut statuses = Vec::new();
    for id in &ids {
        let d = server.get(&format!("/api/runs/{id}")).await;
        configs.push(d["config"].clone());
        statuses.push(d["status"].clone());
        assert_eq!(std::fs::read(server.runs.join(id).join("seeds/fare.md")).unwrap(), SEED);
    }
    assert_eq!(configs.iter().map(|c| c["seed"].as_i64().unwrap()).collect::<Vec<_>>(), [7, 8, 9]);
    assert_eq!(configs[1..].iter().map(|c| c["cast_from"].as_str().unwrap()).collect::<Vec<_>>(), [&source, &source]);
    // one after another: each new run started after the previous one finished
    assert!(statuses[1]["updated_at"].as_str() <= statuses[2]["started_at"].as_str());
    // no persona calls for the repeats; the agreement's calls are made for the study folder
    let calls = server.calls.lock().unwrap().clone();
    for id in &ids[1..] {
        let dir = server.runs.join(id);
        assert!(!calls
            .iter()
            .any(|(d, p)| d.as_ref() == Some(&dir) && (p.starts_with("graph.") || p.starts_with("personas."))));
    }
    let folder = server.runs.join("studies").join(&study_id);
    assert!(calls.iter().any(|(d, p)| d.as_ref() == Some(&folder) && p == "study.agree"));
    let agreement: Value =
        serde_json::from_str(&std::fs::read_to_string(folder.join("agreement.json")).unwrap()).unwrap();
    assert_eq!(agreement["runs"], 3);
    assert!(["consistent", "mixed", "unstable"].contains(&agreement["verdict"].as_str().unwrap()));

    let listed = server.get("/api/studies").await;
    let listed = listed.as_array().unwrap().iter().find(|s| s["id"] == study_id.as_str()).unwrap().clone();
    assert_eq!((&listed["has_compare"], &listed["planned"]), (&json!(true), &json!(3)));

    // the check can be redone, and an agent model can be swapped for the new runs
    assert_eq!(server.post(&format!("/api/studies/{study_id}/compare"), json!({})).await.0, 200);
    let (_, res) =
        server.post(&format!("/api/runs/{source}/check"), json!({"runs": 2, "agent_model": "other-model"})).await;
    let second =
        server.study_when(res["study_id"].as_str().unwrap(), "the second check", |s| !s["agreement"].is_null()).await;
    let run = server.get(&format!("/api/runs/{}", second["runs"][1]["id"].as_str().unwrap())).await;
    assert_eq!(run["config"]["agent_model"], "other-model");
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn check_validation() {
    let server = Server::start(|_, _| ()).await;
    let source = server.finished_run().await;
    let check = format!("/api/runs/{source}/check");
    assert_eq!(server.post(&check, json!({"runs": 6})).await.0, 422);
    assert_eq!(server.post(&check, json!({"runs": 1})).await.0, 422);
    assert_eq!(server.post(&check, json!({"agent_model": "bad model!"})).await.0, 422);
    assert_eq!(server.post("/api/runs/nope/check", json!({})).await.0, 404);
    std::fs::remove_file(server.runs.join(&source).join("report.json")).unwrap();
    let (status, body) = server.post(&check, json!({})).await;
    assert_eq!(status, 409);
    assert!(body["detail"].as_str().unwrap().contains("finished run with a report"));
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_series_that_stops_early_says_why() {
    let server = Server::start(|_, _| ()).await;
    let source = server.finished_run().await;
    *server.only.lock().unwrap() = Some(server.runs.join(&source));
    let (_, body) = server.post(&format!("/api/runs/{source}/check"), json!({"runs": 3})).await;
    let study_id = body["study_id"].as_str().unwrap().to_string();
    let study = server.study_when(&study_id, "the reason", |s| !s["compare_state"]["error"].is_null()).await;
    assert_eq!(study["compare_state"]["error"], "Run 2 did not finish, so the check stopped after 2 of 3 runs");
    assert_eq!((&study["planned"], &study["repeating"]), (&json!(2), &json!(false)));
    assert_eq!(study["runs"].as_array().unwrap().len(), 2);
    let saved: Value = serde_json::from_str(
        &std::fs::read_to_string(server.runs.join("studies").join(&study_id).join("study.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["planned"], 2);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_example_needs_an_ai_tool_to_interview() {
    let not_ready: RuntimeReady = Arc::new(|| async { false }.boxed());
    let server = Server::start(move |options, root| {
        options.runtime_ready = Some(not_ready);
        options.examples_dir = Some(root.join("examples"));
    })
    .await;
    let source = server.finished_run().await;
    let example = server.tmp.path().join("examples/demo");
    copy_tree(&server.runs.join(&source), &example);
    let before = listing(&example);
    let (status, body) = server.post("/api/runs/example-demo/chat", json!({"agent_id": "a01", "message": "Hi"})).await;
    assert_eq!(status, 409);
    assert_eq!(body["detail"], "set up an AI tool first to interview people or ask questions in an example run");
    let (status, body) = server.post("/api/runs/example-demo/check", json!({})).await;
    assert_eq!((status, body["detail"].as_str()), (409, Some("set up an AI tool first to check an example run")));
    let (status, body) = server.post("/api/runs/example-demo/branch", json!({"round": 1})).await;
    assert_eq!((status, body["detail"].as_str()), (409, Some("set up an AI tool first to branch an example run")));
    // a run of its own can reuse the example's people without a tool check
    let (status, body) = server.post_run(&[("cast_from", "example-demo")]).await;
    assert_eq!(status, 200, "{body}");
    let id = body["id"].as_str().unwrap();
    let config: Value =
        serde_json::from_str(&std::fs::read_to_string(server.runs.join(id).join("config.json")).unwrap()).unwrap();
    assert_eq!(config["cast_from"], "example-demo");
    assert!(config["study_id"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(listing(&example), before, "nothing is written next to an example");
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bundle_opens_as_a_read_only_run() {
    let not_ready: RuntimeReady = Arc::new(|| async { false }.boxed());
    let server = Server::start(move |options, _| options.runtime_ready = Some(not_ready)).await;
    let source = server.finished_run().await;
    let res = server.http.get(format!("{}/api/runs/{source}/bundle", server.base)).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 200);
    assert_eq!(res.headers()["content-type"], "application/vnd.tellfold.run+zip");
    assert_eq!(
        res.headers()["content-disposition"].to_str().unwrap(),
        format!("attachment; filename=\"{source}.tellfold\"")
    );
    let bundle = res.bytes().await.unwrap();

    let import = |body: Vec<u8>| server.http.post(format!("{}/api/runs/import", server.base)).body(body).send();
    let res = import(bundle.to_vec()).await.unwrap();
    assert_eq!(res.status().as_u16(), 200);
    let id = res.json::<Value>().await.unwrap()["id"].as_str().unwrap().to_string();
    assert_eq!(id, format!("{source}-2"), "a second copy next to the first");

    let runs = server.get("/api/runs").await;
    let row = runs.as_array().unwrap().iter().find(|r| r["id"] == id.as_str()).unwrap().clone();
    assert_eq!(
        (row["imported"].as_bool(), row["example"].as_bool(), row["state"].as_str()),
        (Some(true), Some(false), Some("done"))
    );
    let own = runs.as_array().unwrap().iter().find(|r| r["id"] == source.as_str()).unwrap().clone();
    assert!(own.get("imported").is_none(), "left out for a run of one's own");
    let detail = server.get(&format!("/api/runs/{id}")).await;
    assert_eq!(detail["imported"], true);
    assert!(detail["report"]["title"].is_string());

    let before = listing(&server.runs.join(&id));
    let read_only =
        "this run was imported from a .tellfold bundle; it cannot be changed. Use Run again to make your own copy";
    let (status, body) = server.post(&format!("/api/runs/{id}/events"), json!({"round": 2, "content": "News"})).await;
    assert_eq!((status, body["detail"].as_str()), (403, Some(read_only)));
    let (status, body) = server.post(&format!("/api/runs/{id}/report"), json!({})).await;
    assert_eq!((status, body["detail"].as_str()), (403, Some(read_only)));
    let (status, body) =
        server.post(&format!("/api/runs/{id}/chat"), json!({"agent_id": "a01", "message": "Hi"})).await;
    assert_eq!(status, 409);
    assert_eq!(body["detail"], "set up an AI tool first to interview people or ask questions in an imported run");
    let (status, body) = server.post(&format!("/api/runs/{id}/check"), json!({})).await;
    assert_eq!((status, body["detail"].as_str()), (409, Some("set up an AI tool first to check an imported run")));
    // nor can its cast be edited, extended or confirmed
    let people = server.get(&format!("/api/runs/{id}/personas")).await;
    assert_eq!(people["waiting"], false);
    let cast = json!({"personas": people["personas"].clone()});
    let (status, body) = server.send(reqwest::Method::PUT, &format!("/api/runs/{id}/personas"), Some(cast)).await;
    assert_eq!((status, body["detail"].as_str()), (403, Some(read_only)));
    let group = json!({"description": "Night-shift nurses", "count": 2});
    let (status, body) = server.post(&format!("/api/runs/{id}/personas/group"), group).await;
    assert_eq!((status, body["detail"].as_str()), (403, Some(read_only)));
    let (status, body) = server.post(&format!("/api/runs/{id}/personas/confirm"), json!({})).await;
    assert_eq!((status, body["detail"].as_str()), (403, Some(read_only)));
    assert_eq!(listing(&server.runs.join(&id)), before, "nothing is written next to an imported run");

    let res = import(b"definitely not a zip".to_vec()).await.unwrap();
    assert_eq!(res.status().as_u16(), 400);
    assert!(res.json::<Value>().await.unwrap()["detail"].as_str().unwrap().contains("not a Tellfold bundle"));
    let res = import(Vec::new()).await.unwrap();
    assert_eq!(res.status().as_u16(), 400);
    server.stop().await;
}

/// A bundle hook that stops each bundle request at `stage` until the test lets it go, and says when one is waiting.
struct Gate {
    waiting: tokio::sync::mpsc::UnboundedReceiver<()>,
    release: std::sync::mpsc::Sender<()>,
}

fn gate(stage: BundleStage) -> (BundleHook, Gate) {
    let (waiting_tx, waiting) = tokio::sync::mpsc::unbounded_channel();
    let (release, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = Mutex::new(release_rx);
    let hook = BundleHook(Arc::new(move |at| {
        if at == stage {
            let _ = waiting_tx.send(());
            let _ = release_rx.lock().unwrap().recv_timeout(TIMEOUT);
        }
    }));
    (hook, Gate { waiting, release })
}

/// Posts a report rebuild until it is accepted; panics if it is still refused after the timeout.
async fn rebuild_accepted(server: &Server, run_id: &str) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let (status, body) = server.post(&format!("/api/runs/{run_id}/report"), json!({})).await;
        if status == 200 {
            break;
        }
        assert!(status == 409 && Instant::now() < deadline, "{status} {body}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let deadline = Instant::now() + TIMEOUT;
    while server.get(&format!("/api/runs/{run_id}")).await["report_rebuild"]["running"] == true {
        assert!(Instant::now() < deadline, "the rebuild did not finish");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_export_dropped_before_its_reply_does_not_block_rebuilds() {
    let (hook, mut gate) = gate(BundleStage::Checked);
    let app = AppOptions { fake_llm: true, bundle_hook: Some(hook), ..Default::default() };
    let server = Server::start_with(|_, _| (), app).await;
    let source = server.finished_run().await;
    // the manager has marked the run and is held before it replies; the client gives up meanwhile
    let export = tokio::spawn(server.http.get(format!("{}/api/runs/{source}/bundle", server.base)).send());
    gate.waiting.recv().await.unwrap();
    export.abort();
    let _ = export.await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    gate.release.send(()).unwrap();
    // the unsent reply took the mark with it
    rebuild_accepted(&server, &source).await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_server_keeps_answering_while_a_bundle_is_made_or_opened() {
    let (hook, mut gate) = gate(BundleStage::Work);
    let app = AppOptions { fake_llm: true, bundle_hook: Some(hook), ..Default::default() };
    let server = Server::start_with(|_, _| (), app).await;
    let source = server.finished_run().await;
    let answers = |path: String| {
        let request = server.http.get(format!("{}{path}", server.base)).send();
        async move { tokio::time::timeout(Duration::from_secs(10), request).await.map(|r| r.unwrap().status().as_u16()) }
    };

    // export: the bundle is being written, and the manager still answers
    let url = format!("{}/api/runs/{source}/bundle", server.base);
    let export = tokio::spawn(server.http.get(url).send());
    gate.waiting.recv().await.unwrap();
    assert_eq!(answers("/api/runs".into()).await, Ok(200));
    assert_eq!(answers(format!("/api/runs/{source}")).await, Ok(200));
    // the report files must not change under the bundle being written
    let (status, body) = server.post(&format!("/api/runs/{source}/report"), json!({})).await;
    assert_eq!(status, 409, "{body}");
    assert!(body["detail"].as_str().unwrap().contains("being saved as a bundle"), "{body}");
    gate.release.send(()).unwrap();
    let bundle = export.await.unwrap().unwrap().bytes().await.unwrap();
    // once it is written, the report can be rebuilt again
    rebuild_accepted(&server, &source).await;

    // import: the same while the bundle is checked and unpacked
    let url = format!("{}/api/runs/import", server.base);
    let import = tokio::spawn(server.http.post(url).body(bundle.to_vec()).send());
    gate.waiting.recv().await.unwrap();
    assert_eq!(answers("/api/runs".into()).await, Ok(200));
    gate.release.send(()).unwrap();
    let res = import.await.unwrap().unwrap();
    assert_eq!(res.status().as_u16(), 200);
    let id = res.json::<Value>().await.unwrap()["id"].as_str().unwrap().to_string();

    // a rewrite suggested here is kept in memory: the imported run stays as it arrived
    let before = listing(&server.runs.join(&id));
    let (status, suggestion) = server.post(&format!("/api/runs/{id}/rewrite"), json!({})).await;
    assert_eq!(status, 200, "{suggestion}");
    assert_eq!(server.get(&format!("/api/runs/{id}/rewrite")).await, suggestion);
    assert_eq!(listing(&server.runs.join(&id)), before);
    server.stop().await;
}

/// Every file under `dir` with its size and modification time.
fn listing(dir: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            out.extend(listing(&path));
        } else {
            let meta = entry.metadata().unwrap();
            out.push((path, meta.len(), meta.modified().unwrap()));
        }
    }
    out.sort();
    out
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().filter_map(|e| e.ok()) {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn settings_roundtrip_and_validation() {
    let server = Server::start(|_, _| ()).await;
    let defaults = server.get("/api/settings").await;
    assert_eq!(defaults["agent_runtime"], "claude-code");
    let mut new = defaults.clone();
    new["agent_runtime"] = json!("openai-compatible:lms");
    new["agent_model"] = json!("qwen/qwen3-8b");
    new["concurrency"] = json!(6);
    new["openai_endpoints"] = json!([{"id": "lms", "name": "LM Studio", "base_url": "http://127.0.0.1:1234/v1"}]);
    let (status, saved) = server.send(reqwest::Method::PUT, "/api/settings", Some(new.clone())).await;
    assert_eq!(status, 200, "{saved}");
    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(server.tmp.path().join("settings.json")).unwrap()).unwrap();
    assert_eq!(on_disk["agent_runtime"], "openai-compatible:lms");
    assert_eq!(server.get("/api/settings").await["agent_runtime"], "openai-compatible:lms");
    let mut bad = new.clone();
    bad["planner_runtime"] = json!("openai-compatible:nope");
    assert_eq!(server.send(reqwest::Method::PUT, "/api/settings", Some(bad)).await.0, 400);
    let mut bad = new.clone();
    bad["concurrency"] = json!(99);
    assert_eq!(server.send(reqwest::Method::PUT, "/api/settings", Some(bad)).await.0, 422);
    let mut bad = new.clone();
    bad["ollama_base_url"] = json!("file:///etc");
    assert_eq!(server.send(reqwest::Method::PUT, "/api/settings", Some(bad)).await.0, 422);

    // a run names its runtimes, else the settings' (planner: the saved claude-code)
    let (status, body) = server
        .post_run(&[("agent_runtime", "ollama"), ("agent_model", "llama3.2:3b"), ("planner_model", "qwen/qwen3-8b")])
        .await;
    assert_eq!(status, 200, "{body}");
    let config: Value = serde_json::from_str(
        &std::fs::read_to_string(server.runs.join(body["id"].as_str().unwrap()).join("config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        (config["agent_runtime"].as_str(), config["planner_runtime"].as_str()),
        (Some("ollama"), Some("claude-code"))
    );
    assert_eq!(config["planner_model"], "qwen/qwen3-8b");
    let (status, body) = server.post_run(&[]).await;
    assert_eq!(status, 200, "{body}");
    let config: Value = serde_json::from_str(
        &std::fs::read_to_string(server.runs.join(body["id"].as_str().unwrap()).join("config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(config["agent_runtime"], "openai-compatible:lms", "the saved default");
    let before = std::fs::read_dir(&server.runs).unwrap().count();
    assert_eq!(server.post_run(&[("agent_runtime", "skynet")]).await.0, 400);
    assert_eq!(server.post_run(&[("planner_runtime", "../../etc")]).await.0, 400);
    assert_eq!(std::fs::read_dir(&server.runs).unwrap().count(), before, "no run directory left behind");

    // the runtime test is a canned answer with a fake model
    let (status, body) = server.post("/api/runtimes/claude-code/test", json!({"model": "sonnet"})).await;
    assert_eq!((status, body["model"].as_str(), &body["ok"]), (200, Some("sonnet"), &json!(true)));
    assert_eq!(server.send(reqwest::Method::POST, "/api/runtimes/BAD%20ID/test", None).await.0, 400);
    assert_eq!(server.get("/api/runtimes?check_auth=7").await["detail"][0]["type"], "less_than_equal");
    server.stop().await;
}
