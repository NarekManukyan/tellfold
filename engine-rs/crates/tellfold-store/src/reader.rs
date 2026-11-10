//! Read-only access to one run folder, whether the run is finished, live in this server or live in a CLI: the port
//! of `tellfold/server/reader.py`.
//!
//! `RunReader` never writes. It opens `world.db` with `mode=ro`, so it cannot interfere with a writer in this or
//! another process (the database is WAL). Every accessor tolerates missing files, missing tables (older runs) and
//! half-written JSON: it returns an empty value instead of failing.

use std::cell::{OnceCell, RefCell};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use rusqlite::types::FromSql;
use rusqlite::{Connection, Params};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tellfold_models::json::{py_round, py_sum};
use tellfold_models::{
    is_agent_purpose, lax, Action, GodEvent, KnowledgeGraph, Persona, Post, Report, RoundStats, RunStage, RunState,
    RunStatus, SimConfig,
};

use crate::api::{ActionOut, AgentDetail, CallTotals, CallsSummary, PostOut, ReportRebuild, RunDetail, RunSummary};
use crate::files::{self, iso_from_timestamp, mtime, now_timestamp};
use crate::world::{open_read_only, POST_COLUMNS};

/// True when a process with this pid exists. Never sends a real signal.
pub fn pid_alive(pid: Option<i64>) -> bool {
    let Some(pid) = pid.filter(|p| *p > 0 && *p <= i32::MAX as i64) else { return false };
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only checks that the process exists and may be signalled.
        if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) // exists, owned by someone else
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false // Windows support comes with the cross-platform milestone
    }
}

/// Read-only view of `runs/<id>/`.
pub struct RunReader {
    pub run_dir: PathBuf,
    pub id: String,
    pub immutable: bool,
    conn: OnceCell<Option<Connection>>,
    persona_cache: RefCell<Option<IndexMap<String, Persona>>>,
}

impl RunReader {
    /// `immutable` is for runs on read-only media (the app's bundled examples): SQLite then skips all locking.
    pub fn new(run_dir: &Path, run_id: Option<&str>, immutable: bool) -> Self {
        let id = run_id
            .map(str::to_string)
            .unwrap_or_else(|| run_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        RunReader {
            run_dir: run_dir.to_path_buf(),
            id,
            immutable,
            conn: OnceCell::new(),
            persona_cache: RefCell::new(None),
        }
    }

    // ---------- files ----------

    pub fn db_path(&self) -> PathBuf {
        self.run_dir.join(files::WORLD_FILE)
    }

    /// A directory counts as a run once it has a config.json.
    pub fn is_run(&self) -> bool {
        self.run_dir.join(files::CONFIG_FILE).is_file()
    }

    fn json_model<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        files::read_model(&self.run_dir.join(name))
    }

    pub fn config(&self) -> Option<SimConfig> {
        self.json_model(files::CONFIG_FILE)
    }

    pub fn graph(&self) -> Option<KnowledgeGraph> {
        self.json_model(files::GRAPH_FILE)
    }

    pub fn report(&self) -> Option<Report> {
        self.json_model(files::REPORT_FILE)
    }

    pub fn has_report(&self) -> bool {
        self.run_dir.join(files::REPORT_FILE).is_file()
    }

    pub fn raw_status(&self) -> Option<RunStatus> {
        files::read_status(&self.run_dir)
    }

    /// From personas.json, else (a run still being set up) from the database.
    pub fn personas(&self) -> Vec<Persona> {
        self.load_personas();
        self.persona_cache.borrow().as_ref().map(|m| m.values().cloned().collect()).unwrap_or_default()
    }

    fn load_personas(&self) {
        if self.persona_cache.borrow().is_some() {
            return;
        }
        let people: Vec<Persona> = match self.json_model::<Vec<Persona>>(files::PERSONAS_FILE) {
            Some(people) => people,
            None => self
                .rows::<String, _>("SELECT data FROM personas ORDER BY id", [])
                .into_iter()
                .filter_map(|data| serde_json::from_str(&data).ok())
                .collect(),
        };
        *self.persona_cache.borrow_mut() = Some(people.into_iter().map(|p| (p.id.clone(), p)).collect());
    }

