//! `tellfold mcp` end to end on the canned model: an MCP client on the other end of stdin/stdout initializes,
//! lists the tools, starts a run, waits for it and reads the report, then compares two drafts on one crowd, and
//! talks to a `tellfold serve` that is already running.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// `tellfold ARGS` on the canned model, with no TELLFOLD_ settings of the machine running the tests.
fn tellfold(home: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tellfold"));
    command.args(args);
    for (key, _) in std::env::vars() {
        if key.starts_with("TELLFOLD_") || key.starts_with("SHOAL_") {
            command.env_remove(key);
        }
    }
    command.env("TELLFOLD_FAKE_LLM", "1").env("TELLFOLD_HOME", home);
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
    command
}

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Client {
    fn start(home: &Path, runs: &Path) -> Self {
        Self::spawn(home, &["mcp", "--runs", runs.to_str().unwrap()], &[])
    }

    fn spawn(home: &Path, args: &[&str], env: &[(&str, &str)]) -> Self {
        let mut command = tellfold(home, args);
        command.envs(env.iter().copied());
        let mut child = command.spawn().unwrap();
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Client { child, stdin, stdout, next_id: 1 }
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    /// The result of a request; every stdout line must be a JSON-RPC message.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let mut line = String::new();
            assert!(self.stdout.read_line(&mut line).unwrap() > 0, "the server closed stdout");
            let message: Value =
                serde_json::from_str(&line).unwrap_or_else(|_| panic!("stdout must carry only JSON-RPC, got {line:?}"));
            assert_eq!(message["jsonrpc"], "2.0", "{message}");
            if message["id"] == json!(id) {
                assert!(message.get("error").is_none(), "{method} failed: {message}");
                return message["result"].clone();
            }
        }
    }

    /// A tool's result; panics on a tool error.
    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let result = self.request("tools/call", json!({"name": tool, "arguments": arguments}));
        assert_ne!(result["isError"], json!(true), "{tool} failed: {result}");
        result
    }

    /// A tool's error text; panics unless the call is a tool error.
    fn call_error(&mut self, tool: &str, arguments: Value) -> String {
        let result = self.request("tools/call", json!({"name": tool, "arguments": arguments}));
        assert_eq!(result["isError"], json!(true), "{tool} should fail: {result}");
        result["content"][0]["text"].as_str().unwrap().to_string()
    }

    fn wait_for_run(&mut self, run_id: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let status = self.call("wait_for_run", json!({"run_id": run_id, "timeout_seconds": 20}));
            let status = status["structuredContent"].clone();
            if status["finished"] == json!(true) {
                return status;
            }
            assert!(Instant::now() < deadline, "the run did not finish: {status}");
        }
    }

    /// Closes stdin and waits for the exit code.
    fn finish(mut self) -> Option<i32> {
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        panic!("tellfold mcp did not exit after stdin closed");
    }
}

fn initialize(client: &mut Client) -> Value {
    let info = client.request(
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "tellfold-test", "version": "1.0.0"},
        }),
    );
    client.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    info
}

const CHANGELOG: &str = "# Mossbook 3.0\n\nMossbook is now a subscription: $6 a month, or $48 a year. Everything you \
bought before stays yours. Sync and the new AI summaries need the subscription.\n";

#[test]
fn initialize_list_tools_run_and_read_the_report() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    let mut client = Client::start(&tmp.path().join("home"), &runs);

    let info = initialize(&mut client);
    assert_eq!(info["serverInfo"]["name"], "tellfold");
    assert_eq!(info["serverInfo"]["version"], tellfold_cli::VERSION);
    assert!(info["capabilities"]["tools"].is_object(), "{info}");
    assert!(info["instructions"].as_str().unwrap().contains("wait_for_run"));

    let tools = client.request("tools/list", json!({}));
    let mut names: Vec<&str> = tools["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "ask_crowd",
            "check_run",
            "compare_drafts",
            "get_report",
            "get_run_status",
            "get_study",
            "inject_event",
            "interview_person",
            "list_people",
            "list_runs",
            "start_simulation",
            "stop_run",
            "wait_for_run",
        ]
    );
    let start = tools["tools"].as_array().unwrap().iter().find(|t| t["name"] == "start_simulation").unwrap();
    assert_eq!(start["inputSchema"]["required"], json!(["question"]), "{start}");
    assert!(start["inputSchema"]["properties"]["text"].is_object());

    // a bad request is a tool error the caller can read, not a protocol error
    let error = client.call_error("start_simulation", json!({"question": "Will they pay?"}));
    assert!(error.contains("text"), "{error}");
    let error = client.call_error("get_run_status", json!({"run_id": "no-such-run"}));
    assert!(error.contains("run not found"), "{error}");

    let started = client.call(
        "start_simulation",
        json!({
            "question": "Will existing users accept the subscription?",
            "text": CHANGELOG,
            "title": "changelog",
            "agents": 4,
            "rounds": 2,
        }),
    );
    let started = &started["structuredContent"];
    assert_eq!(started["documents"], json!(["changelog.md"]));
    let run_id = started["run_id"].as_str().unwrap().to_string();
    assert!(runs.join(&run_id).join("seeds/changelog.md").is_file());

    let status = client.call("get_run_status", json!({"run_id": run_id}))["structuredContent"].clone();
    assert_eq!(status["run_id"], json!(run_id));
    assert_eq!(status["total_rounds"], 2);

    let done = client.wait_for_run(&run_id);
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(done["has_report"], true);
    assert_eq!(done["progress"], 1.0);
    assert_eq!(done["people"], 4);

    let markdown = client.call("get_report", json!({"run_id": run_id}));
    let text = markdown["content"][0]["text"].as_str().unwrap();
    assert_eq!(text, std::fs::read_to_string(runs.join(&run_id).join("report.md")).unwrap());
    assert!(text.starts_with("# "), "{text}");

    let report = client.call("get_report", json!({"run_id": run_id, "format": "json"}));
    let report = &report["structuredContent"];
    assert!(report["prediction"].as_str().is_some_and(|p| !p.is_empty()), "{report}");
    assert!(report["confidence"].is_string(), "{report}");
    let both = client.call("get_report", json!({"run_id": run_id, "format": "both"}));
    assert_eq!(both["content"].as_array().unwrap().len(), 2);

    let listed = client.call("list_runs", json!({}))["structuredContent"].clone();
    assert_eq!(listed["total"], 1);
    assert_eq!(listed["runs"][0]["id"], json!(run_id));
    assert_eq!(listed["runs"][0]["has_report"], true);

    let people = client.call("list_people", json!({"run_id": run_id}))["structuredContent"].clone();
    let first = people["people"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(people["people"].as_array().unwrap().len(), 4);

    let answer = client.call("ask_crowd", json!({"run_id": run_id, "question": "Who pushed back hardest?"}));
    assert!(answer["structuredContent"]["answer"].as_str().is_some_and(|a| !a.is_empty()), "{answer}");
    let reply = client
        .call("interview_person", json!({"run_id": run_id, "person": first, "message": "Would you pay $6 a month?"}));
    assert!(reply["structuredContent"]["reply"].as_str().is_some_and(|r| !r.is_empty()), "{reply}");

    // a finished run takes no events
    let error = client.call_error("inject_event", json!({"run_id": run_id, "round": 2, "content": "News"}));
    assert!(error.contains("HTTP 409"), "{error}");

    assert_eq!(client.finish(), Some(0));
}

