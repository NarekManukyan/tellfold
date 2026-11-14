//! Child processes: the tellfold CLI in its own session (killed as a group on a timeout, an error or Ctrl-C), the
//! Claude Code CLI with a timeout, and waiting for a dropped network.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant};

/// Signs in a failed CLI's output that the network, not the request, failed: worth waiting for and trying again. Only
/// checked on a call or run that failed, since a model's own answer may mention a network of any kind.
pub const NETWORK_ERRORS: &[&str] = &[
    "ENOTFOUND",
    "Can't reach the API",
    "ECONNRESET",
    "ECONNREFUSED",
    "ETIMEDOUT",
    "EAI_AGAIN",
    "getaddrinfo",
    "socket hang up",
];
/// How many times one run or call waits for the network and tries again before it fails.
pub const NETWORK_TRIES: usize = 6;

const POLL: Duration = Duration::from_millis(100);
/// How long a group gets to stop after SIGTERM before SIGKILL, and after SIGKILL before giving up on it.
const GRACE: Duration = Duration::from_secs(30);

pub fn network_error(text: &str) -> bool {
    let text = text.to_lowercase();
    NETWORK_ERRORS.iter().any(|sign| text.contains(&sign.to_lowercase()))
}

/// The last `n` characters of `text`.
pub fn tail(text: &str, n: usize) -> &str {
    let count = text.chars().count();
    if count <= n {
        return text;
    }
    let skip = text.char_indices().nth(count - n).map_or(0, |(i, _)| i);
    &text[skip..]
}

/// The first `n` characters of `text`.
pub fn head(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// Returns once api.anthropic.com accepts a connection at all.
pub fn wait_for_network(check_every: Duration) {
    use std::net::{TcpStream, ToSocketAddrs};
    loop {
        let reachable = ("api.anthropic.com", 443)
            .to_socket_addrs()
            .ok()
            .into_iter()
            .flatten()
            .any(|addr| TcpStream::connect_timeout(&addr, Duration::from_secs(10)).is_ok());
        if reachable {
            return;
        }
        eprintln!("network down, waiting...");
        std::thread::sleep(check_every);
    }
}

// ---------- the CLI's process group and Ctrl-C ----------

/// The process group of the CLI running now, 0 when none.
static GROUP: AtomicI32 = AtomicI32::new(0);
/// Set by SIGINT, SIGTERM or SIGHUP while a CLI runs: the wait loop kills the group and the pass stops.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(sig: libc::c_int) {
    let group = GROUP.load(Ordering::SeqCst);
    if group > 0 && INTERRUPTED.load(Ordering::SeqCst) {
        // a second Ctrl-C while the group is still stopping: kill it outright and quit now
        // SAFETY: killpg, signal and raise are async-signal-safe.
        unsafe {
            libc::killpg(group, libc::SIGKILL);
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        }
    } else if group > 0 {
        INTERRUPTED.store(true, Ordering::SeqCst);
        // SAFETY: killpg is async-signal-safe.
        unsafe { libc::killpg(group, libc::SIGTERM) };
    } else {
        // nothing of ours to stop: die of the signal as if there were no handler
        // SAFETY: signal and raise are async-signal-safe.
        unsafe {
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        }
    }
}

/// Stops the CLI's whole group when the harness is stopped, since the CLI runs in its own session and no longer gets
/// the terminal's signals.
pub fn install_signal_handlers() {
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        // SAFETY: the handler only touches atomics and async-signal-safe calls.
        unsafe { libc::signal(sig, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t) };
    }
}

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// A child that leads its own process group, killed with its whole group unless it was waited for: on a timeout, on
/// an error and when this value is dropped early (a panic).
struct Group {
    child: Child,
    done: bool,
}