    fn handles(&self) -> IndexMap<String, String> {
        self.load_personas();
        self.persona_cache
            .borrow()
            .as_ref()
            .map(|m| m.iter().map(|(id, p)| (id.clone(), p.handle.clone())).collect())
            .unwrap_or_default()
    }

    /// Totals from calls.jsonl. `agent` covers the calls routed to the agent runtime (rounds, sentiment,
    /// interviews), `planner` the rest (graph, personas, report, ask).
    pub fn calls(&self) -> CallsSummary {
        let mut all = CallTotals::default();
        let mut agent = CallTotals::default();
        let mut planner = CallTotals::default();
        if let Ok(text) = fs::read_to_string(self.run_dir.join(files::CALLS_FILE)) {
            for line in text.split_inclusive('\n') {
                let Ok(Value::Object(rec)) = serde_json::from_str::<Value>(line) else {
                    continue; // a line still being written, or not a record
                };
                let seconds = python_float_or_zero(rec.get("seconds"));
                let purpose = python_str_or_empty(rec.get("purpose"));
                let failed = rec.get("ok") == Some(&Value::Bool(false));
                let role = if is_agent_purpose(&purpose) { &mut agent } else { &mut planner };
                for totals in [&mut all, role] {
                    totals.count += 1;
                    totals.failed += failed as i64;
                    totals.seconds += seconds;
                }
            }
        }
        for t in [&mut all, &mut agent, &mut planner] {
            t.seconds = py_round(t.seconds, 2);
        }
        CallsSummary { count: all.count, failed: all.failed, seconds: all.seconds, agent, planner }
    }

    pub fn mtime(&self, name: &str) -> f64 {
        mtime(&self.run_dir.join(name))
    }

    /// Changes whenever the database (or its WAL) is written.
    pub fn db_signature(&self) -> (f64, f64, u64) {
        let wal = self.run_dir.join("world.db-wal");
        let wal_size = fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
        (self.mtime(files::WORLD_FILE), self.mtime("world.db-wal"), wal_size)
    }

    // ---------- database ----------

    fn db(&self) -> Option<&Connection> {
        self.conn
            .get_or_init(|| {
                if !self.db_path().is_file() {
                    return None;
                }
                let conn = open_read_only(&self.db_path(), self.immutable).ok()?;
                // fails here, not later, when the file is not a database
                let probe = conn
                    .prepare("SELECT 1 FROM sqlite_master LIMIT 1")
                    .and_then(|mut stmt| stmt.query([]).and_then(|mut rows| rows.next().map(|_| ())));
                probe.ok()?;
                Some(conn)
            })
            .as_ref()
    }

    /// Rows of a one-column query; empty when the database or the table is missing (an older or half-initialised
    /// run).
    fn rows<T: FromSql, P: Params>(&self, sql: &str, args: P) -> Vec<T> {
        self.query(sql, args, |r| r.get(0))
    }

    fn query<T, P: Params>(&self, sql: &str, args: P, f: impl FnMut(&rusqlite::Row) -> rusqlite::Result<T>) -> Vec<T> {
        let Some(conn) = self.db() else { return Vec::new() };
        let Ok(mut stmt) = conn.prepare(sql) else { return Vec::new() };
        let Ok(rows) = stmt.query_map(args, f) else { return Vec::new() };
        rows.collect::<rusqlite::Result<Vec<T>>>().unwrap_or_default()
    }

    pub fn round_stats(&self) -> Vec<RoundStats> {
        self.rows::<String, _>("SELECT data FROM round_stats ORDER BY round", [])
            .into_iter()
            .filter_map(|data| serde_json::from_str(&data).ok())
            .collect()
    }

    pub fn events(&self) -> Vec<GodEvent> {
        self.query("SELECT round, content FROM events ORDER BY round, seq", [], |r| {
            Ok(GodEvent { round: r.get(0)?, content: r.get(1)? })
        })
    }

    pub fn follows(&self) -> Vec<[String; 2]> {
        self.query("SELECT follower, followee FROM follows ORDER BY follower, followee", [], |r| {
            Ok([r.get(0)?, r.get(1)?])
        })
    }

