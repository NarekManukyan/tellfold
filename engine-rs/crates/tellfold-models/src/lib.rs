//! Shared data models: the Rust port of `tellfold/models.py`.
//!
//! Field order, names and defaults follow the Python models exactly, because the JSON written from these types
//! must be the JSON pydantic writes (see [`json`]). Fields a model fills in accept pydantic's lax coercion
//! (see [`lax`]); validators that rewrite values (recommendation actions, dropped recommendations) are ported too.

pub mod json;
pub mod lax;
pub mod log;
pub mod text;

/// The engine's version: `tellfold.__version__` and the version in pyproject.toml, kept equal to the workspace
/// version in engine-rs/Cargo.toml (tellfold-cli's tests check all three).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

use indexmap::IndexMap;
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};

fn d_06() -> f64 {
    0.6
}
fn d_05() -> f64 {
    0.5
}

// ---------- knowledge graph ----------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    #[serde(deserialize_with = "lax::int")]
    pub id: i64,
    pub source: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub summary: String,
    #[serde(default, deserialize_with = "lax::int_list")]
    pub source_chunk_ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    pub source: String,
    pub target: String,
    pub label: String,
    #[serde(default)]
    pub evidence: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeGraph {
    #[serde(default)]
    pub entities: Vec<Entity>,
    #[serde(default)]
    pub relations: Vec<Relation>,
}