#[test]
fn compare_two_drafts_on_one_crowd() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    let mut client = Client::start(&tmp.path().join("home"), &runs);
    initialize(&mut client);

    let started = client.call(
        "compare_drafts",
        json!({
            "question": "Will existing users accept the subscription?",
            "draft_a": CHANGELOG,
            "draft_b": "# Mossbook 3.0\n\nEverything you have today stays free, forever. A new Plus plan ($6 a month) \
        adds sync and AI summaries.\n",
            "label_b": "Plus plan",
            "agents": 4,
            "rounds": 2,
        }),
    );
    let started = &started["structuredContent"];
    let run_a = started["run_a"].as_str().unwrap().to_string();
    let run_b = started["run_b"].as_str().expect("the canned crowd is ready at once").to_string();
    let study_id = started["study_id"].as_str().unwrap().to_string();

    assert_eq!(client.wait_for_run(&run_a)["state"], "done");
    let b = client.wait_for_run(&run_b);
    assert_eq!((b["state"].as_str(), b["study_id"].as_str()), (Some("done"), Some(study_id.as_str())));
    assert_eq!(b["variant"], "Plus plan");

    // by the draft A run id, as compare_drafts suggests while draft B is being set up
    let study = client.call("get_study", json!({"run_id": run_a, "wait_seconds": 60}))["structuredContent"].clone();
    assert_eq!(study["study_id"], json!(study_id));
    assert_eq!(study["kind"], "compare");
    assert_eq!(study["ready"], true, "{study}");
    assert_eq!(study["runs"].as_array().unwrap().len(), 2);
    assert!(study["comparison"].is_object(), "{study}");

    assert_eq!(client.finish(), Some(0));
}

#[test]
fn use_a_server_that_is_already_running() {
    const TOKEN: &str = "test-token-0123456789abcdef";
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let runs = tmp.path().join("runs");
    let mut server = tellfold(&home, &["serve", "--runs", runs.to_str().unwrap()])
        .env("TELLFOLD_SERVE_TOKEN", TOKEN)
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(server.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
    let port = serde_json::from_str::<Value>(&line).unwrap()["port"].as_u64().unwrap();
    let url = format!("http://127.0.0.1:{port}");

    // a wrong token is the server's 401, as a tool error
    let mut client = Client::spawn(&home, &["mcp", "--url", &url, "--token", "wrong-token-0123456789"], &[]);
    initialize(&mut client);
    let error = client.call_error("list_runs", json!({}));
    assert!(error.contains("HTTP 401"), "{error}");
    assert_eq!(client.finish(), Some(0));

    let mut client = Client::spawn(&home, &["mcp"], &[("TELLFOLD_URL", &url), ("TELLFOLD_SERVE_TOKEN", TOKEN)]);
    initialize(&mut client);
    let started = client.call(
        "start_simulation",
        json!({"question": "Will they accept it?", "text": CHANGELOG, "agents": 3, "rounds": 1}),
    );
    let run_id = started["structuredContent"]["run_id"].as_str().unwrap().to_string();
    assert!(runs.join(&run_id).is_dir(), "the run is the server's");
    assert_eq!(client.wait_for_run(&run_id)["state"], "done");
    assert_eq!(client.finish(), Some(0));

    drop(server.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(20);
    while server.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "tellfold serve did not exit");
        std::thread::sleep(Duration::from_millis(50));
    }
}
