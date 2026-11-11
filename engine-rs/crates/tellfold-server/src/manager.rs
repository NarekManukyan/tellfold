//! In-process runs, server-sent-event fan-out, and a watcher for runs started elsewhere (e.g. by the CLI): the port
//! of `tellfold/server/manager.py`.
//!
//! The manager lives on one thread with a single-threaded tokio runtime, like the Python server's event loop: the
//! engine's futures hold SQLite connections and are not `Send`. HTTP handlers reach it through a [`Handle`], which
//! runs a closure on that thread and sends its result back. A live run is a local task owned by the manager, never by
//! a request, so it survives every client disconnecting. Each SSE client gets its own bounded [`Queue`]; events from
//! `run_pipeline`'s `on_event` are serialised once and pushed to every queue of that run.
//!
//! For runs this server is not executing, a per-run watcher task (alive only while someone is subscribed) polls
//! `status.json` and `world.db` once per `poll_interval` and synthesises `status`, `posts_added`, `round_done` and
//! terminal events, so watching a CLI run live works too.
//!
//! An optional read-only examples root (the Mac app's bundled example runs) is listed next to the normal runs. Each
//! example's id is `example-<folder name>`; examples can be read, streamed and (with an AI tool set up) interviewed,
//! but never changed: they are opened with SQLite's `immutable` flag and nothing is ever written next to them.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::{BoxFuture, LocalBoxFuture};
use futures::FutureExt;
use serde_json::{json, Value};
use std::panic::AssertUnwindSafe;
use tokio::sync::{mpsc, oneshot, watch, Notify};
use tokio::task::JoinHandle;

use tellfold_engine::personas::{generate_group, tidy_cast};
use tellfold_engine::runner::{run_pipeline, CastGate, RunOptions};
use tellfold_llm::{Llm, LlmError};
use tellfold_models::json::{py_dumps_compact, to_string as model_json};
use tellfold_models::{
    Agreement, CompareResult, GodEvent, KnowledgeGraph, Persona, RewriteSuggestion, RunStage, RunState, RunStatus,
    SimConfig, Study, StudyKind, StudyRun,
};
use tellfold_report::chat::{ask_report, interview};
use tellfold_report::report::build_report;
use tellfold_report::rewrite::{build_rewrite, REWRITE_FILE};
use tellfold_report::{write_report, ReportError, ReportWriter};
use tellfold_runtimes::load_settings;
use tellfold_store::api::ReportRebuild;
use tellfold_store::files::{copy_seeds, create_run_dir, now_iso, read_model, read_status, write_json, write_status};
use tellfold_store::study::{
    join_study, letter, load_study, new_study_id, plan_variant, repeat_config, save_study, study_dir, AGREEMENT_FILE,
    COMPARE_FILE,
};
use tellfold_store::{RunReader, World};
use tellfold_study::agree::{build_agreement, MAX_RUNS};
use tellfold_study::branch::{prepare_branch, BranchOptions};
use tellfold_study::compare::{build_compare, Member};

use crate::log;

pub const EXAMPLE_PREFIX: &str = "example-";
pub const QUEUE_SIZE: usize = 2000;
pub const EXAMPLE_READ_ONLY: &str =
    "this is a built-in example run; it cannot be changed. Use Run again to make your own copy";
const LOGGER: &str = "tellfold.server.manager";

/// `llm_factory(run_dir, concurrency, think)`: the model backend for a run folder (None: no call log).
pub type LlmFactory = Arc<dyn Fn(Option<&Path>, i64, bool) -> Result<Arc<dyn Llm>, LlmError> + Send + Sync>;
/// Whether an AI tool is set up on this machine, which interviewing an example needs.
pub type RuntimeReady = Arc<dyn Fn() -> BoxFuture<'static, bool> + Send + Sync>;

/// Why a manager call was refused; the routes turn these into HTTP errors.
#[derive(Debug, Clone)]
pub enum ManagerError {
    /// The request is valid but the run is not in a state that allows it (409).
    Conflict(String),
    /// Invalid input for the run (400).
    BadRequest(String),
    /// The run is a built-in example, which cannot be changed (403).
    ReadOnly(String),
    /// An interview with someone the run does not have (404).
    UnknownAgent,
    /// A model call failed (502).
    Llm(String),
    /// Anything else (500).
    Internal(String),
}

fn conflict(message: impl Into<String>) -> ManagerError {
    ManagerError::Conflict(message.into())
}

fn internal(error: impl std::fmt::Display) -> ManagerError {
    ManagerError::Internal(error.to_string())
}

/// What the manager is configured with (`RunManager`'s fields).
#[derive(Clone)]
pub struct ManagerOptions {
    pub runs_dir: PathBuf,
    pub llm_factory: LlmFactory,
    pub poll_interval: Duration,
    pub shutdown_grace: Duration,
    /// read-only example runs, listed as `example-<folder>`
    pub examples_dir: Option<PathBuf>,
    /// whose runtimes and models interviews of examples use
    pub settings_path: Option<PathBuf>,
    /// whether an AI tool is set up; None: assume it is
    pub runtime_ready: Option<RuntimeReady>,
}

impl ManagerOptions {
    pub fn new(runs_dir: &Path, llm_factory: LlmFactory) -> Self {
        ManagerOptions {
            runs_dir: runs_dir.to_path_buf(),
            llm_factory,
            poll_interval: Duration::from_secs(1),
            shutdown_grace: Duration::from_secs(5),
            examples_dir: None,
            settings_path: None,
            runtime_ready: None,
        }
    }
}

// ---------- client queues ----------

/// One SSE client's bounded queue of `(event name, data)`; `None` ends the stream (the server is shutting down).
#[derive(Default)]
pub struct Queue {
    items: Mutex<VecDeque<Option<(String, String)>>>,
    notify: Notify,
}

impl Queue {
    /// Never blocks the run: a client too slow to keep up loses its oldest events, not the newest.
    pub fn put(&self, item: Option<(String, String)>) {
        let mut items = self.items.lock().expect("not poisoned");
        while items.len() >= QUEUE_SIZE {
            items.pop_front();
        }
        items.push_back(item);
        drop(items);
        self.notify.notify_one();
    }

    pub async fn get(&self) -> Option<(String, String)> {
        loop {
            if let Some(item) = self.items.lock().expect("not poisoned").pop_front() {
                return item;
            }
            self.notify.notified().await;
        }
    }

