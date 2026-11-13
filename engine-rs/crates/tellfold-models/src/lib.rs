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

fn d_03() -> f64 {
    0.3
}

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
    /// described), "edited" (changed by the user before the run), "material" (written from the user's audience
    /// material, see `grounding`). None for older runs. Rust-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The slice of the user's audience material this persona was written from (`source` "material"). Kept when
    /// the user edits the persona or saves the cast as an audience. Rust-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grounding: Option<Grounding>,
}

/// What a persona drawn from the user's own material was based on. Rust-only, like [`MaterialSummary`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Grounding {
    /// The worry or hope the persona's cluster of material shares, in a few words.
    #[serde(default)]
    pub theme: String,
    /// How many items of each kind ("reviews", "survey", ...) that cluster holds, in [`MATERIAL_KINDS`] order.
    #[serde(default)]
    pub counts: IndexMap<String, i64>,
    /// The redacted quotes the persona's background and stance were written from.
    #[serde(default)]
    pub quotes: Vec<String>,
}

/// The kinds of audience material, in the order they are listed everywhere.
pub const MATERIAL_KINDS: [&str; 5] = ["reviews", "survey", "emails", "comments", "notes"];

/// "review" / "reviews" for an item count of one kind of material.
pub fn material_noun(kind: &str, n: i64) -> String {
    let (one, many) = match kind {
        "reviews" => ("review", "reviews"),
        "survey" => ("survey answer", "survey answers"),
        "emails" => ("support email", "support emails"),
        "comments" => ("social comment", "social comments"),
        "notes" => ("meeting note", "meeting notes"),
        _ => ("item", "items"),
    };
    format!("{n} {}", if n == 1 { one } else { many })
}

/// "14 reviews and 3 survey answers": the counts in words, kinds with none left out.
pub fn material_counts_text(counts: &IndexMap<String, i64>) -> String {
    let parts: Vec<String> = counts.iter().filter(|(_, n)| **n > 0).map(|(kind, n)| material_noun(kind, *n)).collect();
    match parts.as_slice() {
        [] => "no material".into(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What redaction of audience material hides, by category: (key in [`MaterialSummary::redacted`], one, many). The
/// one list every message about redaction is written from.
pub const REDACTION_CATEGORIES: [(&str, &str, &str); 6] = [
    ("names", "name", "names"),
    ("emails", "email address", "email addresses"),
    ("phones", "phone number", "phone numbers"),
    ("addresses", "street address", "street addresses"),
    ("numbers", "order or account number", "order or account numbers"),
    ("links", "link", "links"),
];

/// What the redaction rules remove, in words. Names are caught from greetings, sign-offs, titles, introductions and
/// a list of common first names, so a name the rules do not know can remain: every claim says so.
pub const REDACTION_TEXT: &str = "common first names, names in greetings, sign-offs and introductions, @handles, \
email addresses, phone numbers, street addresses, order or account numbers and links";

/// "4 names, 2 email addresses": what redaction hid, in words; "nothing needed hiding" when nothing was.
pub fn redacted_text(redacted: &IndexMap<String, i64>) -> String {
    let parts: Vec<String> = redacted
        .iter()
        .filter(|(_, n)| **n > 0)
        .map(|(key, n)| {
            let (one, many) = REDACTION_CATEGORIES
                .iter()
                .find(|(k, _, _)| k == key)
                .map_or(("other detail", "other details"), |(_, one, many)| (*one, *many));
            format!("{n} {}", if *n == 1 { one } else { many })
        })
        .collect();
    if parts.is_empty() {
        "nothing needed hiding".into()
    } else {
        parts.join(", ")
    }
}

/// The audience material a run's crowd is built from (`SimConfig.material`). The redacted items themselves are in
/// the run folder's `material.json`; the user's files never are. Rust-only.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MaterialSummary {
    /// Items that will be sent to the planner model, after redaction and the size cap.
    #[serde(default)]
    pub items: i64,
    /// Items by kind, in [`MATERIAL_KINDS`] order.
    #[serde(default)]
    pub counts: IndexMap<String, i64>,
    /// Items found but left out by the size cap.
    #[serde(default)]
    pub dropped: i64,
    /// What was hidden before anything left the Mac, by [`REDACTION_CATEGORIES`] key.
    #[serde(default)]
    pub redacted: IndexMap<String, i64>,
    /// The files the material came from, by neutral label ("reviews 1", "survey 1"): their names never leave the
    /// preview.
    #[serde(default)]
    pub files: Vec<String>,
    /// Only when the planner model could read fewer items than were sent (a failed call, an unknown camp): the items
    /// it read, by kind. The people were built from these; every other field still describes what was sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read: Option<IndexMap<String, i64>>,
}

impl MaterialSummary {
    /// The items the people were built from, by kind: [`MaterialSummary::read`] when set, else every item sent.
    pub fn based_on(&self) -> &IndexMap<String, i64> {
        self.read.as_ref().unwrap_or(&self.counts)
    }
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
            grounding: None,
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
    /// The user's own audience material the cast is grounded in (its redacted items are in `material.json`).
    /// Rust-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<MaterialSummary>,
    /// Which crowd rules the run follows: 0 for the original ones, which older runs keep reproducing exactly, and
    /// [`CROWD_RULES`] for a new run. Rust-only.
    #[serde(default, skip_serializing_if = "is_zero", deserialize_with = "lax::int")]
    pub crowd_rules: i64,
    /// The groups the public was cast from and the stance mix it was drawn from, in a run with [`STAKE_RULES`].
    /// Rust-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stakes: Option<CastStakes>,
}

/// One group of people an announcement affects, and what it changes for them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stake {
    pub group: String,
    pub stake: String,
    /// the rough part (0-1) of the people who would post about it that the group makes up
    #[serde(default = "d_03", deserialize_with = "lax::float")]
    pub share: f64,
}

/// Shares of a public by starting stance (0-1 each, adding up to 1 once bounded).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StanceMix {
    #[serde(default, deserialize_with = "lax::float")]
    pub supportive: f64,
    #[serde(default, deserialize_with = "lax::float")]
    pub critical: f64,
    #[serde(default, deserialize_with = "lax::float")]
    pub undecided: f64,
    #[serde(default, deserialize_with = "lax::float")]
    pub indifferent: f64,
}

