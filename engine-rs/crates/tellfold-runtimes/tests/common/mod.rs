#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tellfold_llm::Schema;
use tellfold_runtimes::process::{ProcessResult, ProcessRunner};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const SECRET: &str = "sk-test-0123456789abcdef";

pub fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../golden/fixtures").join(name);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap()
}

/// Runs a future on a fresh runtime (tests hold the environment lock outside any async code).
pub fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap().block_on(future)
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Pick {
    pub color: String,
    pub n: i64,
}

/// `Pick.model_json_schema()`.
pub fn pick_schema() -> Schema {
    Schema::from_value::<Pick>(json!({
        "properties": {"color": {"title": "Color", "type": "string"}, "n": {"title": "N", "type": "integer"}},
        "required": ["color", "n"],
        "title": "Pick",
        "type": "object",
    }))
}

static ENV_LOCK: Mutex<()> = Mutex::new(());

const CLEARED: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "CURSOR_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "GROK_API_KEY",
    "FACTORY_API_KEY",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "COPILOT_GITHUB_TOKEN",
    "NPM_CONFIG_PREFIX",
    "npm_config_prefix",
    "TELLFOLD_KEY_LOCAL",
    "TELLFOLD_ALLOW_UNSAFE_RUNTIMES",
    "SHOAL_ALLOW_UNSAFE_RUNTIMES",
    "TELLFOLD_SERVE_TOKEN",
    "TELLFOLD_KEY_DEEPSEEK",
    "OPENCODE_CONFIG_CONTENT",
];

/// The `isolated_home` fixture: settings, HOME and PATH point into a temp folder, so detection never sees the
/// real tools. Holds the process-wide environment lock; restores every variable it touched when dropped.
pub struct Isolated {
    pub root: tempfile::TempDir,
    saved: HashMap<String, Option<String>>,
    _lock: MutexGuard<'static, ()>,
}

impl Isolated {
    pub fn new() -> Isolated {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = tempfile::tempdir().unwrap();
        let mut isolated = Isolated { root, saved: HashMap::new(), _lock: lock };
        let home = isolated.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(isolated.bin()).unwrap();
        isolated.set("TELLFOLD_HOME", &isolated.path().join("tellfold-home").to_string_lossy());
        isolated.set("HOME", &home.to_string_lossy());
        isolated.set("PATH", &isolated.bin().to_string_lossy());
        for name in CLEARED {
            isolated.remove(name);
        }
        // the real machine's Homebrew / /usr/local folders must not leak in
        tellfold_runtimes::detect::override_extra_bin_dirs(Some(vec![home.join(".local").join("bin")]));
        isolated
    }

    pub fn path(&self) -> PathBuf {
        self.root.path().canonicalize().unwrap()
    }

    pub fn bin(&self) -> PathBuf {
        self.path().join("bin")
    }

    pub fn home(&self) -> PathBuf {
        self.path().join("home")
    }

    fn remember(&mut self, name: &str) {
        self.saved.entry(name.into()).or_insert_with(|| std::env::var(name).ok());
    }

    pub fn set(&mut self, name: &str, value: &str) {
        self.remember(name);
        std::env::set_var(name, value);
    }

    pub fn remove(&mut self, name: &str) {
        self.remember(name);
        std::env::remove_var(name);
    }
}

impl Drop for Isolated {
    fn drop(&mut self) {
        tellfold_runtimes::detect::override_extra_bin_dirs(None);
        for (name, value) in &self.saved {
            match value {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }
    }
}

pub fn make_script(folder: &Path, name: &str, body: &str) -> PathBuf {
    std::fs::create_dir_all(folder).unwrap();
    let path = folder.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

// ---------- a fake process runner ----------

#[derive(Debug, Clone)]
pub struct RunnerCall {
    pub argv: Vec<String>,
    pub stdin: Option<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub cwd_was_empty: bool,
}

pub type Respond = Box<dyn Fn(&[String], &Path) -> ProcessResult + Send + Sync>;

pub enum Step {
    Result(ProcessResult),
    Func(Respond),
}

impl From<ProcessResult> for Step {
    fn from(r: ProcessResult) -> Step {
        Step::Result(r)
    }
}

#[derive(Default)]
pub struct FakeRunner {
    steps: Mutex<Vec<Step>>,
    pub calls: Mutex<Vec<RunnerCall>>,
}

impl FakeRunner {
    pub fn new(steps: Vec<Step>) -> Arc<FakeRunner> {
        Arc::new(FakeRunner { steps: Mutex::new(steps), calls: Mutex::new(Vec::new()) })
    }

    pub fn calls(&self) -> Vec<RunnerCall> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl ProcessRunner for FakeRunner {
    async fn run(
        &self,
        argv: &[String],
        stdin: Option<&str>,
        cwd: &Path,
        env: &BTreeMap<String, String>,
    ) -> io::Result<ProcessResult> {
        let cwd_was_empty = std::fs::read_dir(cwd).unwrap().next().is_none();
        self.calls.lock().unwrap().push(RunnerCall {
            argv: argv.to_vec(),
            stdin: stdin.map(str::to_string),
            cwd: cwd.to_path_buf(),
            env: env.clone(),
            cwd_was_empty,
        });
        let step = self.steps.lock().unwrap().remove(0);
        Ok(match step {
            Step::Result(r) => r,
            Step::Func(f) => f(argv, cwd),
        })
    }
}

// ---------- a local HTTP stub (httpx.MockTransport's stand-in) ----------

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: String,
}

impl Request {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap()
    }
}

pub type Handler = Arc<dyn Fn(&Request) -> (u16, String) + Send + Sync>;

pub struct Stub {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Request>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Serves `handler`'s (status, body) for every request; must be started inside a runtime.
pub async fn stub(handler: impl Fn(&Request) -> (u16, String) + Send + Sync + 'static) -> Stub {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests: Arc<Mutex<Vec<Request>>> = Arc::default();
    let handler: Handler = Arc::new(handler);
    let seen = requests.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let handler = handler.clone();
            let seen = seen.clone();
            tokio::spawn(async move {
                let Some(request) = read_request(&mut socket).await else { return };
                seen.lock().unwrap().push(request.clone());
                let (status, body) = handler(&request);
                let response = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    Stub { url, requests, task }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Request> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let path = first.next()?.to_string();
    let headers: HashMap<String, String> =
        lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string())).collect();
    let length: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < length {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Request { method, path, headers, body: String::from_utf8_lossy(&body).into_owned() })
}

/// A URL nothing listens on.
pub fn closed_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    url
}
