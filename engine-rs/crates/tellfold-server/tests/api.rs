//! The Rust server against the Python one: the session recorded by `golden/make_api.py` (runs, reports, chat, ask,
//! rewrite, examples, studies, branch, check, settings, runtimes and their errors on the canned model) is replayed
//! against a fresh server, and every status code, content type, listed header and body must match after the same
//! normalisation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

use tellfold_llm::canned::fake_responder;
use tellfold_llm::fake::FakeLlm;
use tellfold_llm::Llm;
use tellfold_server::{create_app, AppOptions, LlmFactory, ManagerOptions};

#[derive(Deserialize, Clone)]
struct Form {
    fields: Vec<(String, String)>,
    files: Vec<(String, String, String)>,
}

#[derive(Deserialize, Clone)]
struct Step {
    name: String,
    method: String,
    path: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    json: Option<Value>,
    raw: Option<String>,
    content_type: Option<String>,
    form: Option<Form>,
    #[serde(default)]
    stream: bool,
    wait: Option<BTreeMap<String, String>>,
    copy: Option<BTreeMap<String, String>>,
    status: Option<u16>,
    #[serde(default)]
    headers_out: BTreeMap<String, String>,
    body: Option<Value>,
}

#[derive(Deserialize)]
struct Session {
    runs: usize,
    studies: usize,
    headers: Vec<String>,
    steps: Vec<Step>,
}

/// Status, content type, compared headers and normalised body of one response.
type Output = (u16, String, BTreeMap<String, String>, String);

fn fixture() -> Session {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../golden/fixtures/api/session.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("fixture JSON")
}

/// Steps whose body is a list of runs or studies: compared in id order, since runs started in the same second
/// may sort either way.
const LISTS: [&str; 6] = ["runs", "runs with example", "all runs", "studies", "all studies", "no runs yet"];

fn new_names(folder: &Path, known: &[String]) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n != "studies" && !known.contains(n))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// The run's id without what depends on the clock (its timestamp, and the `-2` suffix a run started in the same second
/// gets), so the slug made from the question is still compared. Same as `masked_run` in make_api.py.
fn masked_run(index: usize, name: &str) -> String {
    let shape = Regex::new(r"^\d{8}-\d{6}-(.+?)(?:-\d+)?$").unwrap();
    match shape.captures(name) {
        Some(c) => format!("{{run:{index}}}-{}", &c[1]),
        None => name.to_string(),
    }
}

/// The study id, when it has the timestamp-and-random-hex shape: an id of any other shape is left to differ.
fn masked_study(index: usize, name: &str) -> String {
    let shape = Regex::new(r"^\d{8}-\d{6}-[0-9a-f]{4}$").unwrap();
    if shape.is_match(name) {
        format!("{{study:{index}}}")
    } else {
        name.to_string()
    }
}