/// How the public of a run with [`STAKE_RULES`] was planned: the groups the announcement affects, the stance mix
/// the planner expected and why, and the mix the cast was drawn from after the bounds that keep a minority on each
/// side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CastStakes {
    pub groups: Vec<Stake>,
    pub reason: String,
    /// as the planner gave it
    pub planned: StanceMix,
    /// after the bounds
    pub mix: StanceMix,
}

/// The crowd rules a new run follows (see [`SimConfig::crowd_rules`]). Version 1: organisation accounts never
/// announce anything the source material doesn't say, every round shows the situation and keeps users on it, everyone
/// gets a turn in the first rounds, and the report's direction comes from the crowd's first reaction. Version 2
/// ([`STAKE_RULES`]) adds: the public is cast from the groups the announcement affects with a stance mix the planner
/// expects for it (bounded, see [`CastStakes`]), organisation posts that state something the source doesn't are
/// dropped, and the report lists every objection. Version 3 ([`REACTION_RULES`]) adds: undecided and indifferent
/// people don't open with a complaint, people who have made their point mostly go quiet instead of escalating, and
/// sentiment is scored as the attitude to the announcement itself.
pub const CROWD_RULES: i64 = 3;

/// The first [`SimConfig::crowd_rules`] version with the planned cast, the fact check and the objections section.
pub const STAKE_RULES: i64 = 2;

/// The first [`SimConfig::crowd_rules`] version with the rules for the middle of the crowd, going quiet, and scoring
/// the attitude to the announcement. Measured on the crowd rules 2 runs of the 23 balanced accuracy cases, with
/// organisation accounts left out: about half of the first posts of people written as undecided (matched by their
/// written stance) scored against and 8% in favour, and people whose first post was in favour posted on average 0.55
/// lower afterwards. "Third and fourth passes" in docs/ACCURACY.md has what version 3 changed.
pub const REACTION_RULES: i64 = 3;

/// Overrides the crowd rules of a new run: `TELLFOLD_CROWD_RULES=0` runs the original ones, as the Python engine
/// does (its parity tests set it).
pub const CROWD_RULES_ENV: &str = "TELLFOLD_CROWD_RULES";

/// The crowd rules a new run follows: [`CROWD_RULES`] unless [`CROWD_RULES_ENV`] holds a number.
pub fn new_crowd_rules() -> i64 {
    std::env::var(CROWD_RULES_ENV).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(CROWD_RULES)
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &i64) -> bool {
    *n == 0
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
            material: None,
            crowd_rules: 0,
            stakes: None,
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
    /// The crowd's first reaction, counted by the engine (never asked of the model) in a run with
    /// [`SimConfig::crowd_rules`]; None for older reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_reaction: Option<FirstReaction>,
}

/// How the people (organisation accounts left out) first reacted: each one's first scored post, read as against
/// (below -0.2), in favour (above 0.2) or neither, and the overall direction that follows from those counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FirstReaction {
    /// "negative", "mixed" or "positive"
    pub direction: String,
    pub against: i64,
    pub in_favour: i64,
    pub neither: i64,
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
