//! Spawning agent CLIs (the process half of `tellfold/runtimes/cli_tools.py`).
//!
//! Every child runs without a shell in its own process group. The group is killed when the call is cancelled or
//! times out (the future is dropped), and anything the tool left running in it is ended once it exits.
//! [`kill_live_process_groups`] kills every group still running when the engine force-exits.

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::sync::Mutex;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, ChildStdout, Command};

pub const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;

/// The longest stdout line a streaming child may write: claude-agent-sdk's default `max_buffer_size` for one
/// stream-json message.
pub const MAX_STREAM_LINE_BYTES: usize = 1024 * 1024;

/// Process groups of agent CLIs that are running right now (each child leads its own group).
static LIVE_GROUPS: Mutex<Option<HashSet<i32>>> = Mutex::new(None);

pub fn live_process_groups() -> HashSet<i32> {
    LIVE_GROUPS.lock().expect("not poisoned").clone().unwrap_or_default()
}

/// SIGKILL every agent CLI process group still running. Safe to call from any thread, e.g. right before the
/// process exits, when the runtime that would cancel them is stuck.
pub fn kill_live_process_groups() {
    for pgid in live_process_groups() {
        kill_group(pgid);
    }
}

fn kill_group(pgid: i32) {
    if pgid > 0 {
        // SAFETY: killpg only sends a signal; errors (no such group, no permission) are ignored.
        unsafe {
            libc::killpg(pgid, libc::SIGKILL);
        }
    }
}

/// Tracks one live group. Dropped before the child finished (cancellation, timeout): kills the whole group.
struct GroupGuard {
    pgid: i32,
    finished: bool,
}

impl GroupGuard {
    fn new(pgid: i32) -> GroupGuard {
        LIVE_GROUPS.lock().expect("not poisoned").get_or_insert_with(HashSet::new).insert(pgid);
        GroupGuard { pgid, finished: false }
    }