    pub fn len(&self) -> usize {
        self.items.lock().expect("not poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ---------- the manager thread ----------

type Job = Box<dyn FnOnce(Rc<Manager>) -> LocalBoxFuture<'static, ()> + Send>;

/// A job that did not answer: the manager thread is gone (the server is shutting down), or the job panicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closed {
    ShuttingDown,
    Panicked,
}

/// Runs closures on the manager's thread. Cheap to clone.
#[derive(Clone)]
pub struct Handle {
    jobs: mpsc::UnboundedSender<Job>,
}

impl Handle {
    /// Runs `f` on the manager thread as its own task and waits for its result.
    pub async fn call<R, F, Fut>(&self, f: F) -> Result<R, Closed>
    where
        R: Send + 'static,
        F: FnOnce(Rc<Manager>) -> Fut + Send + 'static,
        Fut: Future<Output = R> + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let job: Job = Box::new(move |manager| {
            async move {
                // a panicking job answers Panicked instead of dropping its reply
                let result = AssertUnwindSafe(f(manager)).catch_unwind().await.map_err(|_| Closed::Panicked);
                let _ = tx.send(result);
            }
            .boxed_local()
        });
        self.jobs.send(job).map_err(|_| Closed::ShuttingDown)?;
        rx.await.map_err(|_| Closed::ShuttingDown)?
    }

    /// Runs `f` on the manager thread without waiting (e.g. from a `Drop`).
    pub fn spawn<F>(&self, f: F)
    where
        F: FnOnce(Rc<Manager>) + Send + 'static,
    {
        let job: Job = Box::new(move |manager| {
            f(manager);
            async {}.boxed_local()
        });
        let _ = self.jobs.send(job);
    }
}

/// The manager's thread; [`ManagerThread::shutdown`] stops its runs and ends it.
pub struct ManagerThread {
    pub handle: Handle,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ManagerThread {
    /// `RunManager.shutdown()`, then the thread ends (tasks still running are dropped).
    pub async fn shutdown(mut self) {
        let _ = self.handle.call(|m| async move { m.shutdown().await }).await;
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = tokio::task::spawn_blocking(move || thread.join()).await;
        }
    }
}

/// Starts the manager on its own thread.
pub fn spawn_manager(options: ManagerOptions) -> ManagerThread {
    let (jobs, mut rx) = mpsc::unbounded_channel::<Job>();
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    let thread = std::thread::Builder::new()
        .name("tellfold-manager".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a tokio runtime for the run manager");
            let local = tokio::task::LocalSet::new();
            local.block_on(&runtime, async move {
                let manager = Manager::new(options);
                loop {
                    tokio::select! {
                        job = rx.recv() => match job {
                            Some(job) => {
                                tokio::task::spawn_local(job(manager.clone()));
                            }
                            None => break,
                        },
                        _ = &mut stop_rx => break,
                    }
                }
            });
        })
        .expect("the run manager thread");
    ManagerThread { handle: Handle { jobs }, stop: Some(stop_tx), thread: Some(thread) }
}

// ---------- state ----------

/// A `review_cast` run's cast while it waits at `personas_ready`.
#[derive(Default)]
struct CastHold {
    /// Some while the run waits
    personas: RefCell<Option<Vec<Persona>>>,
    /// set when a client confirms the cast
    go: Cell<bool>,
    wake: Notify,
}

/// The run's side of a [`CastHold`]: waits until the cast is confirmed or the run is stopped.
struct HoldGate {
    hold: Rc<CastHold>,
    stop: Arc<AtomicBool>,
}

#[async_trait::async_trait(?Send)]
impl CastGate for HoldGate {
    async fn review(&self, personas: Vec<Persona>) -> Option<Vec<Persona>> {
        *self.hold.personas.borrow_mut() = Some(personas);
        loop {
            if self.stop.load(Ordering::SeqCst) {
                self.hold.personas.borrow_mut().take();
                return None;
            }
            if self.hold.go.get() {
                return self.hold.personas.borrow_mut().take();
            }
            // a notify_one() before this await leaves a permit, so no wake-up is lost
            self.hold.wake.notified().await;
        }
    }
}

struct LiveRun {
    run_dir: PathBuf,
    config: SimConfig,
    stop: Arc<AtomicBool>,
    hold: Rc<CastHold>,
    /// highest round that has started
    current_round: Rc<Cell<i64>>,
    task: Option<JoinHandle<()>>,
    done: watch::Receiver<bool>,
}

struct Watch {
    task: JoinHandle<()>,
    last_post_id: Option<i64>,
    last_db: Option<(f64, f64, u64)>,
    last_round: Option<i64>,
    last_state: Option<RunState>,
}

/// The event a run written by another process ends with, by its new state (Python's `_TERMINAL_EVENTS`).
const TERMINAL_EVENTS: [(RunState, &str); 3] =
    [(RunState::Done, "run_done"), (RunState::Failed, "run_failed"), (RunState::Stopped, "run_stopped")];

impl Watch {
    /// `posts_added`, one per round, for posts a run written elsewhere added since the last poll.
    fn posts_added(&mut self, reader: &RunReader) -> Vec<(&'static str, Value)> {
        let signature = reader.db_signature();
        let Some(last) = self.last_post_id else {
            self.last_post_id = Some(reader.max_post_id());
            self.last_db = Some(signature);
            return Vec::new();
        };
        if self.last_db == Some(signature) {
            return Vec::new();
        }
        self.last_db = Some(signature);
        let new = reader.new_post_ids(last);
        let Some((_, newest)) = new.last() else { return Vec::new() };
        self.last_post_id = Some(*newest);
        let mut by_round: Vec<(i64, Vec<i64>)> = Vec::new();
        for (round, id) in new {
            match by_round.iter_mut().find(|(r, _)| *r == round) {
                Some((_, ids)) => ids.push(id),
                None => by_round.push((round, vec![id])),
            }
        }
        by_round.into_iter().map(|(round, ids)| ("posts_added", json!({"round": round, "post_ids": ids}))).collect()
    }

    /// `round_done` for each round completed elsewhere since the last poll.
    fn rounds_done(&mut self, reader: &RunReader, status: &RunStatus) -> Vec<(&'static str, Value)> {
        let Some(last) = self.last_round else {
            self.last_round = Some(status.round);
            self.last_state = Some(status.state); // just finished in-process: its own events were sent
            return Vec::new();
        };
        if status.round <= last {
            return Vec::new();
        }
        self.last_round = Some(status.round);
        let stats = reader.round_stats();
        (last + 1..=status.round)
            .filter_map(|round| stats.iter().find(|s| s.round == round))
            .map(|s| {
                let mut data = serde_json::to_value(s).unwrap_or_else(|_| json!({}));
                data["total_rounds"] = json!(status.total_rounds);
                ("round_done", data)
            })
            .collect()
    }

    /// The terminal event when a run seen running has ended.
    fn terminal_event(&self, status: &RunStatus) -> Option<(&'static str, Value)> {
        if !matches!(self.last_state, Some(RunState::Running | RunState::Queued)) {
            return None;
        }
        let (_, name) = TERMINAL_EVENTS.iter().find(|(state, _)| *state == status.state)?;
        let data = match status.state {
            RunState::Failed => json!({"error": status.error.clone().unwrap_or_default()}),
            _ => json!({}),
        };
        Some((name, data))
    }
}