    pub fn sentiments(&self) -> IndexMap<i64, f64> {
        self.query("SELECT post_id, score FROM post_sentiment", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .into_iter()
            .collect()
    }

    pub fn max_post_id(&self) -> i64 {
        self.rows::<Option<i64>, _>("SELECT MAX(id) FROM posts", []).into_iter().next().flatten().unwrap_or(0)
    }

    fn posts_where<P: Params>(&self, filter: &str, args: P) -> Vec<Post> {
        let sql = format!("SELECT {POST_COLUMNS} FROM posts {filter} ORDER BY id");
        self.query(&sql, args, |r| {
            Ok(Post {
                id: r.get(0)?,
                round: r.get(1)?,
                author_id: r.get(2)?,
                content: r.get(3)?,
                parent_id: r.get(4)?,
                repost_of: r.get(5)?,
                likes: r.get(6)?,
                dislikes: r.get(7)?,
                reposts: r.get(8)?,
                replies: r.get(9)?,
            })
        })
    }

    fn post_out(&self, posts: Vec<Post>) -> Vec<PostOut> {
        let handles = self.handles();
        let scores = if posts.is_empty() { IndexMap::new() } else { self.sentiments() };
        posts
            .into_iter()
            .map(|p| PostOut {
                author_handle: handles.get(&p.author_id).cloned().unwrap_or_else(|| p.author_id.clone()),
                sentiment: scores.get(&p.id).copied(),
                post: p,
            })
            .collect()
    }

    /// Posts with id > since_id, ascending.
    pub fn posts(&self, since_id: i64) -> Vec<PostOut> {
        let posts = self.posts_where("WHERE id > ?", [since_id]);
        self.post_out(posts)
    }

    /// (round, id) of posts newer than since_id, ascending by id.
    pub fn new_post_ids(&self, since_id: i64) -> Vec<(i64, i64)> {
        self.query("SELECT id, round FROM posts WHERE id > ? ORDER BY id", [since_id], |r| {
            Ok((r.get::<_, i64>(1)?, r.get::<_, i64>(0)?))
        })
    }

    pub fn actions(&self, round: Option<i64>) -> Vec<ActionOut> {
        let rows: Vec<(i64, String)> = match round {
            Some(r) => self.query("SELECT round, data FROM actions WHERE round = ? ORDER BY seq", [r], |row| {
                Ok((row.get(0)?, row.get(1)?))
            }),
            None => {
                self.query("SELECT round, data FROM actions ORDER BY seq", [], |row| Ok((row.get(0)?, row.get(1)?)))
            }
        };
        rows.into_iter()
            .filter_map(|(r, data)| {
                let a: Action = serde_json::from_str(&data).ok()?;
                Some(ActionOut {
                    round: r,
                    agent_id: a.agent_id,
                    r#type: a.r#type.value().into(),
                    target_post_id: a.target_post_id,
                    target_agent_id: a.target_agent_id,
                    content: a.content,
                    reasoning: a.reasoning,
                })
            })
            .collect()
    }

    /// The agent's profile, memory, posts and follows, or None for an unknown agent.
    pub fn agent(&self, agent_id: &str) -> Option<AgentDetail> {
        self.load_personas();
        let persona = self.persona_cache.borrow().as_ref()?.get(agent_id).cloned()?;
        let memory = self
            .rows::<String, _>("SELECT notes FROM memories WHERE agent_id = ?", [agent_id])
            .into_iter()
            .next()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .map(|notes| match notes {
                Value::Array(items) => items.iter().map(python_str).collect(),
                _ => Vec::new(),
            })
            .unwrap_or_default();
        let posts = self.post_out(self.posts_where("WHERE author_id = ?", [agent_id]));
        let mut by_round: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
        for p in &posts {
            if let Some(s) = p.sentiment {
                by_round.entry(p.post.round).or_default().push(s);
            }
        }
        Some(AgentDetail {
            persona,
            memory,
            posts,
            followers: self.rows("SELECT follower FROM follows WHERE followee = ? ORDER BY follower", [agent_id]),
            following: self.rows("SELECT followee FROM follows WHERE follower = ? ORDER BY followee", [agent_id]),
            sentiment_by_round: by_round
                .into_iter()
                .map(|(r, v)| (r, py_round(py_sum(v.iter().copied()) / v.len() as f64, 3)))
                .collect(),
        })
    }

    // ---------- status ----------

    /// The run's effective status.
    ///
    /// `live` says whether this server is executing the run right now. A run whose status.json says `running` but
    /// whose owner process is gone (or is this server, which no longer runs it) is `stopped`. Older runs without
    /// status.json are `done` when report.json exists, else `stopped`.
    pub fn status(&self, live: bool) -> Option<RunStatus> {
        let config = self.config();
        let Some(mut status) = self.raw_status() else {
            return config.map(|c| self.derived_status(&c, live));
        };
        if matches!(status.state, RunState::Running | RunState::Queued) && !live {
            let owner_gone =
                status.pid.is_none() || status.pid == Some(std::process::id() as i64) || !pid_alive(status.pid);
            if owner_gone {
                status.state = RunState::Stopped;
            }
        }
        Some(status)
    }

    fn derived_status(&self, config: &SimConfig, live: bool) -> RunStatus {
        let rounds: Vec<i64> = self.round_stats().iter().map(|s| s.round).collect();
        let done = self.has_report();
        let (state, stage) = if live {
            (RunState::Running, RunStage::Reading)
        } else if done {
            (RunState::Done, RunStage::Finished)
        } else {
            let stage = if !rounds.is_empty() {
                RunStage::Rounds
            } else if self.run_dir.join(files::PERSONAS_FILE).exists() {
                RunStage::Personas
            } else if self.run_dir.join(files::GRAPH_FILE).exists() {
                RunStage::Graph
            } else {
                RunStage::Reading
            };
            (RunState::Stopped, stage)
        };
        let started = match self.mtime(files::CONFIG_FILE) {
            0.0 => now_timestamp(),
            t => t,
        };
        let updated = started.max(self.mtime(files::WORLD_FILE)).max(self.mtime(files::REPORT_FILE));
        RunStatus {
            state,
            stage,
            round: rounds.iter().copied().max().unwrap_or(0),
            total_rounds: config.rounds,
            detail: if live { String::new() } else { "no status file (older run)".into() },
            error: None,
            pid: None,
            started_at: iso_from_timestamp(started),
            updated_at: iso_from_timestamp(updated),
            failed_batches: Vec::new(),
        }
    }

    // ---------- API shapes ----------

    pub fn summary(&self, live: bool) -> Option<RunSummary> {
        let config = self.config()?;
        let status = self.status(live)?;
        Some(RunSummary {
            id: self.id.clone(),
            question: config.question,
            state: status.state.value().into(),
            stage: status.stage.value().into(),
            round: status.round,
            total_rounds: if status.total_rounds != 0 { status.total_rounds } else { config.rounds },
            agents: config.num_agents,
            created_at: status.started_at,
            has_report: self.has_report(),
            example: false,
            study_id: config.study_id,
            variant: config.variant,
        })
    }

    pub fn detail(&self, live: bool) -> Option<RunDetail> {
        let config = self.config()?;
        Some(RunDetail {
            id: self.id.clone(),
            status: self.status(live),
            live,
            personas: self.personas(),
            graph: self.graph(),
            stats: self.round_stats(),
            events: self.events(),
            follows: self.follows(),
            report: self.report(),
            calls: self.calls(),
            report_rebuild: ReportRebuild::default(),
            example: false,
            config,
        })
    }
}

/// `float(value or 0)`, 0 when Python would raise.
fn python_float_or_zero(value: Option<&Value>) -> f64 {
    match value {
        None | Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => *b as i64 as f64,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) if s.is_empty() => 0.0,
        Some(Value::String(s)) => lax::parse_float(s).unwrap_or(0.0),
        Some(_) => 0.0,
    }
}

/// `str(value or "")`.
fn python_str_or_empty(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => String::new(),
        Some(Value::Number(n)) if n.as_f64() == Some(0.0) => String::new(),
        Some(Value::Array(a)) if a.is_empty() => String::new(),
        Some(Value::Object(o)) if o.is_empty() => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => python_str(v),
    }
}

/// `str(value)` for a JSON value, enough for the strings, numbers and booleans found in run files.
fn python_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => "None".into(),
        Value::Number(n) if n.is_f64() => tellfold_models::json::py_float_repr(n.as_f64().unwrap_or(0.0)),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}