// ---------- agents ----------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Persona {
    pub id: String,
    pub handle: String,
    pub name: String,
    pub bio: String,
    pub background: String,
    pub stance: String,
    #[serde(default)]
    pub traits: Vec<String>,
    #[serde(default = "d_06", deserialize_with = "lax::float")]
    pub activity: f64,
    #[serde(default = "d_05", deserialize_with = "lax::float")]
    pub influence: f64,
    #[serde(default)]
    pub entity_id: Option<String>,
    /// The opinion group the cast was planned around (see `tellfold_engine::personas::CAMPS`), when known. Runs
    /// made before camps were planned have none. Rust-only: the Python engine has no camps, cast review or groups.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camp: Option<String>,
    /// Where the persona came from: "cast" (planned and written by the engine), "group" (a group the user
    /// described), "edited" (changed by the user before the run). None for older runs. Rust-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl Persona {
    /// A persona with the model defaults for every optional field.
    pub fn new(id: &str, handle: &str, name: &str, bio: &str, background: &str, stance: &str) -> Self {
        Persona {
            id: id.into(),
            handle: handle.into(),
            name: name.into(),
            bio: bio.into(),
            background: background.into(),
            stance: stance.into(),
            traits: Vec::new(),
            activity: 0.6,
            influence: 0.5,
            entity_id: None,
            camp: None,
            source: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentMemory {
    pub agent_id: String,
    #[serde(default)]
    pub notes: Vec<String>,
}

// ---------- social world ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionType {
    Post,
    Reply,
    Repost,
    Like,
    Dislike,
    Follow,
    Nothing,
}

impl ActionType {
    pub fn value(self) -> &'static str {
        match self {
            ActionType::Post => "post",
            ActionType::Reply => "reply",
            ActionType::Repost => "repost",
            ActionType::Like => "like",
            ActionType::Dislike => "dislike",
            ActionType::Follow => "follow",
            ActionType::Nothing => "nothing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    pub agent_id: String,
    pub r#type: ActionType,
    #[serde(default)]
    pub content: String,
    #[serde(default, deserialize_with = "lax::opt_int")]
    pub target_post_id: Option<i64>,
    #[serde(default)]
    pub target_agent_id: Option<String>,
    #[serde(default)]
    pub reasoning: String,
}

impl Action {
    pub fn new(agent_id: &str, kind: ActionType) -> Self {
        Action {
            agent_id: agent_id.into(),
            r#type: kind,
            content: String::new(),
            target_post_id: None,
            target_agent_id: None,
            reasoning: String::new(),
        }
    }
    pub fn content(mut self, text: &str) -> Self {
        self.content = text.into();
        self
    }
    pub fn post(mut self, id: i64) -> Self {
        self.target_post_id = Some(id);
        self
    }
    pub fn agent(mut self, id: &str) -> Self {
        self.target_agent_id = Some(id.into());
        self
    }
    pub fn reasoning(mut self, text: &str) -> Self {
        self.reasoning = text.into();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Post {
    #[serde(deserialize_with = "lax::int")]
    pub id: i64,
    #[serde(deserialize_with = "lax::int")]
    pub round: i64,
    pub author_id: String,
    pub content: String,
    #[serde(default, deserialize_with = "lax::opt_int")]
    pub parent_id: Option<i64>,
    #[serde(default, deserialize_with = "lax::opt_int")]
    pub repost_of: Option<i64>,
    #[serde(default, deserialize_with = "lax::int")]
    pub likes: i64,
    #[serde(default, deserialize_with = "lax::int")]
    pub dislikes: i64,
    #[serde(default, deserialize_with = "lax::int")]
    pub reposts: i64,
    #[serde(default, deserialize_with = "lax::int")]
    pub replies: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GodEvent {
    #[serde(deserialize_with = "lax::int")]
    pub round: i64,
    pub content: String,
}

impl GodEvent {
    pub fn new(round: i64, content: &str) -> Self {
        GodEvent { round, content: content.into() }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RoundStats {
    #[serde(deserialize_with = "lax::int")]
    pub round: i64,
    #[serde(default, deserialize_with = "lax::int_map")]
    pub actions: IndexMap<String, i64>,
    #[serde(default, deserialize_with = "lax::int")]
    pub new_posts: i64,
    #[serde(default, deserialize_with = "lax::int")]
    pub active_agents: i64,
    #[serde(default, deserialize_with = "lax::opt_float")]
    pub sentiment: Option<f64>,
    #[serde(default, deserialize_with = "lax::int")]
    pub llm_calls: i64,
    #[serde(default, deserialize_with = "lax::float")]
    pub llm_seconds: f64,
}

// ---------- run config / results ----------

fn d_agents() -> i64 {
    20
}
fn d_rounds() -> i64 {
    6
}
fn d_batch() -> i64 {
    6
}
fn d_feed() -> i64 {
    12
}
fn d_memory() -> i64 {
    8
}
fn d_haiku() -> String {
    "haiku".into()
}
fn d_sonnet() -> String {
    "sonnet".into()
}
fn d_concurrency() -> i64 {
    4
}
fn d_language() -> String {
    "English".into()
}
fn d_seed() -> i64 {
    7
}
fn d_runtime() -> String {
    "claude-code".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimConfig {
    pub question: String,
    pub seed_files: Vec<String>,
    #[serde(default = "d_agents", deserialize_with = "lax::int")]
    pub num_agents: i64,
    #[serde(default = "d_rounds", deserialize_with = "lax::int")]
    pub rounds: i64,
    #[serde(default = "d_batch", deserialize_with = "lax::int")]
    pub batch_size: i64,
    #[serde(default = "d_feed", deserialize_with = "lax::int")]
    pub feed_size: i64,
    #[serde(default = "d_memory", deserialize_with = "lax::int")]
    pub memory_size: i64,
    #[serde(default = "d_haiku")]
    pub agent_model: String,
    #[serde(default = "d_sonnet")]
    pub planner_model: String,
    #[serde(default = "d_concurrency", deserialize_with = "lax::int")]
    pub concurrency: i64,
    #[serde(default = "d_language")]
    pub language: String,
    #[serde(default)]
    pub events: Vec<GodEvent>,
    #[serde(default = "d_seed", deserialize_with = "lax::int")]
    pub seed: i64,
    #[serde(default, deserialize_with = "lax::boolean")]
    pub think: bool,
    #[serde(default = "d_runtime")]
    pub agent_runtime: String,
    #[serde(default = "d_runtime")]
    pub planner_runtime: String,
    #[serde(default)]
    pub cast_from: Option<String>,
    #[serde(default)]
    pub study_id: Option<String>,
    #[serde(default)]
    pub variant: Option<String>,
    #[serde(default)]
    pub branched_from: Option<String>,
    #[serde(default, deserialize_with = "lax::opt_int")]
    pub branch_round: Option<i64>,
    /// Pause once the cast is ready (status `pause: "personas_ready"`) until a client confirms or edits it.
    /// Rust-only, like `fair_turns` and `audience`: the Python engine has no cast review.
    #[serde(default, skip_serializing_if = "is_false", deserialize_with = "lax::boolean")]
    pub review_cast: bool,
    /// Make sure every camp gets a turn to post and strongly held views are not starved. Off for older runs, which
    /// keep reproducing exactly.
    #[serde(default, skip_serializing_if = "is_false", deserialize_with = "lax::boolean")]
    pub fair_turns: bool,
    /// The saved audience this run's cast came from (its name). Its people and topic map are written into the run
    /// folder before the run starts, so the graph and persona stages are skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl SimConfig {
    /// A config with the model defaults, like `SimConfig(question=..., seed_files=...)`.
    pub fn new(question: &str, seed_files: Vec<String>) -> Self {
        SimConfig {
            question: question.into(),
            seed_files,
            num_agents: 20,
            rounds: 6,
            batch_size: 6,
            feed_size: 12,
            memory_size: 8,
            agent_model: d_haiku(),
            planner_model: d_sonnet(),
            concurrency: 4,
            language: d_language(),
            events: Vec::new(),
            seed: 7,
            think: false,
            agent_runtime: d_runtime(),
            planner_runtime: d_runtime(),
            cast_from: None,
            study_id: None,
            variant: None,
            branched_from: None,
            branch_round: None,
            review_cast: false,
            fair_turns: false,
            audience: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportSection {
    pub title: String,
    pub body_markdown: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Camp {
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub members: Vec<String>,
    #[serde(default, deserialize_with = "lax::opt_float")]
    pub sentiment: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecommendationAction {
    Keep,
    Change,
    Add,
    Drop,
    Test,
}

/// Words a model may use instead of the five actions; anything else fails validation like any other bad enum.
const ACTION_SYNONYMS: &[(&str, &str)] = &[
    ("build", "add"),
    ("implement", "add"),
    ("introduce", "add"),
    ("remove", "drop"),
    ("cut", "drop"),
    ("stop", "drop"),
    ("avoid", "drop"),
    ("don't do", "drop"),
    ("do not do", "drop"),
    ("fix", "change"),
    ("rework", "change"),
    ("reword", "change"),
    ("adjust", "change"),
    ("modify", "change"),
    ("protect", "keep"),
    ("continue", "keep"),
    ("experiment", "test"),
    ("validate", "test"),
    ("try", "test"),
    ("pilot", "test"),
    ("investigate", "test"),
    ("research", "test"),
    ("explore", "test"),
    ("check", "test"),
];

fn recommendation_action<'de, D: Deserializer<'de>>(d: D) -> Result<RecommendationAction, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    let value = match value {
        serde_json::Value::String(s) => {
            let key = text::py_strip(&s).to_lowercase().replace('\u{2019}', "'");
            let key = text::py_split(&key).collect::<Vec<_>>().join(" ");
            let mapped = ACTION_SYNONYMS.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string()).unwrap_or(key);
            serde_json::Value::String(mapped)
        }
        other => other,
    };
    RecommendationAction::deserialize(value).map_err(de::Error::custom)
}

fn lowercase_confidence<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Confidence>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(d)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => {
            Confidence::deserialize(serde_json::Value::String(text::py_strip(&s).to_lowercase()))
                .map(Some)
                .map_err(de::Error::custom)
        }
        Some(other) => Confidence::deserialize(other).map(Some).map_err(de::Error::custom),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recommendation {
    #[serde(deserialize_with = "recommendation_action")]
    pub action: RecommendationAction,
    pub title: String,
    #[serde(default)]
    pub why: String,
    #[serde(default)]
    pub addresses: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub expected_effect: String,
    #[serde(default, deserialize_with = "lowercase_confidence")]
    pub confidence: Option<Confidence>,
}

/// One step with an unknown action or a missing title is left out instead of failing the whole report.
fn drop_bad_recommendations<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Recommendation>, D::Error> {
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::Array(items) => {
            Ok(items.into_iter().filter_map(|item| Recommendation::deserialize(item).ok()).collect())
        }
        _ => Err(de::Error::custom("recommendations should be a valid list")),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Misquote {
    #[serde(deserialize_with = "lax::int")]
    pub post_id: i64,
    pub quote: String,
    #[serde(default)]
    pub r#ref: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CitationAudit {
    #[serde(default, deserialize_with = "lax::int")]
    pub checked: i64,
    #[serde(default, deserialize_with = "lax::int_list")]
    pub unknown_posts: Vec<i64>,
    #[serde(default)]
    pub unknown_handles: Vec<String>,
    #[serde(default)]
    pub unknown_refs: Vec<String>,
    #[serde(default)]
    pub misquotes: Vec<Misquote>,
    #[serde(default, deserialize_with = "lax::boolean")]
    pub repaired: bool,
    #[serde(default, deserialize_with = "lax::int")]
    pub marked: i64,
}

impl CitationAudit {
    pub fn problems(&self) -> usize {
        self.unknown_posts.len() + self.unknown_handles.len() + self.unknown_refs.len() + self.misquotes.len()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub title: String,
    pub summary: String,
    pub prediction: String,
    pub confidence: Confidence,
    #[serde(default)]
    pub sections: Vec<ReportSection>,
    #[serde(default)]
    pub key_voices: Vec<String>,
    #[serde(default)]
    pub reasons: Vec<String>,
    #[serde(default)]
    pub camps: Vec<Camp>,
    #[serde(default)]
    pub risks: Vec<String>,
    #[serde(default)]
    pub caveats: Vec<String>,
    #[serde(default, deserialize_with = "drop_bad_recommendations")]
    pub recommendations: Vec<Recommendation>,
    /// Filled in by the engine, never asked of the model (kept out of the JSON schema).
    #[serde(default)]
    pub citation_audit: Option<CitationAudit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RewriteChange {
    pub what: String,
    pub why: String,
    #[serde(default, deserialize_with = "lax::int_list")]
    pub cites: Vec<i64>,
}

/// `rewrite.json` in a run folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RewriteSuggestion {
    pub revised_text: String,
    #[serde(default)]
    pub changes: Vec<RewriteChange>,
    #[serde(default)]
    pub kept_on_purpose: Vec<String>,
    #[serde(default)]
    pub source_name: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub citation_audit: Option<CitationAudit>,
}

// ---------- studies ----------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StudyRun {
    pub id: String,
    pub variant: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StudyKind {
    #[default]
    Compare,
    Repeat,
}

/// `studies/<id>/study.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Study {
    pub id: String,
    #[serde(default)]
    pub kind: StudyKind,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub runs: Vec<StudyRun>,
    #[serde(default, deserialize_with = "lax::int")]
    pub planned: i64,
}

impl Study {
    pub fn new(id: &str, created_at: &str) -> Self {
        Study { id: id.into(), kind: StudyKind::Compare, created_at: created_at.into(), runs: Vec::new(), planned: 0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariantSummary {
    pub variant: String,
    pub summary: String,
    #[serde(default)]
    pub objections: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariantObjections {
    pub variant: String,
    #[serde(default)]
    pub objections: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoundPoint {
    #[serde(deserialize_with = "lax::int")]
    pub round: i64,
    #[serde(default, deserialize_with = "lax::opt_float")]
    pub sentiment: Option<f64>,
    #[serde(default, deserialize_with = "lax::int")]
    pub posts: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CampDigest {
    pub name: String,
    #[serde(deserialize_with = "lax::int")]
    pub size: i64,
    #[serde(default, deserialize_with = "lax::opt_float")]
    pub sentiment: Option<f64>,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShiftDigest {
    pub agent_id: String,
    pub handle: String,
    #[serde(deserialize_with = "lax::int")]
    pub first_post: i64,
    #[serde(deserialize_with = "lax::int")]
    pub last_post: i64,
    #[serde(default, deserialize_with = "lax::opt_float")]
    pub change: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunDigest {
    pub letter: String,
    pub run_id: String,
    pub variant: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub headline: Option<String>,
    #[serde(default)]
    pub rounds: Vec<RoundPoint>,
    #[serde(default, deserialize_with = "lax::opt_float")]
    pub final_sentiment: Option<f64>,
    #[serde(default)]
    pub camps: Vec<CampDigest>,
    #[serde(default)]
    pub shifts: Vec<ShiftDigest>,
    #[serde(default)]
    pub objections: Vec<String>,
    #[serde(default, deserialize_with = "lax::int")]
    pub posts: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompareReport {
    pub headline: String,
    #[serde(default)]
    pub better_variant: Option<String>,
    #[serde(default)]
    pub why: String,
    #[serde(default)]
    pub per_variant: Vec<VariantSummary>,
    #[serde(default)]
    pub objections_only_in: Vec<VariantObjections>,
    #[serde(default)]
    pub caveats: Vec<String>,
    #[serde(default)]
    pub citation_audit: Option<CitationAudit>,
}

/// `studies/<id>/compare.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompareResult {
    pub study_id: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub runs: Vec<RunDigest>,
    pub report: CompareReport,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoundBand {
    #[serde(deserialize_with = "lax::int")]
    pub round: i64,
    #[serde(default, deserialize_with = "lax::opt_float")]
    pub low: Option<f64>,
    #[serde(default, deserialize_with = "lax::opt_float")]
    pub high: Option<f64>,
    #[serde(default, deserialize_with = "lax::int")]
    pub runs: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CampMatch {
    pub label: String,
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default, deserialize_with = "lax::int")]
    pub runs: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CampGroupOut {
    pub label: String,
    #[serde(default)]
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CampMatches {
    #[serde(default)]
    pub groups: Vec<CampGroupOut>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Consistent,
    #[default]
    Mixed,
    Unstable,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgreementCall {
    #[serde(default)]
    pub held: Vec<String>,
    #[serde(default)]
    pub split: Vec<String>,
    #[serde(default)]
    pub verdict: Verdict,
    #[serde(default)]
    pub summary: String,
}

/// `studies/<id>/agreement.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Agreement {
    pub study_id: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default, deserialize_with = "lax::int")]
    pub runs: i64,
    #[serde(default, deserialize_with = "lax::opt_float")]
    pub final_low: Option<f64>,
    #[serde(default, deserialize_with = "lax::opt_float")]
    pub final_high: Option<f64>,
    #[serde(default, deserialize_with = "lax::opt_float")]
    pub spread: Option<f64>,
    #[serde(default)]
    pub bands: Vec<RoundBand>,
    #[serde(default)]
    pub camps: Vec<CampMatch>,
    #[serde(default)]
    pub held: Vec<String>,
    #[serde(default)]
    pub split: Vec<String>,
    #[serde(default)]
    pub verdict: Verdict,
    #[serde(default)]
    pub summary: String,
}

// ---------- run lifecycle (status.json) ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    Queued,
    Running,
    Done,
    Failed,
    Stopped,
}

impl RunState {
    pub fn value(self) -> &'static str {
        match self {
            RunState::Queued => "queued",
            RunState::Running => "running",
            RunState::Done => "done",
            RunState::Failed => "failed",
            RunState::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStage {
    Reading,
    Graph,
    Personas,
    Seeding,
    Rounds,
    Report,
    Finished,
}

impl RunStage {
    pub fn value(self) -> &'static str {
        match self {
            RunStage::Reading => "reading",
            RunStage::Graph => "graph",
            RunStage::Personas => "personas",
            RunStage::Seeding => "seeding",
            RunStage::Rounds => "rounds",
            RunStage::Report => "report",
            RunStage::Finished => "finished",
        }
    }
}

/// `int | str`: a batch index, or `"sentiment"` for the per-round scoring call.
#[derive(Debug, Clone, PartialEq)]
pub enum BatchRef {
    Index(i64),
    Name(String),
}

impl Serialize for BatchRef {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            BatchRef::Index(i) => s.serialize_i64(*i),
            BatchRef::Name(n) => s.serialize_str(n),
        }
    }
}

impl<'de> Deserialize<'de> for BatchRef {
    /// pydantic's smart union: an exact int or str first, then lax int (integral floats, booleans).
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::String(s) => Ok(BatchRef::Name(s)),
            serde_json::Value::Bool(b) => Ok(BatchRef::Index(b as i64)),
            serde_json::Value::Number(n) => n
                .as_i64()
                .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
                .map(BatchRef::Index)
                .ok_or_else(|| de::Error::custom("batch should be an integer or a string")),
            _ => Err(de::Error::custom("batch should be an integer or a string")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FailedBatch {
    #[serde(deserialize_with = "lax::int")]
    pub round: i64,
    pub batch: BatchRef,
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub error: String,
}

/// `status.json` in a run folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunStatus {
    pub state: RunState,
    pub stage: RunStage,
    #[serde(default, deserialize_with = "lax::int")]
    pub round: i64,
    #[serde(deserialize_with = "lax::int")]
    pub total_rounds: i64,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default, deserialize_with = "lax::opt_int")]
    pub pid: Option<i64>,
    pub started_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub failed_batches: Vec<FailedBatch>,
    /// Why a running run is waiting for a client: "personas_ready" while the cast is up for review. Only a running
    /// run has one. Rust-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause: Option<String>,
}

// ---------- routing ----------

/// Purposes sent to the agent runtime (rounds, sentiment, interviews); every other purpose goes to the planner.
pub const AGENT_PURPOSES: &[&str] = &["engine.", "chat.interview"];

pub fn is_agent_purpose(purpose: &str) -> bool {
    AGENT_PURPOSES.iter().any(|p| purpose.starts_with(p))
}