#[derive(Default)]
struct State {
    live: HashMap<String, LiveRun>,
    subs: HashMap<String, Vec<Arc<Queue>>>,
    watches: HashMap<String, Watch>,
    /// what the server last told clients about a run's status, so polling only sends changes
    sent: HashMap<String, String>,
    rebuilding: HashMap<String, JoinHandle<()>>,
    /// why the last rebuild failed
    rebuild_errors: HashMap<String, String>,
    /// study id -> compare task
    comparing: HashMap<String, JoinHandle<()>>,
    /// why the last comparison failed
    compare_errors: HashMap<String, String>,
    /// study id -> "Check it" series
    repeating: HashMap<String, JoinHandle<()>>,
    /// examples are read-only, so their suggestions are kept here
    example_rewrites: HashMap<String, RewriteSuggestion>,
    /// set by shutdown(): no new run starts
    closing: bool,
}

pub struct Manager {
    pub runs_dir: PathBuf,
    pub examples_dir: Option<PathBuf>,
    pub settings_path: Option<PathBuf>,
    llm_factory: LlmFactory,
    runtime_ready: Option<RuntimeReady>,
    poll_interval: Duration,
    shutdown_grace: Duration,
    me: Weak<Manager>,
    st: RefCell<State>,
}

/// Removes a finished (or cancelled) run from the live set and tells waiters it ended.
struct LiveGuard {
    manager: Weak<Manager>,
    run_id: String,
    done: Option<watch::Sender<bool>>,
}

impl Drop for LiveGuard {
    fn drop(&mut self) {
        if let Some(manager) = self.manager.upgrade() {
            manager.st.borrow_mut().live.remove(&self.run_id);
            manager.refresh_status(&self.run_id);
        }
        if let Some(done) = self.done.take() {
            let _ = done.send(true);
        }
    }
}

impl Manager {
    pub fn new(options: ManagerOptions) -> Rc<Manager> {
        Rc::new_cyclic(|me| Manager {
            runs_dir: options.runs_dir,
            examples_dir: options.examples_dir,
            settings_path: options.settings_path,
            llm_factory: options.llm_factory,
            runtime_ready: options.runtime_ready,
            poll_interval: options.poll_interval,
            shutdown_grace: options.shutdown_grace,
            me: me.clone(),
            st: RefCell::new(State::default()),
        })
    }

    fn rc(&self) -> Rc<Manager> {
        self.me.upgrade().expect("the manager is alive while it runs")
    }

    fn llm(&self, run_dir: Option<&Path>, concurrency: i64, think: bool) -> Result<Arc<dyn Llm>, ManagerError> {
        (self.llm_factory)(run_dir, concurrency, think).map_err(|e| ManagerError::Internal(e.0))
    }

    // ---------- lookup ----------

    pub fn is_live(&self, run_id: &str) -> bool {
        self.st.borrow().live.contains_key(run_id)
    }

    pub fn live_ids(&self) -> HashSet<String> {
        self.st.borrow().live.keys().cloned().collect()
    }

