//! The simulated social platform, persisted in SQLite: the port of `tellfold/world.py`.
//!
//! All writes go through `World`; it validates every action so malformed model output can never corrupt the
//! state. The schema, the JSON in its columns and the feed ranking are the Python engine's, so either engine can
//! open the other's `world.db`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use blake2::digest::{Update, VariableOutput};
use blake2::Blake2bVar;
use indexmap::IndexMap;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Row};
use tellfold_models::text::{py_prefix, py_strip};
use tellfold_models::{json, Action, ActionType, AgentMemory, GodEvent, Persona, Post, RoundStats};

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS personas (
    id TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS follows (
    follower TEXT NOT NULL,
    followee TEXT NOT NULL,
    PRIMARY KEY (follower, followee)
);
CREATE TABLE IF NOT EXISTS posts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    round INTEGER NOT NULL,
    author_id TEXT NOT NULL,
    content TEXT NOT NULL,
    parent_id INTEGER,
    repost_of INTEGER,
    likes INTEGER NOT NULL DEFAULT 0,
    dislikes INTEGER NOT NULL DEFAULT 0,
    reposts INTEGER NOT NULL DEFAULT 0,
    replies INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS posts_round ON posts(round);
CREATE TABLE IF NOT EXISTS reactions (
    agent_id TEXT NOT NULL,
    post_id INTEGER NOT NULL,
    kind TEXT NOT NULL,
    PRIMARY KEY (agent_id, post_id)
);
CREATE TABLE IF NOT EXISTS actions (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    round INTEGER NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS actions_round ON actions(round);
CREATE TABLE IF NOT EXISTS memories (
    agent_id TEXT PRIMARY KEY,
    notes TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS memory_history (
    agent_id TEXT NOT NULL,
    round INTEGER NOT NULL,
    notes TEXT NOT NULL,
    PRIMARY KEY (agent_id, round)
);
CREATE TABLE IF NOT EXISTS round_stats (
    round INTEGER PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    round INTEGER NOT NULL,
    content TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS post_sentiment (
    post_id INTEGER PRIMARY KEY,
    score REAL NOT NULL
);
";

pub const POST_COLUMNS: &str = "id, round, author_id, content, parent_id, repost_of, likes, dislikes, reposts, replies";
const MAX_CONTENT: usize = 600;
const RECENCY_DECAY: f64 = 0.7; // feed score multiplier per round of age
const TIER_WINDOW: i64 = 2; // replies/followed posts only jump the queue while younger than this many rounds

#[derive(Debug, thiserror::Error)]
pub enum WorldError {
    #[error("unknown agent '{0}'")]
    UnknownAgent(String),
    #[error("unknown post {0}")]
    UnknownPost(i64),
    /// Branching from a round whose memories were not kept.
    #[error("{0}")]
    Branch(String),
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, WorldError>;

pub struct World {
    db: Connection,
    seed: i64,
    persona_cache: RefCell<Option<IndexMap<String, Persona>>>,
}

/// `file:` URI for a path, percent-encoded like Python's `Path.as_uri()`.
pub fn file_uri(path: &Path) -> String {
    let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut out = String::from("file://");
    for b in absolute.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/_.-~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Opens `path` read-only through a URI (`mode=ro`, plus `immutable=1` for read-only media).
pub fn open_read_only(path: &Path, immutable: bool) -> rusqlite::Result<Connection> {
    let query = if immutable { "mode=ro&immutable=1" } else { "mode=ro" };
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(format!("{}?{query}", file_uri(path)), flags)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(conn)
}

fn row_post(row: &Row) -> rusqlite::Result<Post> {
    Ok(Post {
        id: row.get(0)?,
        round: row.get(1)?,
        author_id: row.get(2)?,
        content: row.get(3)?,
        parent_id: row.get(4)?,
        repost_of: row.get(5)?,
        likes: row.get(6)?,
        dislikes: row.get(7)?,
        reposts: row.get(8)?,
        replies: row.get(9)?,
    })
}

/// `likes + 2*reposts + replies - dislikes`, shifted so a fresh post scores 1 and heavy dislikes floor at 0.2.
pub fn engagement_weight(post: &Post) -> f64 {
    let engagement = post.likes + 2 * post.reposts + post.replies - post.dislikes;
    (1.0 + engagement as f64).max(0.2)
}

fn downgrade(action: &Action, problem: &str) -> Action {
    let note = format!("[invalid {}: {problem}]", action.r#type.value());
    Action {
        agent_id: action.agent_id.clone(),
        r#type: ActionType::Nothing,
        content: String::new(),
        target_post_id: None,
        target_agent_id: None,
        reasoning: py_strip(&format!("{note} {}", action.reasoning)).to_string(),
    }
}

impl World {
    /// Opens (and creates) a writable world. `seed` only perturbs feed tie-breaks.
    pub fn open(path: &Path, seed: i64) -> Result<World> {
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
        db.execute_batch("PRAGMA synchronous=NORMAL")?; // WAL + NORMAL survives app crashes
        db.execute_batch(SCHEMA)?;
        Ok(World { db, seed, persona_cache: RefCell::new(None) })
    }

    /// Opens an existing finished world that must never change (the app's bundled examples): no schema setup, no
    /// journal files, and every write fails.
    pub fn open_read_only(path: &Path, seed: i64) -> Result<World> {
        let db = open_read_only(path, true)?;
        Ok(World { db, seed, persona_cache: RefCell::new(None) })
    }

    pub fn connection(&self) -> &Connection {
        &self.db
    }

    pub fn close(self) -> Result<()> {
        self.db.close().map_err(|(_, e)| e.into())
    }

    // ---------- personas & follow graph ----------

    pub fn add_personas(&self, personas: &[Persona]) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        for p in personas {
            tx.execute("INSERT OR REPLACE INTO personas (id, data) VALUES (?, ?)", params![p.id, json::to_string(p)])?;
        }
        tx.commit()?;
        *self.persona_cache.borrow_mut() = None;
        Ok(())
    }

    fn with_personas<T>(&self, f: impl FnOnce(&IndexMap<String, Persona>) -> T) -> Result<T> {
        if self.persona_cache.borrow().is_none() {
            let mut stmt = self.db.prepare("SELECT data FROM personas ORDER BY id")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            let mut people = IndexMap::new();
            for data in rows {
                let p: Persona = serde_json::from_str(&data?)?;
                people.insert(p.id.clone(), p);
            }
            *self.persona_cache.borrow_mut() = Some(people);
        }
        Ok(f(self.persona_cache.borrow().as_ref().expect("just filled")))
    }

    pub fn personas(&self) -> Result<Vec<Persona>> {
        self.with_personas(|m| m.values().cloned().collect())
    }

    pub fn persona(&self, agent_id: &str) -> Result<Persona> {
        self.with_personas(|m| m.get(agent_id).cloned())?.ok_or_else(|| WorldError::UnknownAgent(agent_id.into()))
    }

    fn is_agent(&self, agent_id: &str) -> Result<bool> {
        self.with_personas(|m| m.contains_key(agent_id))
    }

    /// Create a follow edge without logging an action (used to seed the initial graph).
    pub fn add_follow(&self, follower: &str, followee: &str) -> Result<bool> {
        if follower == followee || !self.is_agent(follower)? || !self.is_agent(followee)? {
            return Ok(false);
        }
        let n = self
            .db
            .execute("INSERT OR IGNORE INTO follows (follower, followee) VALUES (?, ?)", [follower, followee])?;
        Ok(n == 1)
    }

    fn strings(&self, sql: &str, arg: &str) -> Result<Vec<String>> {
        let mut stmt = self.db.prepare(sql)?;
        let rows = stmt.query_map([arg], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn followers(&self, agent_id: &str) -> Result<Vec<String>> {
        self.strings("SELECT follower FROM follows WHERE followee = ? ORDER BY follower", agent_id)
    }

    pub fn following(&self, agent_id: &str) -> Result<Vec<String>> {
        self.strings("SELECT followee FROM follows WHERE follower = ? ORDER BY followee", agent_id)
    }

    /// Every follow edge as (follower, followee), in a stable order.
    pub fn follow_edges(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self.db.prepare("SELECT follower, followee FROM follows ORDER BY follower, followee")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn follower_counts(&self) -> Result<IndexMap<String, i64>> {
        let mut stmt = self.db.prepare("SELECT followee, COUNT(*) FROM follows GROUP BY followee")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---------- actions ----------

    /// Validate and apply one action. Invalid actions are stored as `nothing`. Returns a new post id.
    pub fn apply(&self, round: i64, action: &Action) -> Result<Option<i64>> {
        let tx = self.db.unchecked_transaction()?;
        let downgraded;
        let action = match self.validate(action)? {
            Some(problem) => {
                downgraded = downgrade(action, problem);
                &downgraded
            }
            None => action,
        };
        let post_id = self.execute(round, action)?;
        tx.execute("INSERT INTO actions (round, data) VALUES (?, ?)", params![round, json::to_string(action)])?;
        tx.commit()?;
        Ok(post_id)
    }

    fn validate(&self, a: &Action) -> Result<Option<&'static str>> {
        if !self.is_agent(&a.agent_id)? {
            return Ok(Some("unknown agent"));
        }
        let has_content = !py_strip(&a.content).is_empty();
        Ok(match a.r#type {
            ActionType::Post => (!has_content).then_some("empty post"),
            ActionType::Reply => self.check_target(a)?.or((!has_content).then_some("empty reply")),
            ActionType::Repost => match self.check_target(a)? {
                Some(problem) => Some(problem),
                None => {
                    let original = self.original(self.post(a.target_post_id.expect("checked"))?)?;
                    (original.author_id == a.agent_id).then_some("repost of own post")
                }
            },
            ActionType::Like | ActionType::Dislike => match self.check_target(a)? {
                Some(problem) => Some(problem),
                None => {
                    let target = a.target_post_id.expect("checked");
                    if self.post(target)?.author_id == a.agent_id {
                        Some("reaction to own post")
                    } else {
                        let seen = self
                            .db
                            .query_row(
                                "SELECT 1 FROM reactions WHERE agent_id = ? AND post_id = ?",
                                params![a.agent_id, target],
                                |_| Ok(()),
                            )
                            .optional()?;
                        seen.map(|_| "already reacted")
                    }
                }
            },
            ActionType::Follow => match &a.target_agent_id {
                Some(target) if self.is_agent(target)? => {
                    if *target == a.agent_id {
                        Some("self-follow")
                    } else if self.following(&a.agent_id)?.contains(target) {
                        Some("already following")
                    } else {
                        None
                    }
                }
                _ => Some("unknown follow target"),
            },
            ActionType::Nothing => None,
        })
    }

    fn check_target(&self, a: &Action) -> Result<Option<&'static str>> {
        let Some(id) = a.target_post_id else { return Ok(Some("missing target post")) };
        let exists = self.db.query_row("SELECT 1 FROM posts WHERE id = ?", [id], |_| Ok(())).optional()?;
        Ok(exists.is_none().then_some("unknown target post"))
    }

    fn execute(&self, round: i64, a: &Action) -> Result<Option<i64>> {
        let content = py_prefix(py_strip(&a.content), MAX_CONTENT);
        match a.r#type {
            ActionType::Post => Ok(Some(self.insert_post(round, &a.agent_id, content, None, None)?)),
            ActionType::Reply => {
                let target = a.target_post_id.expect("validated");
                self.bump(target, "replies")?;
                Ok(Some(self.insert_post(round, &a.agent_id, content, Some(target), None)?))
            }
            ActionType::Repost => {
                let original = self.original(self.post(a.target_post_id.expect("validated"))?)?;
                self.bump(original.id, "reposts")?;
                Ok(Some(self.insert_post(round, &a.agent_id, content, None, Some(original.id))?))
            }
            ActionType::Like | ActionType::Dislike => {
                let target = a.target_post_id.expect("validated");
                self.db.execute(
                    "INSERT INTO reactions (agent_id, post_id, kind) VALUES (?, ?, ?)",
                    params![a.agent_id, target, a.r#type.value()],
                )?;
                self.bump(target, if a.r#type == ActionType::Like { "likes" } else { "dislikes" })?;
                Ok(None)
            }
            ActionType::Follow => {
                self.db.execute(
                    "INSERT INTO follows (follower, followee) VALUES (?, ?)",
                    params![a.agent_id, a.target_agent_id],
                )?;
                Ok(None)
            }
            ActionType::Nothing => Ok(None),
        }
    }

    fn insert_post(
        &self,
        round: i64,
        author: &str,
        content: &str,
        parent_id: Option<i64>,
        repost_of: Option<i64>,
    ) -> Result<i64> {
        self.db.execute(
            "INSERT INTO posts (round, author_id, content, parent_id, repost_of) VALUES (?, ?, ?, ?, ?)",
            params![round, author, content, parent_id, repost_of],
        )?;
        Ok(self.db.last_insert_rowid())
    }

    fn bump(&self, post_id: i64, column: &str) -> Result<()> {
        self.db.execute(&format!("UPDATE posts SET {column} = {column} + 1 WHERE id = ?"), [post_id])?;
        Ok(())
    }

    /// Reposting a repost credits the original post.
    fn original(&self, post: Post) -> Result<Post> {
        match post.repost_of {
            Some(id) => self.post(id),
            None => Ok(post),
        }
    }

    pub fn actions(&self, round: Option<i64>) -> Result<Vec<(i64, Action)>> {
        let mut sql = String::from("SELECT round, data FROM actions");
        if round.is_some() {
            sql.push_str(" WHERE round = ?1");
        }
        sql.push_str(" ORDER BY seq");
        let mut stmt = self.db.prepare(&sql)?;
        let read = |row: &Row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?));
        let rows: Vec<(i64, String)> = match round {
            Some(r) => stmt.query_map([r], read)?.collect::<rusqlite::Result<_>>()?,
            None => stmt.query_map([], read)?.collect::<rusqlite::Result<_>>()?,
        };
        let mut out = Vec::new();
        for (r, data) in rows {
            out.push((r, serde_json::from_str(&data)?));
        }
        Ok(out)
    }

    /// Agent id -> the last round that agent acted in, for every agent that has acted.
    pub fn last_active_rounds(&self) -> Result<HashMap<String, i64>> {
        let mut stmt = self
            .db
            .prepare("SELECT json_extract(data, '$.agent_id') AS agent, MAX(round) FROM actions GROUP BY agent")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)))?;
        let mut out = HashMap::new();
        for row in rows {
            if let (Some(agent), round) = row? {
                out.insert(agent, round);
            }
        }
        Ok(out)
    }

    // ---------- posts & feeds ----------

    /// Everyone who has written a post, reply or repost.
    pub fn authors(&self) -> Result<HashSet<String>> {
        let mut stmt = self.db.prepare("SELECT DISTINCT author_id FROM posts")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn post(&self, post_id: i64) -> Result<Post> {
        self.db
            .query_row(&format!("SELECT {POST_COLUMNS} FROM posts WHERE id = ?"), [post_id], row_post)
            .optional()?
            .ok_or(WorldError::UnknownPost(post_id))
    }

    pub fn posts(&self, round: Option<i64>) -> Result<Vec<Post>> {
        let mut stmt;
        let rows = match round {
            Some(r) => {
                stmt = self.db.prepare(&format!("SELECT {POST_COLUMNS} FROM posts WHERE round = ? ORDER BY id"))?;
                stmt.query_map([r], row_post)?.collect::<rusqlite::Result<Vec<_>>>()?
            }
            None => {
                stmt = self.db.prepare(&format!("SELECT {POST_COLUMNS} FROM posts ORDER BY id"))?;
                stmt.query_map([], row_post)?.collect::<rusqlite::Result<Vec<_>>>()?
            }
        };
        Ok(rows)
    }

    /// Rank visible posts for one agent.
    ///
    /// Posts the agent has not yet engaged with come first; ones it already liked, disliked, replied to or
    /// reposted sink to the bottom. Among fresh posts (this round or the previous one), replies to the agent's own
    /// posts come first, then posts by followed accounts, then the rest. Older posts get no tier bonus and compete
    /// on score alone: engagement x author reach x recency, then a seeded tie-break.
    /// The first `k` posts by rank; a negative `k` drops that many from the end, like Python's `[:k]`.
    pub fn feed_for(&self, agent_id: &str, round: i64, k: i64) -> Result<Vec<Post>> {
        let columns: Vec<String> = POST_COLUMNS.split(", ").map(|c| format!("p.{c}")).collect();
        let sql = format!(
            "SELECT {}, parent.author_id AS parent_author FROM posts p LEFT JOIN posts parent ON p.parent_id = parent.id \
             WHERE p.round <= ? AND p.author_id != ?",
            columns.join(", ")
        );
        let mut stmt = self.db.prepare(&sql)?;
        let rows = stmt
            .query_map(params![round, agent_id], |r| Ok((row_post(r)?, r.get::<_, Option<String>>(10)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let following = self.following(agent_id)?;
        let engaged = self.engagement_by(agent_id)?;
        let reach = self.author_reach()?;

        let mut ranked: Vec<((bool, u8, f64, u64), Post)> = rows
            .into_iter()
            .map(|(post, parent_author)| {
                let age = (round - post.round).max(0);
                let tier = if age >= TIER_WINDOW {
                    2
                } else if parent_author.as_deref() == Some(agent_id) {
                    0
                } else if following.contains(&post.author_id) {
                    1
                } else {
                    2
                };
                let score = engagement_weight(&post)
                    * reach.get(&post.author_id).copied().unwrap_or(0.5)
                    * RECENCY_DECAY.powf(age as f64);
                ((engaged.contains_key(&post.id), tier, -score, self.tiebreak(agent_id, post.id)), post)
            })
            .collect();
        // An infinite influence can make a score NaN (inf x 0). Python's sort tolerates it; here it sorts last, and
        // `+ 0.0` keeps -0.0 equal to 0.0 under the total order.
        let score_key = |x: f64| if x.is_nan() { f64::INFINITY } else { x + 0.0 };
        ranked.sort_by(|(a, _), (b, _)| {
            a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(score_key(a.2).total_cmp(&score_key(b.2))).then(a.3.cmp(&b.3))
        });
        let keep = if k >= 0 { k } else { (ranked.len() as i64 + k).max(0) };
        Ok(ranked.into_iter().take(keep as usize).map(|(_, p)| p).collect())
    }

    /// Post id -> how this agent already engaged with it ("liked", "disliked", "replied", "reposted").
    pub fn engagement_by(&self, agent_id: &str) -> Result<IndexMap<i64, Vec<String>>> {
        let mut out: IndexMap<i64, Vec<String>> = IndexMap::new();
        let mut stmt = self.db.prepare("SELECT post_id, kind FROM reactions WHERE agent_id = ? ORDER BY post_id")?;
        for row in stmt.query_map([agent_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (post, kind) = row?;
            out.entry(post).or_default().push(if kind == "like" { "liked" } else { "disliked" }.into());
        }
        let mut stmt = self.db.prepare(
            "SELECT parent_id, repost_of FROM posts WHERE author_id = ? \
             AND (parent_id IS NOT NULL OR repost_of IS NOT NULL) ORDER BY id",
        )?;
        for row in stmt.query_map([agent_id], |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?)))? {
            let (parent, repost) = row?;
            for (target, label) in [(parent, "replied"), (repost, "reposted")] {
                if let Some(target) = target {
                    let labels = out.entry(target).or_default();
                    if !labels.iter().any(|l| l == label) {
                        labels.push(label.into());
                    }
                }
            }
        }
        Ok(out)
    }

    /// Static influence blended with the share of the population that currently follows the author.
    fn author_reach(&self) -> Result<IndexMap<String, f64>> {
        let counts = self.follower_counts()?;
        self.with_personas(|personas| {
            let others = (personas.len() as i64 - 1).max(1) as f64;
            personas
                .iter()
                .map(|(pid, p)| {
                    let count = counts.get(pid).copied().unwrap_or(0) as f64;
                    (pid.clone(), 0.5 + p.influence + 0.5 * count / others)
                })
                .collect()
        })
    }

    fn tiebreak(&self, agent_id: &str, post_id: i64) -> u64 {
        let mut hasher = Blake2bVar::new(8).expect("8 is a valid BLAKE2b output size");
        hasher.update(format!("{}:{agent_id}:{post_id}", self.seed).as_bytes());
        let mut digest = [0u8; 8];
        hasher.finalize_variable(&mut digest).expect("output size matches");
        u64::from_be_bytes(digest)
    }

    // ---------- memory, stats, events ----------

    pub fn save_memory(&self, mem: &AgentMemory) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO memories (agent_id, notes) VALUES (?, ?)",
            params![mem.agent_id, json::py_dumps(&mem.notes)],
        )?;
        Ok(())
    }

    pub fn memory(&self, agent_id: &str) -> Result<AgentMemory> {
        let notes: Option<String> =
            self.db.query_row("SELECT notes FROM memories WHERE agent_id = ?", [agent_id], |r| r.get(0)).optional()?;
        let notes = match notes {
            Some(text) => serde_json::from_str(&text)?,
            None => Vec::new(),
        };
        Ok(AgentMemory { agent_id: agent_id.into(), notes })
    }

    /// Keep every agent's memory as it is at the end of `round`, so a branch can start from that round.
    pub fn snapshot_memories(&self, round: i64) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        tx.execute("DELETE FROM memory_history WHERE round = ?", [round])?;
        tx.execute(
            "INSERT INTO memory_history (agent_id, round, notes) SELECT agent_id, ?, notes FROM memories",
            [round],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Rounds whose end-of-round memories were kept (older runs: none). Fails on an older database opened
    /// read-only, which has no `memory_history` table.
    pub fn memory_rounds(&self) -> Result<Vec<i64>> {
        let mut stmt = self.db.prepare("SELECT DISTINCT round FROM memory_history ORDER BY round")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn last_round(&self) -> Result<i64> {
        let last: Option<i64> = self.db.query_row("SELECT MAX(round) FROM round_stats", [], |r| r.get(0))?;
        Ok(last.unwrap_or(0))
    }

    /// Delete everything that happened after `round` and restore the agents' memories as they were then.
    ///
    /// Reactions and follows made later are found through their actions; the posts' counters are recomputed from
    /// what remains, and the id counters restart after the last kept row, so a continued run numbers its posts on
    /// from there. Fails when the memories at `round` were not kept (older runs can branch from their last round,
    /// or from round 0 with empty memories).
    pub fn truncate_after(&self, round: i64) -> Result<()> {
        let history = self.memory_rounds()?;
        let last = self.last_round()?;
        if !history.contains(&round) && round != last && round != 0 {
            return Err(WorldError::Branch(format!(
                "this run did not keep its agents' memories for round {round}; \
                 it can be branched from its last round or from round 0"
            )));
        }
        let later: Vec<Action> = {
            let mut stmt = self.db.prepare("SELECT data FROM actions WHERE round > ?")?;
            let rows = stmt.query_map([round], |r| r.get::<_, String>(0))?;
            let mut out = Vec::new();
            for data in rows {
                out.push(serde_json::from_str(&data?)?);
            }
            out
        };
        let tx = self.db.unchecked_transaction()?;
        for a in &later {
            match (a.r#type, a.target_post_id, &a.target_agent_id) {
                (ActionType::Like | ActionType::Dislike, Some(post), _) => {
                    tx.execute("DELETE FROM reactions WHERE agent_id = ? AND post_id = ?", params![a.agent_id, post])?;
                }
                (ActionType::Follow, _, Some(target)) => {
                    tx.execute("DELETE FROM follows WHERE follower = ? AND followee = ?", params![a.agent_id, target])?;
                }
                _ => {}
            }
        }
        tx.execute("DELETE FROM post_sentiment WHERE post_id IN (SELECT id FROM posts WHERE round > ?)", [round])?;
        tx.execute("DELETE FROM reactions WHERE post_id IN (SELECT id FROM posts WHERE round > ?)", [round])?;
        for table in ["posts", "actions", "round_stats", "events"] {
            tx.execute(&format!("DELETE FROM {table} WHERE round > ?"), [round])?;
        }
        tx.execute(
            "UPDATE posts SET \
             likes = (SELECT COUNT(*) FROM reactions r WHERE r.post_id = posts.id AND r.kind = 'like'), \
             dislikes = (SELECT COUNT(*) FROM reactions r WHERE r.post_id = posts.id AND r.kind = 'dislike'), \
             replies = (SELECT COUNT(*) FROM posts c WHERE c.parent_id = posts.id), \
             reposts = (SELECT COUNT(*) FROM posts c WHERE c.repost_of = posts.id)",
            [],
        )?;
        for (table, key) in [("posts", "id"), ("actions", "seq"), ("events", "seq")] {
            tx.execute(
                &format!(
                    "UPDATE sqlite_sequence SET seq = (SELECT COALESCE(MAX({key}), 0) FROM {table}) WHERE name = ?"
                ),
                [table],
            )?;
        }
        if history.contains(&round) {
            tx.execute("DELETE FROM memories", [])?;
            tx.execute(
                "INSERT INTO memories (agent_id, notes) SELECT agent_id, notes FROM memory_history WHERE round = ?",
                [round],
            )?;
        } else if round == 0 && last != 0 {
            tx.execute("DELETE FROM memories", [])?; // an older run: round 0 starts from empty memories
        }
        tx.execute("DELETE FROM memory_history WHERE round > ?", [round])?;
        tx.commit()?;
        Ok(())
    }

    pub fn save_round_stats(&self, stats: &RoundStats) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO round_stats (round, data) VALUES (?, ?)",
            params![stats.round, json::to_string(stats)],
        )?;
        Ok(())
    }

    pub fn round_stats(&self) -> Result<Vec<RoundStats>> {
        let mut stmt = self.db.prepare("SELECT data FROM round_stats ORDER BY round")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for data in rows {
            out.push(serde_json::from_str(&data?)?);
        }
        Ok(out)
    }

    /// Store per-post sentiment (-1..1, clamped). Re-scoring a post overwrites its old score.
    pub fn save_post_sentiment(&self, scores: &[(i64, f64)]) -> Result<()> {
        if scores.is_empty() {
            return Ok(());
        }
        let tx = self.db.unchecked_transaction()?;
        for (pid, score) in scores {
            tx.execute(
                "INSERT OR REPLACE INTO post_sentiment (post_id, score) VALUES (?, ?)",
                params![pid, (-1.0f64).max(1.0f64.min(*score))],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Post id -> sentiment score, for every post the engine's sentiment pass has scored.
    pub fn post_sentiments(&self) -> Result<IndexMap<i64, f64>> {
        let mut stmt = self.db.prepare("SELECT post_id, score FROM post_sentiment ORDER BY post_id")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn add_event(&self, event: &GodEvent) -> Result<()> {
        self.db.execute("INSERT INTO events (round, content) VALUES (?, ?)", params![event.round, event.content])?;
        Ok(())
    }

    pub fn events(&self) -> Result<Vec<GodEvent>> {
        let mut stmt = self.db.prepare("SELECT round, content FROM events ORDER BY round, seq")?;
        let rows = stmt.query_map([], |r| Ok(GodEvent { round: r.get(0)?, content: r.get(1)? }))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}