fn normalise(text: &str, root: &Path, runs: &[String], studies: &[String]) -> String {
    let mut folders = vec![root.to_string_lossy().into_owned()];
    if let Ok(real) = root.canonicalize() {
        folders.push(real.to_string_lossy().into_owned());
    }
    folders.sort_by_key(|f| std::cmp::Reverse(f.len()));
    folders.dedup();
    let mut text = text.to_string();
    for folder in &folders {
        text = text.replace(folder.as_str(), "<root>");
    }
    let mut names: Vec<(&str, String)> = runs.iter().enumerate().map(|(i, n)| (n.as_str(), masked_run(i, n))).collect();
    names.extend(studies.iter().enumerate().map(|(i, n)| (n.as_str(), masked_study(i, n))));
    names.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
    for (name, placeholder) in names {
        text = text.replace(name, &placeholder);
    }
    let stamp = Regex::new(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:\d\d)?").unwrap();
    let pid = Regex::new(r#""pid":\s?\d+"#).unwrap();
    let timing = Regex::new(r#""seconds":\s?-?\d+(?:\.\d+)?(?:e[+-]?\d+)?"#).unwrap();
    let text = stamp.replace_all(&text, "<time>");
    let text = pid.replace_all(&text, r#""pid":0"#);
    timing.replace_all(&text, r#""seconds":0"#).into_owned()
}

fn sorted_by_id(text: &str) -> String {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Array(mut items)) => {
            items.sort_by_key(|v| v["id"].as_str().unwrap_or("").to_string());
            serde_json::to_string(&items).unwrap()
        }
        _ => text.to_string(),
    }
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

struct Client {
    http: reqwest::Client,
    base: String,
}

impl Client {
    async fn json(&self, path: &str) -> Value {
        self.http.get(format!("{}{path}", self.base)).send().await.unwrap().json().await.unwrap()
    }

    async fn wait(&self, what: &BTreeMap<String, String>) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if let Some(run) = what.get("run") {
                let detail = self.json(&format!("/api/runs/{run}")).await;
                let state = detail["status"]["state"].as_str().unwrap_or("");
                if detail["live"] == false && ["done", "failed", "stopped"].contains(&state) {
                    return;
                }
            } else if let Some(run) = what.get("rebuild") {
                if self.json(&format!("/api/runs/{run}")).await["report_rebuild"]["running"] == false {
                    return;
                }
            } else {
                let study = self.json(&format!("/api/studies/{}", what["study"])).await;
                let busy = study["repeating"] == true
                    || study["compare_state"]["running"] == true
                    || study["runs"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|r| ["running", "queued"].contains(&r["state"].as_str().unwrap_or("")));
                let ended = !study["compare"].is_null()
                    || !study["agreement"].is_null()
                    || !study["compare_state"]["error"].is_null();
                if !busy && ended {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("timed out waiting for {what:?}");
    }

    /// (status, content type, picked headers, body)
    async fn send(&self, step: &Step, path: &str, resolve: &dyn Fn(&str) -> String, picked: &[String]) -> Output {
        let method = reqwest::Method::from_bytes(step.method.as_bytes()).unwrap();
        let mut request = self.http.request(method, format!("{}{path}", self.base));
        for (k, v) in &step.headers {
            request = request.header(k.as_str(), v.as_str());
        }
        if let Some(json) = &step.json {
            request = request.json(json);
        } else if let Some(raw) = &step.raw {
            request = request.header("content-type", step.content_type.clone().unwrap_or_default()).body(raw.clone());
        } else if let Some(form) = &step.form {
            let mut multipart = reqwest::multipart::Form::new();
            for (k, v) in &form.fields {
                multipart = multipart.text(k.clone(), resolve(v));
            }
            for (k, name, text) in &form.files {
                let part = reqwest::multipart::Part::bytes(text.as_bytes().to_vec())
                    .file_name(name.clone())
                    .mime_str("application/octet-stream")
                    .unwrap();
                multipart = multipart.part(k.clone(), part);
            }
            if form.files.is_empty() {
                multipart = multipart.text("unused", "");
            }
            request = request.multipart(multipart);
        }
        let mut response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
        let content_type = header("content-type").unwrap_or_default();
        let headers: BTreeMap<String, String> =
            picked.iter().filter_map(|h| header(h).map(|v| (h.clone(), v))).collect();
        let body = if step.stream {
            let mut text = String::new();
            while let Ok(Some(chunk)) = tokio::time::timeout(Duration::from_secs(5), response.chunk()).await.unwrap() {
                text.push_str(&String::from_utf8_lossy(&chunk));
                if text.contains("event: status") && text.ends_with("\n\n") {
                    break;
                }
            }
            text
        } else {
            response.text().await.unwrap()
        };
        (status, content_type, headers, body)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_of_requests_matches_python() {
    let session = fixture();
    let tmp = tempfile::tempdir().unwrap();
    let root: PathBuf = tmp.path().to_path_buf();
    let (runs_dir, examples_dir) = (root.join("runs"), root.join("examples"));
    std::fs::create_dir_all(&examples_dir).unwrap();
    let factory: LlmFactory = Arc::new(|_, _, _| Ok(Arc::new(FakeLlm::new(fake_responder)) as Arc<dyn Llm>));
    let mut options = ManagerOptions::new(&runs_dir, factory);
    options.poll_interval = Duration::from_millis(100);
    options.settings_path = Some(root.join("settings.json"));
    options.examples_dir = Some(examples_dir.clone());
    let (router, manager) = create_app(options, AppOptions { fake_llm: true, ..Default::default() });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move { axum::serve(listener, router).await });
    let client = Client { http: reqwest::Client::new(), base: format!("http://127.0.0.1:{port}") };

    let mut runs: Vec<String> = Vec::new();
    let mut studies: Vec<String> = Vec::new();
    let mut outputs: Vec<Option<Output>> = Vec::new();
    let placeholder = Regex::new(r"\{(run|study):(\d+)\}").unwrap();
    for step in &session.steps {
        let resolve = |text: &str| -> String {
            placeholder
                .replace_all(text, |c: &regex::Captures| {
                    let i: usize = c[2].parse().unwrap();
                    if &c[1] == "run" {
                        runs[i].clone()
                    } else {
                        studies[i].clone()
                    }
                })
                .into_owned()
        };
        match step.method.as_str() {
            "WAIT" => {
                let what = step.wait.as_ref().unwrap().iter().map(|(k, v)| (k.clone(), resolve(v))).collect();
                client.wait(&what).await;
                outputs.push(None);
            }
            "COPY" => {
                let copy = step.copy.as_ref().unwrap();
                copy_tree(&runs_dir.join(resolve(&copy["from"])), &examples_dir.join(&copy["to"]));
                outputs.push(None);
            }
            _ => {
                let path = resolve(&step.path);
                outputs.push(Some(client.send(step, &path, &resolve, &session.headers).await));
            }
        }
        let new = new_names(&runs_dir, &runs);
        runs.extend(new);
        let new = new_names(&runs_dir.join("studies"), &studies);
        studies.extend(new);
    }
    assert_eq!((runs.len(), studies.len()), (session.runs, session.studies), "runs and studies made");

    let mut failures = Vec::new();
    for (step, output) in session.steps.iter().zip(&outputs) {
        let Some((status, content_type, headers, body)) = output else { continue };
        let label = format!("{} ({} {})", step.name, step.method, step.path);
        if Some(*status) != step.status {
            failures.push(format!("{label}: status {status}, Python {:?}\n  body: {body}", step.status));
            continue;
        }
        if content_type != step.content_type.as_deref().unwrap_or("") {
            failures.push(format!("{label}: content type {content_type:?}, Python {:?}", step.content_type));
        }
        let headers: BTreeMap<String, String> =
            headers.iter().map(|(k, v)| (k.clone(), normalise(v, &root, &runs, &studies))).collect();
        if headers != step.headers_out {
            failures.push(format!("{label}: headers {headers:?}, Python {:?}", step.headers_out));
        }
        match &step.body {
            Some(Value::Number(n)) => {
                if body.len() as u64 != n.as_u64().unwrap() {
                    failures.push(format!("{label}: {} bytes, Python {n}", body.len()));
                }
            }
            Some(Value::String(want)) => {
                let mut got = normalise(body, &root, &runs, &studies);
                let mut want = want.clone();
                if LISTS.contains(&step.name.as_str()) {
                    got = sorted_by_id(&got);
                    want = sorted_by_id(&want);
                }
                if got != want {
                    failures.push(format!("{label}: body differs\n  rust:   {got}\n  python: {want}"));
                }
            }
            other => panic!("{label}: unexpected fixture body {other:?}"),
        }
    }
    server.abort();
    manager.shutdown().await;
    assert!(
        failures.is_empty(),
        "{} of {} steps differ:\n\n{}",
        failures.len(),
        outputs.iter().flatten().count(),
        failures.join("\n\n")
    );
}