    /// Example id -> folder, for every folder in the examples root that holds a run.
    pub fn examples(&self) -> BTreeMap<String, PathBuf> {
        let Some(root) = self.examples_dir.as_ref().filter(|r| r.is_dir()) else { return BTreeMap::new() };
        let Ok(entries) = std::fs::read_dir(root) else { return BTreeMap::new() };
        entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                p.is_dir() && !name.starts_with('.') && p.join("config.json").is_file()
            })
            .map(|p| (format!("{EXAMPLE_PREFIX}{}", p.file_name().unwrap_or_default().to_string_lossy()), p))
            .collect()
    }

    pub fn is_example(&self, run_id: &str) -> bool {
        run_id.starts_with(EXAMPLE_PREFIX) && self.examples().contains_key(run_id)
    }

    /// The folder of a run: an example's folder for an example id, else `runs_dir/run_id`.
    pub fn run_path(&self, run_id: &str) -> PathBuf {
        if run_id.starts_with(EXAMPLE_PREFIX) {
            if let Some(path) = self.examples().remove(run_id) {
                return path;
            }
        }
        self.runs_dir.join(run_id)
    }

    pub fn reader(&self, run_id: &str) -> RunReader {
        if self.is_example(run_id) {
            return RunReader::new(&self.run_path(run_id), Some(run_id), true);
        }
        RunReader::new(&self.runs_dir.join(run_id), None, false)
    }

    fn writable(&self, run_id: &str) -> Result<(), ManagerError> {
        if self.is_example(run_id) {
            return Err(ManagerError::ReadOnly(EXAMPLE_READ_ONLY.into()));
        }
        Ok(())
    }

    pub fn effective_status(&self, run_id: &str) -> Option<RunStatus> {
        let status = self.reader(run_id).status(self.is_live(run_id));
        if status.is_some() {
            return status;
        }
        let st = self.st.borrow();
        st.live.get(run_id).map(|live| RunStatus {
            state: RunState::Queued,
            stage: RunStage::Reading,
            round: 0,
            total_rounds: live.config.rounds,
            detail: String::new(),
            error: None,
            pid: None,
            started_at: now_iso(),
            updated_at: now_iso(),
            failed_batches: Vec::new(),
            pause: None,
        })
    }

    // ---------- starting / controlling runs ----------

    pub fn create_run_dir(&self, question: &str) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(&self.runs_dir)?;
        create_run_dir(&self.runs_dir, question)
    }

    /// Start `config` in `run_dir` (already created, seeds already saved) as a background task.
    ///
    /// With `config.cast_from` set, the cast is read from that run's folder (an example's too).
    pub fn start(&self, run_dir: &Path, config: SimConfig, seeds: Vec<PathBuf>) -> Result<String, ManagerError> {
        let run_id = run_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        write_json(&run_dir.join("config.json"), &config).map_err(internal)?;
        let llm = self.llm(Some(run_dir), config.concurrency, config.think)?;
        let stop = Arc::new(AtomicBool::new(false));
        let current_round = Rc::new(Cell::new(0));
        let (done_tx, done_rx) = watch::channel(false);
        let cast_dir = config.cast_from.as_deref().map(|c| self.run_path(c));
        let hold = Rc::new(CastHold::default());
        self.st.borrow_mut().live.insert(
            run_id.clone(),
            LiveRun {
                run_dir: run_dir.to_path_buf(),
                config: config.clone(),
                stop: stop.clone(),
                hold: hold.clone(),
                current_round: current_round.clone(),
                task: None,
                done: done_rx,
            },
        );
        let guard = LiveGuard { manager: self.me.clone(), run_id: run_id.clone(), done: Some(done_tx) };
        let task = tokio::task::spawn_local(self.rc().execute(
            run_id.clone(),
            run_dir.to_path_buf(),
            config,
            seeds,
            cast_dir,
            llm,
            stop,
            hold,
            current_round,
            guard,
        ));
        if let Some(live) = self.st.borrow_mut().live.get_mut(&run_id) {
            live.task = Some(task);
        }
        Ok(run_id)
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute(
        self: Rc<Self>,
        run_id: String,
        run_dir: PathBuf,
        config: SimConfig,
        seeds: Vec<PathBuf>,
        cast_dir: Option<PathBuf>,
        llm: Arc<dyn Llm>,
        stop: Arc<AtomicBool>,
        hold: Rc<CastHold>,
        current_round: Rc<Cell<i64>>,
        guard: LiveGuard,
    ) {
        let gate = HoldGate { hold, stop: stop.clone() };
        let me = Rc::downgrade(&self);
        let id = run_id.clone();
        let on_event = move |name: &str, data: &Value| {
            if name == "round_start" {
                if let Some(round) = data.get("round").and_then(Value::as_i64) {
                    current_round.set(current_round.get().max(round));
                }
            }
            if let Some(manager) = me.upgrade() {
                manager.dispatch(&id, name, data);
            }
        };
        let result = run_pipeline(
            llm.as_ref(),
            &run_dir,
            &config,
            &seeds,
            &ReportWriter,
            RunOptions {
                on_event: Some(&on_event),
                stop: Some(&stop),
                cast_dir: cast_dir.as_deref(),
                cast_gate: Some(&gate),
            },
        )
        .await;
        if let Err(e) = result {
            // the pipeline already recorded the failure in status.json
            log::warning(LOGGER, &format!("run {run_id} failed: {e}"));
        }
        drop(guard);
        if let Some(study_id) = config.study_id.as_deref().filter(|s| !s.is_empty()) {
            self.compare_when_ready(study_id);
        }
    }

    fn dispatch(&self, run_id: &str, name: &str, data: &Value) {
        if name == "status" {
            if let Ok(status) = serde_json::from_value::<RunStatus>(data.clone()) {
                self.send_status(run_id, &status);
            }
            return;
        }
        self.publish(run_id, name, data);
    }

    pub fn inject_event(&self, run_id: &str, round: i64, content: &str) -> Result<(), ManagerError> {
        self.writable(run_id)?;
        let (run_dir, rounds, seed, current) = {
            let st = self.st.borrow();
            let Some(live) = st.live.get(run_id) else {
                return Err(conflict("events can only be injected into a run that is live in this server"));
            };
            (live.run_dir.clone(), live.config.rounds, live.config.seed, live.current_round.get())
        };
        let content = tellfold_models::text::py_strip(content);
        if content.is_empty() {
            return Err(ManagerError::BadRequest("event content is empty".into()));
        }
        if !(1..=rounds).contains(&round) {
            return Err(ManagerError::BadRequest(format!("round must be between 1 and {rounds}")));
        }
        let status = self.effective_status(run_id);
        let current = current.max(status.map_or(0, |s| s.round));
        if round <= current {
            return Err(conflict(format!("round {round} has already started (current round is {current})")));
        }
        let world = World::open(&run_dir.join("world.db"), seed).map_err(internal)?;
        let added = world.add_event(&GodEvent::new(round, content)).map_err(internal);
        let _ = world.close();
        added
    }

    pub fn stop(&self, run_id: &str) -> Result<(), ManagerError> {
        self.writable(run_id)?;
        let st = self.st.borrow();
        let Some(live) = st.live.get(run_id) else {
            return Err(conflict("only a run that is live in this server can be stopped"));
        };
        live.stop.store(true, Ordering::SeqCst);
        live.hold.wake.notify_one();
        Ok(())
    }

    // ---------- the cast of a run waiting at personas_ready ----------

    /// The hold of a run that waits for its cast to be checked; 409 otherwise.
    fn waiting(&self, run_id: &str) -> Result<(Rc<CastHold>, PathBuf, SimConfig), ManagerError> {
        self.writable(run_id)?;
        let st = self.st.borrow();
        match st.live.get(run_id) {
            Some(live) if live.hold.personas.borrow().is_some() && !live.hold.go.get() => {
                Ok((live.hold.clone(), live.run_dir.clone(), live.config.clone()))
            }
            _ => Err(conflict("this run is not waiting for its cast to be checked")),
        }
    }

    /// `{waiting, personas}`: the cast under review while the run waits, else the run's people as written.
    pub fn cast(&self, run_id: &str) -> Value {
        if let Ok((hold, _, _)) = self.waiting(run_id) {
            let personas = hold.personas.borrow().clone().unwrap_or_default();
            return json!({"waiting": true, "personas": personas});
        }
        json!({"waiting": false, "personas": self.reader(run_id).personas()})
    }

    /// Replaces the cast of a waiting run (tidied: ids renumbered, handles made unique) and returns it.
    pub fn replace_cast(&self, run_id: &str, personas: Vec<Persona>) -> Result<Vec<Persona>, ManagerError> {
        let (hold, run_dir, _) = self.waiting(run_id)?;
        let personas = tidy_cast(personas);
        if personas.is_empty() {
            return Err(ManagerError::BadRequest("the cast needs at least one person".into()));
        }
        tellfold_store::files::write_json_list(&run_dir.join("personas.json"), &personas).map_err(internal)?;
        *hold.personas.borrow_mut() = Some(personas.clone());
        Ok(personas)
    }

    /// Lets a waiting run go on with its (possibly edited) cast.
    pub fn confirm_cast(&self, run_id: &str) -> Result<(), ManagerError> {
        let (hold, _, _) = self.waiting(run_id)?;
        hold.go.set(true);
        hold.wake.notify_one();
        Ok(())
    }

    /// Writes `count` people for a group described in words, adds them to the waiting cast, and returns the cast.
    pub async fn add_group(&self, run_id: &str, description: &str, count: usize) -> Result<Vec<Persona>, ManagerError> {
        let (hold, run_dir, config) = self.waiting(run_id)?;
        let graph: KnowledgeGraph = read_model(&run_dir.join("graph.json")).unwrap_or_default();
        let existing = hold.personas.borrow().clone().unwrap_or_default();
        let llm = self.llm(Some(&run_dir), config.concurrency, config.think)?;
        let group = generate_group(llm.as_ref(), &graph, &config, description, count, &existing)
            .await
            .map_err(|e| ManagerError::Llm(e.0))?;
        // the cast may have been edited while the model wrote
        let (hold, _, _) = self.waiting(run_id)?;
        let mut personas = hold.personas.borrow().clone().unwrap_or_default();
        personas.extend(group);
        self.replace_cast(run_id, personas)
    }

    // ---------- chat, ask, report ----------

    fn open_world(&self, run_id: &str) -> Result<(SimConfig, World), ManagerError> {
        let config = self.reader(run_id).config();
        let db = self.run_path(run_id).join("world.db");
        let Some(config) = config.filter(|_| db.is_file()) else {
            return Err(conflict("this run has no simulated world yet"));
        };
        let world = if self.is_example(run_id) {
            World::open_read_only(&db, config.seed)
        } else {
            World::open(&db, config.seed)
        }
        .map_err(internal)?;
        Ok((config, world))
    }

    /// The model for interviews and follow-up questions, and the config that names its models.
    ///
    /// A normal run uses its own runtimes. An example uses the AI tool set up on this machine (its recorded runtime
    /// may not be installed here), logs no calls (its folder is read-only), and needs a tool at all.
    async fn talker(&self, run_id: &str, config: SimConfig) -> Result<(Arc<dyn Llm>, SimConfig), ManagerError> {
        if !self.is_example(run_id) {
            return Ok((self.llm(Some(&self.runs_dir.join(run_id)), 1, false)?, config));
        }
        let config = self.local_tool(config, "to interview people or ask questions in an example run").await?;
        Ok((self.llm(None, 1, false)?, config))
    }

    /// An example's config with the AI tool and models set up on this machine instead of the recorded ones.
    /// Conflict when no tool is set up; `purpose` finishes that message.
    async fn local_tool(&self, config: SimConfig, purpose: &str) -> Result<SimConfig, ManagerError> {
        if let Some(ready) = &self.runtime_ready {
            if !ready().await {
                return Err(conflict(format!("set up an AI tool first {purpose}")));
            }
        }
        let settings = load_settings(self.settings_path.as_deref());
        Ok(SimConfig {
            agent_model: if settings.agent_model.is_empty() {
                config.agent_model.clone()
            } else {
                settings.agent_model
            },
            planner_model: if settings.planner_model.is_empty() {
                config.planner_model.clone()
            } else {
                settings.planner_model
            },
            agent_runtime: settings.agent_runtime,
            planner_runtime: settings.planner_runtime,
            ..config
        })
    }

    pub async fn chat(
        &self,
        run_id: &str,
        agent_id: &str,
        message: &str,
        history: &[(String, String)],
    ) -> Result<String, ManagerError> {
        let (config, world) = self.open_world(run_id)?;
        let personas = world.personas().map_err(internal)?;
        if !personas.iter().any(|p| p.id == agent_id) {
            return Err(ManagerError::UnknownAgent);
        }
        let (llm, config) = self.talker(run_id, config).await?;
        let reply = interview(llm.as_ref(), &world, agent_id, message, &config.agent_model, history).await;
        let _ = world.close();
        reply.map_err(report_error)
    }

    pub async fn ask(&self, run_id: &str, question: &str) -> Result<String, ManagerError> {
        let Some(report) = self.reader(run_id).report() else { return Err(conflict("this run has no report yet")) };
        let (config, world) = self.open_world(run_id)?;
        let (llm, config) = self.talker(run_id, config).await?;
        let reply = ask_report(llm.as_ref(), &world, &report, question, &config.planner_model).await;
        let _ = world.close();
        reply.map_err(report_error)
    }

    /// Copy a run up to the end of `round`, give it new events after that, and continue it as a new run in a study
    /// with its source. Returns (new run id, study id).
    pub async fn branch(
        &self,
        source_id: &str,
        round: i64,
        events: Vec<GodEvent>,
    ) -> Result<(String, String), ManagerError> {
        if self.is_live(source_id) {
            return Err(conflict("the run is still in progress; branch it once it has finished"));
        }
        let Some(mut source) = self.reader(source_id).config() else {
            return Err(conflict("this run has no settings to branch from"));
        };
        let example = self.is_example(source_id);
        if example {
            source = self.local_tool(source, "to branch an example run").await?;
        }
        let label = format!("Branch at round {round}");
        let (study, label) =
            plan_variant(&self.runs_dir, source_id, &source, None, Some(&label), &now_iso(), Some("Original"))
                .map_err(|e| ManagerError::BadRequest(e.to_string()))?;
        let run_dir = self.create_run_dir(&source.question).map_err(internal)?;
        let options = BranchOptions {
            source_id: Some(source_id),
            immutable: example,
            study_id: Some(&study.id),
            variant: Some(&label),
        };
        let config = match prepare_branch(&self.run_path(source_id), &run_dir, &source, round, &events, options) {
            Ok(config) => config,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&run_dir);
                return Err(match e {
                    tellfold_study::Error::Run(e) => ManagerError::BadRequest(e.0),
                    other => internal(other),
                });
            }
        };
        let run_name = run_dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
        join_study(&self.runs_dir, &study, &run_name, &label).map_err(internal)?;
        let seeds: Vec<PathBuf> = config.seed_files.iter().map(PathBuf::from).filter(|p| p.is_file()).collect();
        self.start(&run_dir, config, seeds)?;
        Ok((run_name, study.id))
    }

    /// Suggest a better version of the run's text (one `report.rewrite` call) and keep it in rewrite.json.
    pub async fn rewrite(&self, run_id: &str) -> Result<RewriteSuggestion, ManagerError> {
        if self.is_live(run_id) {
            return Err(conflict("the run is still in progress"));
        }
        let reader = self.reader(run_id);
        let (Some(report), Some(config)) = (reader.report(), reader.config()) else {
            return Err(conflict("this run has no report yet"));
        };
        drop(reader);
        let (llm, config) = self.talker(run_id, config).await?;
        let example = self.is_example(run_id);
        let suggestion =
            build_rewrite(llm.as_ref(), &self.run_path(run_id), &config, &report, &config.planner_model, example)
                .await
                .map_err(|e| match e {
                    ReportError::Rewrite(message) => ManagerError::BadRequest(message),
                    other => report_error(other),
                })?;
        if example {
            self.st.borrow_mut().example_rewrites.insert(run_id.into(), suggestion.clone());
        } else {
            write_json(&self.run_path(run_id).join(REWRITE_FILE), &suggestion).map_err(internal)?;
        }
        Ok(suggestion)
    }

    pub fn saved_rewrite(&self, run_id: &str) -> Option<RewriteSuggestion> {
        if self.is_example(run_id) {
            return self.st.borrow().example_rewrites.get(run_id).cloned();
        }
        read_model(&self.run_path(run_id).join(REWRITE_FILE))
    }

    pub fn rebuild_report(&self, run_id: &str) -> Result<(), ManagerError> {
        self.writable(run_id)?;
        if self.is_live(run_id) {
            return Err(conflict("the run is still in progress; it writes its own report"));
        }
        if running(&self.st.borrow().rebuilding, run_id) {
            return Err(conflict("the report is already being rebuilt"));
        }
        let (config, world) = self.open_world(run_id)?;
        let _ = world.close();
        let task = tokio::task::spawn_local(self.rc().rebuild(run_id.to_string(), config));
        let mut st = self.st.borrow_mut();
        st.rebuild_errors.remove(run_id);
        st.rebuilding.insert(run_id.into(), task);
        Ok(())
    }

    async fn rebuild(self: Rc<Self>, run_id: String, config: SimConfig) {
        let run_dir = self.runs_dir.join(&run_id);
        let graph = self.reader(&run_id).graph().unwrap_or_default();
        let outcome: Result<(), String> = async {
            let world = World::open(&run_dir.join("world.db"), config.seed).map_err(|e| e.to_string())?;
            let result = async {
                let llm = self.llm(Some(&run_dir), config.concurrency, config.think).map_err(|e| match e {
                    ManagerError::Internal(m) => m,
                    other => format!("{other:?}"),
                })?;
                let report = build_report(llm.as_ref(), &world, &graph, &config, &config.planner_model)
                    .await
                    .map_err(|e| error_text(&e))?;
                write_report(&run_dir, &report, &world, &config).map_err(|e| error_text(&e))?;
                Ok(())
            }
            .await;
            let _ = world.close();
            result
        }
        .await;
        match outcome {
            Ok(()) => self.publish(&run_id, "report_rebuilt", &json!({"ok": true})),
            Err(error) => {
                log::warning(LOGGER, &format!("report rebuild for {run_id} failed: {error}"));
                self.st.borrow_mut().rebuild_errors.insert(run_id.clone(), error.clone());
                self.publish(&run_id, "report_rebuilt", &json!({"ok": false, "error": error}));
            }
        }
        self.st.borrow_mut().rebuilding.remove(&run_id);
    }

    /// Whether a report rebuild is running, and why the last one failed (None if it did not).
    pub fn rebuild_state(&self, run_id: &str) -> ReportRebuild {
        let st = self.st.borrow();
        ReportRebuild { running: running(&st.rebuilding, run_id), error: st.rebuild_errors.get(run_id).cloned() }
    }

    // ---------- studies ----------

    pub fn study(&self, study_id: &str) -> Option<Study> {
        load_study(&self.runs_dir, study_id)
    }

    /// The study's runs that still exist, lettered in study order (A is the first run ever added).
    pub fn study_members(&self, study: &Study) -> Vec<Member> {
        let mut members = Vec::new();
        for (i, run) in study.runs.iter().enumerate() {
            let path = self.run_path(&run.id);
            if path.join("config.json").is_file() {
                let mut member = Member::new(&letter(i as i64), &run.id, &run.variant, &path);
                member.immutable = self.is_example(&run.id);
                members.push(member);
            }
        }
        members
    }

    pub fn run_state(&self, run_id: &str) -> Option<RunState> {
        self.effective_status(run_id).map(|s| s.state)
    }

    pub fn compare_result(&self, study_id: &str) -> Option<CompareResult> {
        read_model(&study_dir(&self.runs_dir, study_id).ok()?.join(COMPARE_FILE))
    }

    pub fn agreement(&self, study_id: &str) -> Option<Agreement> {
        read_model(&study_dir(&self.runs_dir, study_id).ok()?.join(AGREEMENT_FILE))
    }

    /// `{running, error}` of the study's comparison in this server.
    pub fn compare_state(&self, study_id: &str) -> Value {
        let st = self.st.borrow();
        json!({"running": running(&st.comparing, study_id), "error": st.compare_errors.get(study_id)})
    }

    pub fn is_comparing(&self, study_id: &str) -> bool {
        running(&self.st.borrow().comparing, study_id)
    }

    pub fn is_repeating(&self, study_id: &str) -> bool {
        running(&self.st.borrow().repeating, study_id)
    }

    /// Members that finished with a report; empty while any member is still running.
    fn ready_members(&self, study: &Study) -> Vec<Member> {
        let members = self.study_members(study);
        if members.iter().any(|m| {
            self.is_live(&m.run_id) || matches!(self.run_state(&m.run_id), Some(RunState::Running | RunState::Queued))
        }) {
            return Vec::new();
        }
        members.into_iter().filter(|m| m.run_dir.join("report.json").is_file()).collect()
    }

    /// Start (or redo) the study's comparison in the background. None: there is no such study.
    pub fn compare(&self, study_id: &str) -> Result<Option<()>, ManagerError> {
        let Some(study) = self.study(study_id) else { return Ok(None) };
        if self.is_comparing(study_id) {
            return Err(conflict("the comparison is already being written"));
        }
        if self.is_repeating(study_id) {
            return Err(conflict("the runs are still going; the check is written after the last one"));
        }
        let members = self.ready_members(&study);
        if members.len() < 2 {
            return Err(conflict("a comparison needs at least two finished runs with a report"));
        }
        let task = tokio::task::spawn_local(self.rc().run_compare(study, members));
        let mut st = self.st.borrow_mut();
        st.compare_errors.remove(study_id);
        st.comparing.insert(study_id.into(), task);
        Ok(Some(()))
    }

    /// After a study run ends: compare once every member is finished, unless a comparison is under way.
    fn compare_when_ready(&self, study_id: &str) {
        let Some(study) = self.study(study_id) else { return };
        if study.kind == StudyKind::Repeat || self.is_comparing(study_id) {
            return; // a "Check it" series writes its agreement itself, after its last run
        }
        if self.ready_members(&study).len() >= 2 {
            let _ = self.compare(study_id);
        }
    }

    async fn run_compare(self: Rc<Self>, study: Study, members: Vec<Member>) {
        let outcome: Result<(), String> = async {
            let folder = study_dir(&self.runs_dir, &study.id).map_err(|e| e.to_string())?;
            let config = self.reader(&members[members.len() - 1].run_id).config().ok_or("AssertionError")?;
            // The study folder gets the newest run's config, so the comparison uses the same AI tool and models,
            // and its calls are logged in the study folder (calls.jsonl), not in one of the runs.
            std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
            write_json(&folder.join("config.json"), &config).map_err(|e| e.to_string())?;
            let llm = self.llm(Some(&folder), 1, false).map_err(|e| format!("{e:?}"))?;
            if study.kind == StudyKind::Repeat {
                let agreement = build_agreement(
                    llm.as_ref(),
                    &study.id,
                    &members,
                    &config.question,
                    &config.planner_model,
                    &config.language,
                )
                .await
                .map_err(|e| e.to_string())?;
                write_json(&folder.join(AGREEMENT_FILE), &agreement).map_err(|e| e.to_string())?;
                return Ok(());
            }
            let states: HashMap<String, String> = members
                .iter()
                .map(|m| (m.run_id.clone(), self.run_state(&m.run_id).unwrap_or(RunState::Done).value().to_string()))
                .collect();
            let result = build_compare(
                llm.as_ref(),
                &study.id,
                &members,
                &config.question,
                &config.planner_model,
                &config.language,
                Some(&states),
            )
            .await
            .map_err(|e| e.to_string())?;
            write_json(&folder.join(COMPARE_FILE), &result).map_err(|e| e.to_string())?;
            Ok(())
        }
        .await;
        let mut st = self.st.borrow_mut();
        if let Err(error) = outcome {
            log::warning(LOGGER, &format!("comparison for study {} failed: {error}", study.id));
            st.compare_errors.insert(study.id.clone(), if error.is_empty() { "Exception".into() } else { error });
        }
        st.comparing.remove(&study.id);
    }

    /// Check it: run `source_id` again `runs - 1` times with the same people, text and settings but the next RNG
    /// seeds, one run after another so usage limits aren't hammered, then write what held (agreement.json).
    /// `agent_model` swaps the model the agents use in the new runs. An example's repeats use this machine's AI
    /// tool, like its interviews.
    pub async fn check(&self, source_id: &str, runs: i64, agent_model: Option<String>) -> Result<Study, ManagerError> {
        if !(2..=MAX_RUNS as i64).contains(&runs) {
            return Err(ManagerError::BadRequest(format!("runs must be between 2 and {MAX_RUNS}")));
        }
        if self.is_live(source_id) {
            return Err(conflict("the run is still in progress"));
        }
        let reader = self.reader(source_id);
        let (config, has_report) = (reader.config(), reader.has_report());
        drop(reader);
        let source = self.run_path(source_id);
        let Some(mut config) = config.filter(|_| has_report && source.join("personas.json").is_file()) else {
            return Err(conflict("only a finished run with a report can be checked"));
        };
        if self.is_example(source_id) {
            config = self.local_tool(config, "to check an example run").await?;
        }
        let study = Study {
            id: new_study_id(None),
            kind: StudyKind::Repeat,
            created_at: now_iso(),
            planned: runs,
            runs: vec![StudyRun { id: source_id.into(), variant: "Run 1".into() }],
        };
        save_study(&self.runs_dir, &study).map_err(internal)?;
        let task =
            tokio::task::spawn_local(self.rc().repeat(study.clone(), source_id.to_string(), config, agent_model));
        self.st.borrow_mut().repeating.insert(study.id.clone(), task);
        Ok(study)
    }

    async fn repeat(self: Rc<Self>, study: Study, source_id: String, source: SimConfig, agent_model: Option<String>) {
        let study_id = study.id.clone();
        let outcome = self.clone().repeat_runs(study, &source_id, &source, agent_model.as_deref()).await;
        self.st.borrow_mut().repeating.remove(&study_id);
        if outcome {
            let _ = self.compare(&study_id);
        }
    }

    /// The runs of a "Check it" series; true when all of them finished.
    async fn repeat_runs(
        self: Rc<Self>,
        mut study: Study,
        source_id: &str,
        source: &SimConfig,
        agent_model: Option<&str>,
    ) -> bool {
        let seeds_dir = self.run_path(source_id).join("seeds");
        let result: Result<bool, String> = async {
            for i in 1..study.planned {
                let mut files: Vec<PathBuf> = std::fs::read_dir(&seeds_dir)
                    .map(|entries| {
                        entries
                            .filter_map(|e| e.ok())
                            .map(|e| e.path())
                            .filter(|p| {
                                p.is_file() && !p.file_name().unwrap_or_default().to_string_lossy().starts_with('.')
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                files.sort();
                if files.is_empty() {
                    return Err(format!("{source_id} has no saved seed documents to run again"));
                }
                if self.st.borrow().closing {
                    return Ok(false);
                }
                let run_dir = self.create_run_dir(&source.question).map_err(|e| e.to_string())?;
                let seeds = copy_seeds(&run_dir, &files).map_err(|e| e.to_string())?;
                let config = repeat_config(source, source_id, i, &seeds, &study.id, agent_model);
                let variant = config.variant.clone().unwrap_or_default();
                let run_name = run_dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
                study = join_study(&self.runs_dir, &study, &run_name, &variant).map_err(|e| e.to_string())?;
                self.start(&run_dir, config, seeds).map_err(|e| format!("{e:?}"))?;
                let done = self.st.borrow().live.get(&run_name).map(|l| l.done.clone());
                if let Some(mut done) = done {
                    let _ = done.wait_for(|d| *d).await;
                }
                if self.run_state(&run_name) != Some(RunState::Done) {
                    log::info(LOGGER, &format!("check {} stopped: {run_name} did not finish", study.id));
                    if !self.st.borrow().closing {
                        // on shutdown the app shows "stopped after N of M runs" itself
                        let reason = format!(
                            "{variant} did not finish, so the check stopped after {} of {} runs",
                            study.runs.len(),
                            study.planned
                        );
                        self.stop_series(&study, &reason);
                    }
                    return Ok(false);
                }
            }
            Ok(true)
        }
        .await;
        match result {
            Ok(done) => done,
            Err(error) => {
                log::warning(LOGGER, &format!("check {} failed: {error}", study.id));
                self.stop_series(&study, if error.is_empty() { "Exception" } else { &error });
                false
            }
        }
    }

    /// A "Check it" series ends early: record why, and stop waiting for the runs that will not come.
    fn stop_series(&self, study: &Study, reason: &str) {
        self.st.borrow_mut().compare_errors.insert(study.id.clone(), reason.into());
        let _ = save_study(&self.runs_dir, &Study { planned: study.runs.len() as i64, ..study.clone() });
    }

    // ---------- fan-out ----------

    /// A new client queue. The first item is the run's current `status` event.
    pub fn subscribe(&self, run_id: &str) -> Arc<Queue> {
        let queue = Arc::new(Queue::default());
        self.st.borrow_mut().subs.entry(run_id.into()).or_default().push(queue.clone());
        let status = self.effective_status(run_id);
        if let Some(status) = &status {
            let text = model_json(status);
            queue.put(Some(("status".into(), text.clone())));
            self.st.borrow_mut().sent.entry(run_id.into()).or_insert(text);
        }
        if let Ok((hold, _, _)) = self.waiting(run_id) {
            // a client that connects during the pause still learns the cast is up for review
            let personas = hold.personas.borrow().clone().unwrap_or_default();
            queue.put(Some(("personas_ready".into(), py_dumps_compact(&json!({"personas": personas})))));
        }
        if !self.st.borrow().watches.contains_key(run_id) {
            let task = tokio::task::spawn_local(self.rc().watch(run_id.to_string()));
            let mut watch = Watch { task, last_post_id: None, last_db: None, last_round: None, last_state: None };
            if !self.is_live(run_id) {
                // remember where a run written elsewhere stands, so the watcher only reports what changes next
                let reader = self.reader(run_id);
                watch.last_post_id = Some(reader.max_post_id());
                watch.last_db = Some(reader.db_signature());
                watch.last_round = status.as_ref().map(|s| s.round);
                watch.last_state = status.as_ref().map(|s| s.state);
            }
            self.st.borrow_mut().watches.insert(run_id.into(), watch);
        }
        queue
    }

    pub fn unsubscribe(&self, run_id: &str, queue: &Arc<Queue>) {
        let task = {
            let mut st = self.st.borrow_mut();
            let Some(subs) = st.subs.get_mut(run_id) else { return };
            subs.retain(|q| !Arc::ptr_eq(q, queue));
            if !subs.is_empty() {
                return;
            }
            st.subs.remove(run_id);
            st.watches.remove(run_id).map(|w| w.task)
        };
        if let Some(task) = task {
            task.abort();
        }
    }

    pub fn subscriber_count(&self, run_id: &str) -> usize {
        self.st.borrow().subs.get(run_id).map_or(0, Vec::len)
    }

    pub fn publish(&self, run_id: &str, name: &str, data: &Value) {
        let subs = self.st.borrow().subs.get(run_id).cloned().unwrap_or_default();
        if subs.is_empty() {
            return;
        }
        let payload = py_dumps_compact(data);
        for queue in subs {
            queue.put(Some((name.into(), payload.clone())));
        }
    }

    fn send_status(&self, run_id: &str, status: &RunStatus) {
        let text = model_json(status);
        let subs = {
            let mut st = self.st.borrow_mut();
            if st.sent.get(run_id) == Some(&text) {
                return;
            }
            st.sent.insert(run_id.into(), text.clone());
            st.subs.get(run_id).cloned().unwrap_or_default()
        };
        for queue in subs {
            queue.put(Some(("status".into(), text.clone())));
        }
    }

    pub fn refresh_status(&self, run_id: &str) {
        if let Some(status) = self.effective_status(run_id) {
            self.send_status(run_id, &status);
        }
    }

    // ---------- watcher (runs this server is not executing) ----------

    async fn watch(self: Rc<Self>, run_id: String) {
        loop {
            tokio::time::sleep(self.poll_interval).await;
            if !self.st.borrow().watches.contains_key(&run_id) {
                return;
            }
            // a bad read must not kill the watcher
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.poll(&run_id)));
            if outcome.is_err() {
                log::debug(LOGGER, &format!("watch poll for {run_id} failed"));
            }
        }
    }

    fn poll(&self, run_id: &str) {
        let live = self.is_live(run_id);
        let reader = self.reader(run_id);
        let status = reader.status(live);
        if let Some(status) = &status {
            self.send_status(run_id, status);
        }
        let mut st = self.st.borrow_mut();
        let Some(watch) = st.watches.get_mut(run_id) else { return };
        if live {
            // in-process runs push their own events; re-baseline once they finish
            watch.last_post_id = None;
            watch.last_round = None;
            watch.last_state = None;
            return;
        }
        let mut events = watch.posts_added(&reader);
        if let Some(status) = status {
            events.extend(watch.rounds_done(&reader, &status));
            events.extend(watch.terminal_event(&status));
            watch.last_state = Some(status.state);
        }
        drop(st);
        for (name, data) in events {
            self.publish(run_id, name, &data);
        }
    }

    // ---------- lifecycle ----------

    /// Ask every live run to stop, give them `shutdown_grace`, then cancel what is left.
    pub async fn shutdown(&self) {
        let (mut tasks, dones, run_dirs) = {
            let mut st = self.st.borrow_mut();
            st.closing = true; // a "Check it" series must not start its next run during the grace wait
            let mut tasks = Vec::new();
            let mut dones = Vec::new();
            let mut run_dirs = Vec::new();
            for live in st.live.values_mut() {
                live.stop.store(true, Ordering::SeqCst);
                live.hold.wake.notify_one();
                if let Some(task) = live.task.take() {
                    tasks.push(task);
                    dones.push(live.done.clone());
                    run_dirs.push(live.run_dir.clone());
                }
            }
            (tasks, dones, run_dirs)
        };
        if !tasks.is_empty() {
            let all_done = futures::future::join_all(dones.into_iter().map(|mut d| async move {
                let _ = d.wait_for(|v| *v).await;
            }));
            if tokio::time::timeout(self.shutdown_grace, all_done).await.is_err() {
                tasks.retain(|t| !t.is_finished());
                for task in &tasks {
                    task.abort();
                }
                let _ = tokio::time::timeout(Duration::from_secs(2), futures::future::join_all(tasks)).await;
                // The pipeline marks itself stopped when it is dropped; this catches a run whose own status
                // write failed, like `_mark_stopped(run_dir, "stopped: server shut down")` after cancelling.
                for run_dir in &run_dirs {
                    Self::mark_stopped(run_dir, "stopped: server shut down");
                }
            }
        }
        let (others, queues) = {
            let mut st = self.st.borrow_mut();
            let mut others: Vec<JoinHandle<()>> = Vec::new();
            others.extend(st.repeating.drain().map(|(_, t)| t));
            others.extend(st.rebuilding.drain().map(|(_, t)| t));
            others.extend(st.comparing.drain().map(|(_, t)| t));
            others.extend(st.watches.drain().map(|(_, w)| w.task));
            let queues: Vec<Arc<Queue>> = st.subs.values().flatten().cloned().collect();
            (others, queues)
        };
        for task in others {
            task.abort();
        }
        for queue in queues {
            queue.put(None);
        }
    }

    /// Marks a run that is still `running` or `queued` on disk as stopped with `detail`.
    pub fn mark_stopped(run_dir: &Path, detail: &str) {
        if let Some(mut status) = read_status(run_dir) {
            if matches!(status.state, RunState::Running | RunState::Queued) {
                status.state = RunState::Stopped;
                status.detail = detail.into();
                status.updated_at = now_iso();
                let _ = write_status(run_dir, &status);
            }
        }
    }

    /// The graph of a run, or an empty one.
    pub fn graph(&self, run_id: &str) -> KnowledgeGraph {
        self.reader(run_id).graph().unwrap_or_default()
    }
}

/// Whether the task under `key` is still going: `task is not None and not task.done()`. A task that panicked has
/// finished without removing its entry, and must not block the next rebuild, comparison or series.
fn running(tasks: &HashMap<String, JoinHandle<()>>, key: &str) -> bool {
    tasks.get(key).is_some_and(|task| !task.is_finished())
}

/// `str(e) or type(e).__name__` of a report-stage error.
fn error_text(e: &ReportError) -> String {
    let text = e.to_string();
    if text.is_empty() {
        "Exception".into()
    } else {
        text
    }
}

fn report_error(e: ReportError) -> ManagerError {
    match e {
        ReportError::Llm(e) => ManagerError::Llm(e.0),
        ReportError::UnknownAgent(_) => ManagerError::UnknownAgent,
        other => internal(other),
    }
}

/// Validate chat history from the client: a list of [question, answer] pairs of strings.
pub fn history_pairs(raw: &[Value]) -> Result<Vec<(String, String)>, ManagerError> {
    raw.iter()
        .map(|item| match item.as_array().map(Vec::as_slice) {
            Some([Value::String(q), Value::String(a)]) => Ok((q.clone(), a.clone())),
            _ => Err(ManagerError::BadRequest("history must be a list of [question, answer] string pairs".into())),
        })
        .collect()
}