    /// The tool exited. Cursor Agent leaves a node worker behind after every call (about 150 MB each, for
    /// minutes) that ignores SIGTERM, and they pile up over a run. The tool has already answered, so anything
    /// still in its group is killed outright.
    fn finish(&mut self) {
        self.finished = true;
        kill_group(self.pgid);
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if !self.finished {
            kill_group(self.pgid);
        }
        if let Some(live) = LIVE_GROUPS.lock().expect("not poisoned").as_mut() {
            live.remove(&self.pgid);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessResult {
    pub returncode: i32,
    pub stdout: String,
    pub stderr: String,
}

impl ProcessResult {
    pub fn new(returncode: i32, stdout: &str, stderr: &str) -> ProcessResult {
        ProcessResult { returncode, stdout: stdout.into(), stderr: stderr.into() }
    }
}

/// asyncio's `returncode`: the exit code, or minus the signal that ended the process.
pub fn returncode(status: ExitStatus) -> i32 {
    status.code().or_else(|| status.signal().map(|s| -s)).unwrap_or(-1)
}

/// How a CLI adapter starts its tool; tests swap in a fake.
#[async_trait]
pub trait ProcessRunner: Send + Sync {
    async fn run(
        &self,
        argv: &[String],
        stdin: Option<&str>,
        cwd: &Path,
        env: &BTreeMap<String, String>,
    ) -> io::Result<ProcessResult>;
}

/// The real runner: [`run_process`].
pub struct RealRunner;

#[async_trait]
impl ProcessRunner for RealRunner {
    async fn run(
        &self,
        argv: &[String],
        stdin: Option<&str>,
        cwd: &Path,
        env: &BTreeMap<String, String>,
    ) -> io::Result<ProcessResult> {
        run_process(argv, stdin, cwd, env).await
    }
}

fn command(argv: &[String], cwd: &Path, env: &BTreeMap<String, String>, stdin: bool) -> io::Result<Command> {
    let (program, args) =
        argv.split_first().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty command"))?;
    let mut command = Command::new(program);
    command
        .args(args)
        .env_clear()
        .envs(env)
        .current_dir(cwd)
        .stdin(if stdin { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    Ok(command)
}

/// Everything the pipe delivers (reading on keeps the tool from blocking on a full pipe), of which the first
/// [`MAX_OUTPUT_BYTES`] are kept.
async fn read_capped<R: AsyncReadExt + Unpin>(reader: Option<R>) -> Vec<u8> {
    let mut out = Vec::new();
    let Some(mut reader) = reader else { return out };
    let mut chunk = [0u8; 64 * 1024];
    while let Ok(n) = reader.read(&mut chunk).await {
        if n == 0 {
            break;
        }
        let room = MAX_OUTPUT_BYTES.saturating_sub(out.len());
        out.extend_from_slice(&chunk[..n.min(room)]);
    }
    out
}

async fn write_stdin(child: &mut Child, text: Option<&str>) {
    if let (Some(mut pipe), Some(text)) = (child.stdin.take(), text) {
        // a tool that exits without reading its input is not an error here (BrokenPipe is ignored, like
        // asyncio's communicate)
        let _ = pipe.write_all(text.as_bytes()).await;
        let _ = pipe.shutdown().await;
    }
}

/// Spawn without a shell in its own process group; kill the whole group if the call is dropped, and end anything
/// the tool left running in it once it exits.
pub async fn run_process(
    argv: &[String],
    stdin: Option<&str>,
    cwd: &Path,
    env: &BTreeMap<String, String>,
) -> io::Result<ProcessResult> {
    let mut child = command(argv, cwd, env, stdin.is_some())?.spawn()?;
    let mut guard = GroupGuard::new(child.id().map_or(0, |id| id as i32));
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let ((), out, err) = tokio::join!(write_stdin(&mut child, stdin), read_capped(stdout), read_capped(stderr));
    let status = child.wait().await?;
    guard.finish();
    Ok(ProcessResult {
        returncode: returncode(status),
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
    })
}

/// A running child whose stdout is read line by line (Claude Code's stream-json).
pub struct StreamingChild {
    child: Child,
    guard: GroupGuard,
    stdout: tokio::io::BufReader<ChildStdout>,
    stderr: Option<tokio::task::JoinHandle<Vec<u8>>>,
}

impl StreamingChild {
    pub async fn spawn(
        argv: &[String],
        stdin: &str,
        cwd: &Path,
        env: &BTreeMap<String, String>,
    ) -> io::Result<StreamingChild> {
        let mut child = command(argv, cwd, env, true)?.spawn()?;
        let guard = GroupGuard::new(child.id().map_or(0, |id| id as i32));
        let stdout = child.stdout.take().ok_or_else(|| io::Error::other("no stdout pipe"))?;
        let stderr: Option<ChildStderr> = child.stderr.take();
        let stderr = Some(tokio::spawn(read_capped(stderr)));
        write_stdin(&mut child, Some(stdin)).await;
        Ok(StreamingChild { child, guard, stdout: tokio::io::BufReader::new(stdout), stderr })
    }

    /// The next stdout line without its `\n` (or `\r\n`), None at the end. A line longer than
    /// [`MAX_STREAM_LINE_BYTES`] is an `InvalidData` error, like the SDK's buffer limit.
    pub async fn next_line(&mut self) -> io::Result<Option<String>> {
        use tokio::io::AsyncBufReadExt;
        let mut line = Vec::new();
        loop {
            let available = self.stdout.fill_buf().await?;
            if available.is_empty() {
                if line.is_empty() {
                    return Ok(None);
                }
                break;
            }
            let (taken, done) = match available.iter().position(|&b| b == b'\n') {
                Some(i) => (i + 1, true),
                None => (available.len(), false),
            };
            line.extend_from_slice(&available[..taken]);
            self.stdout.consume(taken);
            if line.len() > MAX_STREAM_LINE_BYTES + 1 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("JSON message exceeded maximum buffer size of {MAX_STREAM_LINE_BYTES} bytes"),
                ));
            }
            if done {
                break;
            }
        }
        if line.last() == Some(&b'\n') {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
        }
        String::from_utf8(line).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// Waits for the exit and ends the group; returns the exit code and what the tool wrote to stderr.
    pub async fn finish(mut self) -> io::Result<(i32, String)> {
        let status = self.child.wait().await?;
        self.guard.finish();
        let err = match self.stderr.take() {
            Some(task) => task.await.unwrap_or_default(),
            None => Vec::new(),
        };
        Ok((returncode(status), String::from_utf8_lossy(&err).into_owned()))
    }
}
