//! `tellfold serve` as the Mac app runs it (ports of the serve tests in `tests/test_runtimes_server.py` and
//! `tests/test_serve_shutdown.py`): one `{"port": N}` line on stdout, a bearer token on every /api request, and an
//! exit when stdin closes or a signal arrives, with live runs marked stopped.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TOKEN: &str = "test-token-0123456789abcdef";

fn serve(home: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tellfold"));
    command.arg("serve").args(args);
    for (key, _) in std::env::vars() {
        if key.starts_with("TELLFOLD_") || key.starts_with("SHOAL_") {
            command.env_remove(key);
        }
    }
    command.env("TELLFOLD_FAKE_LLM", "1").env("TELLFOLD_HOME", home).env("TELLFOLD_SERVE_TOKEN", TOKEN);
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    command
}

/// The port from the first stdout line.
fn port_of(child: &mut Child) -> u16 {
    let mut line = String::new();
    BufReader::new(child.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
    let value: serde_json::Value = serde_json::from_str(&line).unwrap_or_else(|_| panic!("a port line, got {line:?}"));
    assert_eq!(line, format!("{{\"port\": {}}}\n", value["port"]), "the exact line Python prints");
    value["port"].as_u64().unwrap() as u16
}

/// (status, body) of a plain HTTP/1.1 request.
fn request(port: u16, method: &str, path: &str, token: Option<&str>, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let auth = token.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
    let kind = if body.is_empty() { String::new() } else { "Content-Type: application/json\r\n".into() };
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth}{kind}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let status = response.split(' ').nth(1).unwrap().parse().unwrap();
    let body = response.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

fn wait_exit(child: &mut Child, limit: Duration) -> Option<i32> {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            return status.code();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    panic!("the server did not exit within {limit:?}");
}

#[test]
fn serve_prints_port_requires_token_and_exits_on_stdin_eof() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    let mut child =
        serve(&tmp.path().join("home"), &["--port", "0", "--runs", runs.to_str().unwrap()]).spawn().unwrap();
    let port = port_of(&mut child);
    assert!(port > 0);
    assert_eq!(request(port, "GET", "/api/health", None, "").0, 401);
    assert_eq!(request(port, "GET", "/api/health", Some("nope"), "").0, 401);
    let (status, body) = request(port, "GET", "/api/health", Some(TOKEN), "");
    assert_eq!((status, body.as_str()), (200, r#"{"ok":true,"fake_llm":true,"version":"0.3.1"}"#));
    assert_eq!(request(port, "GET", "/api/settings", Some(TOKEN), "").0, 200);
    drop(child.stdin.take());
    assert_eq!(wait_exit(&mut child, Duration::from_secs(15)), Some(0));
    let mut rest = String::new();
    child.stdout.take().unwrap().read_to_string(&mut rest).unwrap();
    assert_eq!(rest, "", "exactly one stdout line; logs go to stderr");
    let mut err = String::new();
    child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    let real = std::fs::canonicalize(&runs).unwrap();
    assert_eq!(err, format!("tellfold serve: 127.0.0.1:{port}  runs: {}  (fake LLM)\n", real.display()));
}

#[test]
fn serve_rejects_a_short_token_and_a_bad_log_level() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let out =
        serve(&home, &["--token", "short", "--runs", tmp.path().join("runs").to_str().unwrap()]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&out.stderr), "error: --token must be at least 16 characters\n");
    let out = serve(&home, &["--log-level", "loud"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&out.stderr), "error: --log-level must be debug, info, warning or error\n");
}

#[test]
fn serve_port_zero_gives_each_server_its_own_port_and_a_taken_port_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let mut children: Vec<Child> = (0..2)
        .map(|i| serve(&home, &["--runs", tmp.path().join(format!("runs{i}")).to_str().unwrap()]).spawn().unwrap())
        .collect();
    let ports: Vec<u16> = children.iter_mut().map(port_of).collect();
    assert_ne!(ports[0], ports[1]);
    let taken = serve(&home, &["--port", &ports[0].to_string(), "--runs", tmp.path().join("runs2").to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(taken.status.code(), Some(1));
    let err = String::from_utf8_lossy(&taken.stderr);
    assert!(
        err.starts_with(&format!("error: cannot bind 127.0.0.1:{}: [Errno ", ports[0]))
            && err.contains("Address already in use"),
        "{err}"
    );
    for mut child in children {
        drop(child.stdin.take());
        assert_eq!(wait_exit(&mut child, Duration::from_secs(15)), Some(0));
    }
}

#[test]
fn a_signal_stops_live_runs_before_the_server_exits() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    let mut command = serve(&tmp.path().join("home"), &["--runs", runs.to_str().unwrap()]);
    command.env("TELLFOLD_FAKE_DELAY", "0.3");
    let mut child = command.spawn().unwrap();
    let port = port_of(&mut child);
    // a run with slow fake calls: it is still going when the signal arrives
    let boundary = "XBOUNDARY";
    let seed = "# Ferry fares\n\nThe council wants to raise the ferry fare. Commuters disagree.\n";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"question\"\r\n\r\nWill the fare rise pass?\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"agents\"\r\n\r\n3\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"files[]\"; filename=\"seed.md\"\r\n\r\n{seed}\r\n--{boundary}--\r\n"
    );
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        stream,
        "POST /api/runs HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {TOKEN}\r\n\
         Content-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let id = response.split("\"id\":\"").nth(1).unwrap().split('"').next().unwrap().to_string();
    std::thread::sleep(Duration::from_millis(700));
    // SAFETY: kill only sends SIGTERM to the child we started.
    unsafe {
        libc_kill(child.id() as i32, 15);
    }
    assert_eq!(wait_exit(&mut child, Duration::from_secs(15)), Some(0));
    let status: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(runs.join(&id).join("status.json")).unwrap()).unwrap();
    assert_eq!(status["state"], "stopped", "{status}");
}

extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, signal: i32) -> i32;
}