impl Group {
    fn kill(&mut self) {
        if self.done {
            return;
        }
        let pid = self.child.id() as libc::pid_t;
        for sig in [libc::SIGTERM, libc::SIGKILL] {
            // SAFETY: plain syscall on our own child's group.
            if unsafe { libc::killpg(pid, sig) } != 0 {
                break; // no such group: everything in it is gone
            }
            if wait_until(&mut self.child, Instant::now() + GRACE).ok().flatten().is_some() {
                break;
            }
        }
        let _ = self.child.try_wait();
        self.done = true;
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        self.kill();
        GROUP.store(0, Ordering::SeqCst);
    }
}

fn wait_until(child: &mut Child, deadline: Instant) -> std::io::Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(POLL);
    }
}

/// Why [`run_cli`] returned without an exit code.
#[derive(Debug)]
pub enum CliEnd {
    /// It exited with this code (-1 when a signal ended it).
    Exited(i32),
    /// It ran past the timeout; its group was killed.
    TimedOut,
}

/// Runs a CLI into `log` in its own session and process group. On a timeout, an error or Ctrl-C the whole group is
/// killed; Ctrl-C then returns an error so the pass stops.
pub fn run_cli(
    argv: &[String],
    log: &Path,
    env: &[(OsString, OsString)],
    cwd: &Path,
    timeout: Duration,
) -> Result<CliEnd, String> {
    let out = std::fs::File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]).env_clear().envs(env.iter().cloned()).current_dir(cwd);
    command.stdin(Stdio::null()).stdout(out).stderr(err);
    // SAFETY: setsid is async-signal-safe and touches nothing of the parent.
    unsafe {
        command.pre_exec(|| if libc::setsid() == -1 { Err(std::io::Error::last_os_error()) } else { Ok(()) });
    }
    let child = command.spawn().map_err(|e| format!("cannot start {}: {e}", argv[0]))?;
    GROUP.store(child.id() as i32, Ordering::SeqCst);
    let mut group = Group { child, done: false };
    let deadline = Instant::now() + timeout;
    loop {
        if interrupted() {
            group.kill();
            return Err("interrupted".into());
        }
        match group.child.try_wait() {
            Ok(Some(status)) => {
                group.done = true;
                return Ok(CliEnd::Exited(status.code().unwrap_or(-1)));
            }
            Ok(None) if Instant::now() >= deadline => {
                group.kill();
                return Ok(CliEnd::TimedOut);
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(e) => return Err(format!("waiting for {}: {e}", argv[0])), // the drop kills the group
        }
    }
}

/// What one Claude Code call printed.
pub struct Captured {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Reads a pipe to its end on a thread of its own; the bytes arrive on the returned channel.
fn read_all(mut pipe: impl Read + Send + 'static) -> std::sync::mpsc::Receiver<Vec<u8>> {
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        let _ = send.send(buf);
    });
    receive
}

/// Runs a command with `input` on stdin; None when it ran past `timeout` (it is killed). The timeout covers reading
/// its output too: a process it started that keeps the pipes open counts as no reply.
pub fn run_captured(argv: &[String], input: &str, timeout: Duration) -> Result<Option<Captured>, String> {
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", argv[0]))?;
    let mut stdin = child.stdin.take().expect("piped");
    let input = input.to_string();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let out_reader = read_all(child.stdout.take().expect("piped"));
    let err_reader = read_all(child.stderr.take().expect("piped"));
    let deadline = Instant::now() + timeout;
    let status = loop {
        if interrupted() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("interrupted".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => break None,
            Ok(None) => std::thread::sleep(POLL),
            Err(e) => {
                let _ = child.kill();
                return Err(e.to_string());
            }
        }
    };
    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        return Ok(None);
    };
    let left = || deadline.saturating_duration_since(Instant::now());
    let (Ok(stdout), Ok(stderr)) = (out_reader.recv_timeout(left()), err_reader.recv_timeout(left())) else {
        return Ok(None);
    };
    let _ = writer.join();
    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    let stderr = String::from_utf8_lossy(&stderr).into_owned();
    Ok(Some(Captured { code: status.code().unwrap_or(-1), stdout, stderr }))
}
