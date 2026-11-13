//! The round loop (port of `tellfold/engine.py`): who is active, what they see, what they decide, and how the
//! world changes.
//!
//! The engine borrows a [`World`], which is not `Sync`, so its futures are not `Send`: run them on one thread
//! (a current-thread runtime or a `LocalSet`). The LLM calls of one round still run concurrently.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::task::Poll;

use futures::future::{join_all, maybe_done, MaybeDone};
use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Map, Value};
use tellfold_llm::{json, JsonModel, Llm, LlmError};
use tellfold_models::json::{py_repr, py_round, py_round_int, py_sum};
use tellfold_models::lax;
use tellfold_models::text::{py_prefix, py_strip};
use tellfold_models::{
    Action, ActionType, AgentMemory, GodEvent, KnowledgeGraph, Persona, Post, RoundStats, SimConfig, REACTION_RULES,
    STAKE_RULES, TOPIC_RULES,
};
use tellfold_store::world::{World, WorldError};

use crate::pytext::{char_len, rstrip, squash};
use crate::rng::PyRandom;

pub const MAX_ACTIONS_PER_AGENT: usize = 3;
pub const MAX_NOTE_CHARS: usize = 280;
/// fraction of agents that write a round-0 opening post
pub const OPENING_SHARE: f64 = 0.3;
/// breaking news pulls lurkers in
pub const EVENT_ACTIVITY_BOOST: f64 = 0.15;
/// With `fair_turns`, someone who holds a view strongly is never idle for more than this many rounds in a row.
pub const STRONG_GAP: i64 = 2;

/// Prompt notes for a `fair_turns` run, by why the user is given the floor this round.
pub const TURN_UNHEARD: &str = "Not heard yet: nobody with this user's outlook has posted so far. This round they \
write one post or reply in their own voice saying where they stand, even if that is unsure or unbothered.";
pub const TURN_STRONG: &str = "This user feels strongly about the topic and has not posted yet: this round they \
write one post or reply saying what they think.";
/// With the crowd rules, everyone who has not posted by then is given the floor in the rounds up to this one.
pub const FIRST_TURN_ROUNDS: i64 = 2;
pub const TURN_FIRST: &str = "This user has not said anything yet: this round they write one post or reply giving \
their first reaction to the announcement, in their own voice, even if it is unsure or unbothered.";

fn action_alias(kind: &str) -> &str {
    match kind {
        "comment" | "respond" => "reply",
        "share" | "retweet" | "quote" => "repost",
        "upvote" => "like",
        "downvote" => "dislike",
        "none" | "skip" | "lurk" => "nothing",
        other => other,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    World(#[from] WorldError),
    /// `ValueError` in Python: a precondition the caller broke.
    #[error("{0}")]
    Value(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// The persona's entity, if it has one: Python tests `if p.entity_id`, so an empty id is none.
fn entity(p: &Persona) -> Option<&str> {
    p.entity_id.as_deref().filter(|e| !e.is_empty())
}

// ---------- LLM response schemas ----------

/// `target_post_id` accepts "#12" and "12" as well as numbers; other strings become null.
fn post_ref<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<i64>, D::Error> {
    let value = Value::deserialize(d)?;
    if let Value::String(s) = &value {
        let digits = py_strip(s).trim_start_matches('#');
        return Ok(if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
            digits.parse().ok()
        } else {
            None
        });
    }
    lax::opt_int(value).map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProposedAction {
    pub r#type: String,
    #[serde(default)]
    pub content: String,
    #[serde(default, deserialize_with = "post_ref")]
    pub target_post_id: Option<i64>,
    #[serde(default)]
    pub target_agent: Option<String>,
    #[serde(default)]
    pub reasoning: String,
}

impl ProposedAction {
    fn nothing() -> Self {
        ProposedAction {
            r#type: "nothing".into(),
            content: String::new(),
            target_post_id: None,
            target_agent: None,
            reasoning: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentDecision {
    pub agent_id: String,
    #[serde(default)]
    pub actions: Vec<ProposedAction>,
    #[serde(default)]
    pub memory_note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoundDecisions {
    pub decisions: Vec<AgentDecision>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpeningPost {
    pub agent_id: String,
    pub content: String,
    #[serde(default)]
    pub memory_note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpeningPosts {
    pub posts: Vec<OpeningPost>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PostScore {
    #[serde(deserialize_with = "lax::int")]
    pub post_id: i64,
    #[serde(deserialize_with = "lax::float")]
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SentimentScores {
    pub scores: Vec<PostScore>,
}

/// The numbers of the organisation posts that state something the source doesn't (see [`FACT_CHECK_SYSTEM`]).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FactCheck {
    #[serde(default)]
    pub invented: Vec<Value>,
}

tellfold_llm::json_model!(RoundDecisions, "../../../schemas/RoundDecisions.json");
tellfold_llm::json_model!(OpeningPosts, "../../../schemas/OpeningPosts.json");
tellfold_llm::json_model!(SentimentScores, "../../../schemas/SentimentScores.json");
// Rust-only, so it lives with the crate.
tellfold_llm::json_model!(FactCheck, "../schemas/FactCheck.json");

// ---------- prompts ----------

const ROUND_SYSTEM: &str = r#"You drive several independent users of a social media platform in a social simulation.
Each user is a distinct person or organisation with their own background, interests, stance and voice. For
every user listed you decide what they do during this round, after they have scrolled their own feed.

Behave like a real platform, not a debate club:
- Most people mostly read. The most common outcome is doing nothing, or a single like. Replies come when
  something provokes, worries or excites a user. A brand-new original post is rarer and comes mainly from
  highly active users, organisations, or people with a strong reaction to fresh news.
- Respect each user's activity level: a low-activity user should usually do nothing or just like something.
- Users react to what is actually in their feed. They may agree, push back, joke, ask questions, share
  personal experience, or ignore the topic entirely.
- Opinions can move. Repeated exposure, a persuasive argument, social proof or breaking news may soften or
  harden a user's view; stubborn users barely move. Shifts are gradual and must fit the person.
- Voices must differ: vary length, tone, vocabulary and certainty. Organisations speak in an official or PR
  register. Avoid hashtag spam, slogans, and everybody sounding alike. Posts stay under 280 characters.
- Never invent facts presented as certain unless the user would plausibly believe or spread them.

Hard rules:
- A user may only reference post ids shown in THAT user's own feed (reply, repost, like, dislike).
- A user can like or dislike a post only once: never like or dislike a post marked as already liked or disliked.
- A user may only follow an author who appears in their own feed, and never themselves.
- Give each user 1 to 3 actions. Use "nothing" when they just scroll past.
- Write all post, reply and memory text in {language}. Keep JSON keys, action types and agent ids exactly as specified.

Reply with JSON only, shaped like:
{"decisions": [{"agent_id": "a03", "actions": [{"type": "like", "target_post_id": 12, "reasoning": "..."},
{"type": "reply", "target_post_id": 12, "content": "...", "reasoning": "..."}],
"memory_note": "..."}]}
Action types: post (content), reply (target_post_id, content), repost (target_post_id, optional content),
like / dislike (target_post_id), follow (target_agent), nothing.
memory_note: one first-person sentence for the user's own memory: what stood out this round and where their
view on the topic now stands (unchanged, softened, hardened, or newly formed)."#;

const OPENING_SYSTEM: &str = r#"You write the first posts of a social simulation, on a platform where the conversation about a
topic is just starting. Each listed user writes ONE original post in their own voice, grounded in their
background and starting stance and in the situation described.

- Make the posts differ in angle, length, tone and certainty; organisations speak officially, individuals
  personally. Some users are passionate, some casual, some only tangentially interested.
- Under 280 characters each, no hashtag spam, no two posts making the same point the same way.
- Write all post and memory text in {language}. Keep agent ids exactly as given.

Reply with JSON only, shaped like:
{"posts": [{"agent_id": "a01", "content": "...", "memory_note": "..."}]}
memory_note: one first-person sentence about why they posted and how they currently feel about the topic."#;

/// Added to the round and opening system prompts of a run with the crowd rules ([`SimConfig::crowd_rules`]).
const FACT_RULES: &str = r#"

Facts and topic:
- The situation from the source material is everything that is known. An organisation or official account may
  restate, explain or defend what it says, but never announces anything it doesn't: no new concessions, exemptions,
  figures, data, dates, promises or changes of plan. Nobody else reports such things as fact either.
- Stay on the announcement in the situation and on what users have actually said in the feed. Don't bring in other
  plans, decisions, events or controversies that neither of them mentions."#;

/// Added to the round and opening system prompts of a run with [`REACTION_RULES`]. Without it, the undecided and
/// indifferent people of the balanced accuracy cases mostly opened with a complaint, and supporters who had made their
/// point kept posting until they sounded like critics.
const REACTION_RULES_TEXT: &str = r#"

The middle of the crowd:
- A user whose starting stance is undecided or conflicted stays that way until something specific in their feed
  persuades them. Their posts weigh what they like against what worries them, or ask a question; they don't only
  complain.
- An indifferent user mostly scrolls past. When they do post, it is about the practical effect on them, without
  taking a side.
- Supporters can ask for more, sooner, or in writing and still be supporters: wanting a good thing done well is not
  turning against it.
- Once a user has made their point, they mostly read. A user who got what they wanted, or whose question was
  answered, rarely keeps posting about it, and nobody grows angrier round after round unless something new in their
  feed gives them a reason."#;

/// Added to the sentiment prompt of a run with [`REACTION_RULES`]: supporters' posts asking what comes next were
/// scored as opposition to the decision itself.
const SENTIMENT_RULES: &str = r#"

Score the attitude to the announcement or decision itself, not to the conversation around it:
- A post that welcomes it but asks for more, sooner, clearer details or a written guarantee is positive.
- A post that weighs good and bad points about evenly, or mainly asks a question, is close to 0.
- Agreeing or disagreeing with another user counts only through what the post says about the announcement."#;

/// After a source cut at [`SOURCE_CHARS`] in the fact check: posts about the part not shown are not failed for it.
const SOURCE_CUT: &str = "\n[The source material continues beyond this point but is not shown. Don't fail a post only \
because it states something that could come from the part not shown.]";

/// The characters of the seed documents an organisation post is checked against ([`Engine::with_source`]).
pub const SOURCE_CHARS: usize = 12_000;

/// The check on organisation posts in a run with [`STAKE_RULES`]: the round prompt asks them not to announce
/// anything new, and in the second accuracy pass they still did late in a run (docs/ACCURACY.md).
pub const FACT_CHECK_SYSTEM: &str = r#"You check posts written for organisation accounts in a social-media simulation
against the source material, which is everything that is known about the situation. A post fails when it states,
offers or announces anything the source doesn't contain: a concession, exemption, option, pilot, review,
consultation, talks, figure, date, promise or change of plan. Restating, explaining or defending what the source
says is fine, and so are opinions, thanks, sympathy, general promises to listen and declining to comment. When in
doubt about a specific new fact, it fails. Reply with JSON only."#;

const SENTIMENT_SYSTEM: &str = r#"You are a careful annotator of social media posts. For each post, rate the attitude it
expresses toward the subject of the question below, from -1 (strongly negative, pessimistic or opposed)
through 0 (neutral, mixed or off-topic) to +1 (strongly positive, optimistic or supportive). A plain repost
endorses the original post. Judge the author's own attitude, not the topic's mood in general.

Reply with JSON only: {"scores": [{"post_id": 12, "score": -0.4}]} with one entry per post."#;

/// `str.format(language=...)` on the round and opening system prompts (their other braces are doubled in Python).
fn with_language(template: &str, language: &str) -> String {
    template.replace("{language}", language)
}

// ---------- engine ----------

/// LLM usage for one round.
#[derive(Default)]
struct Ledger {
    calls: Cell<i64>,
    seconds: Cell<f64>,
}

/// What one agent is shown this round, and therefore what it may reference.
#[derive(Debug, Clone)]
pub struct AgentView {
    pub persona: Persona,
    pub feed: Vec<Post>,
    pub allowed_posts: HashSet<i64>,
    pub visible_authors: HashSet<String>,
    /// a `fair_turns` note giving this user the floor this round ([`TURN_UNHEARD`] or [`TURN_STRONG`])
    pub turn: Option<&'static str>,
}

impl AgentView {
    pub fn new(persona: Persona, feed: Vec<Post>) -> Self {
        AgentView { persona, feed, allowed_posts: HashSet::new(), visible_authors: HashSet::new(), turn: None }
    }
}

/// `on_event(name, payload)`: the payload is a JSON object, keys in Python's order.
pub type EventCallback<'a> = Box<dyn Fn(&str, Value) + 'a>;

pub struct Engine<'a> {
    pub llm: &'a dyn Llm,
    pub world: &'a World,
    pub config: &'a SimConfig,
    emit: EventCallback<'a>,
    /// the source material's digest, shown in every round with the crowd rules ([`Engine::with_graph`])
    situation: String,
    /// the source material itself, which organisation posts are checked against ([`Engine::with_source`])
    source: String,
    /// whether the seed text ran past [`SOURCE_CHARS`] and `source` is its start
    source_cut: bool,
}

impl<'a> Engine<'a> {
    pub fn new(llm: &'a dyn Llm, world: &'a World, config: &'a SimConfig, on_event: Option<EventCallback<'a>>) -> Self {
        Engine {
            llm,
            world,
            config,
            emit: on_event.unwrap_or_else(|| Box::new(|_, _| {})),
            situation: String::new(),
            source: String::new(),
            source_cut: false,
        }
    }

    /// The topic map the run was seeded from: with the crowd rules every round prompt shows its digest, as the
    /// opening prompt always does, so users stay on the announcement.
    pub fn with_graph(mut self, graph: &KnowledgeGraph) -> Self {
        self.situation = graph_digest(graph, 25);
        self
    }

    /// The text of the seed documents (the first [`SOURCE_CHARS`] characters). With [`STAKE_RULES`] every
    /// organisation post is checked against it, and dropped when it states something the source doesn't.
    pub fn with_source(mut self, text: &str) -> Self {
        self.source = text.chars().take(SOURCE_CHARS).collect();
        self.source_cut = text.chars().nth(SOURCE_CHARS).is_some();
        self
    }

    fn rules(&self) -> bool {
        self.config.crowd_rules >= TOPIC_RULES
    }

    /// The indexes of the organisation posts (author, text) that state something the source doesn't, checked by
    /// the planner model. Empty without [`STAKE_RULES`] or a source, and when the check fails (the posts stay).
    async fn invented(&self, r: i64, posts: &[(&Persona, &str)], ledger: &Ledger) -> HashSet<usize> {
        if self.config.crowd_rules < STAKE_RULES || self.source.is_empty() || posts.is_empty() {
            return HashSet::new();
        }
        let lines: Vec<String> = posts
            .iter()
            .enumerate()
            .map(|(i, (p, text))| format!("[{}] @{} ({}): {}", i + 1, p.handle, p.name, squash(text)))
            .collect();
        // a longer source is cut at SOURCE_CHARS: the checker is told, so a post about the part it can't see passes
        let cut = if self.source_cut { SOURCE_CUT } else { "" };
        let prompt = format!(
            "SOURCE MATERIAL (everything that is known):\n{}{cut}\n\nPOSTS BY ORGANISATION ACCOUNTS:\n{}\n\n\
Return {{\"invented\": [the numbers of the posts that fail]}}, an empty list when none does.",
            self.source,
            lines.join("\n")
        );
        let result = self
            .call_on::<FactCheck>(&self.config.planner_model, &prompt, FACT_CHECK_SYSTEM, "engine.factcheck", ledger)
            .await;
        let flagged: HashSet<usize> = match result {
            Ok(out) => out
                .invented
                .iter()
                .filter_map(|v| {
                    v.as_i64().or_else(|| {
                        v.as_str().and_then(|s| s.trim().trim_start_matches('[').trim_end_matches(']').parse().ok())
                    })
                })
                .filter(|&n| n >= 1 && (n as usize) <= posts.len())
                .map(|n| n as usize - 1)
                .collect(),
            Err(e) => {
                self.emit(
                    "batch_failed",
                    json!({"round": r, "batch": "factcheck", "agents": [], "error": e.to_string()}),
                );
                return HashSet::new();
            }
        };
        if !flagged.is_empty() {
            let mut dropped: Vec<usize> = flagged.iter().copied().collect();
            dropped.sort_unstable();
            let dropped: Vec<Value> =
                dropped.iter().map(|&i| json!({"agent_id": posts[i].0.id, "content": posts[i].1})).collect();
            self.emit("posts_dropped", json!({"round": r, "reason": "invented", "posts": dropped}));
        }
        flagged
    }

    /// A round or opening system prompt, with the fact rules in a run with the crowd rules and the rules for the
    /// middle of the crowd from [`REACTION_RULES`].
    fn system(&self, template: &str) -> String {
        let mut system = with_language(template, &self.config.language);
        if self.rules() {
            system += FACT_RULES;
        }
        if self.config.crowd_rules >= REACTION_RULES {
            system += REACTION_RULES_TEXT;
        }
        system
    }

    /// The sentiment system prompt: scored against the announcement itself from [`REACTION_RULES`].
    fn sentiment_system(&self) -> String {
        if self.config.crowd_rules >= REACTION_RULES {
            format!("{SENTIMENT_SYSTEM}{SENTIMENT_RULES}")
        } else {
            SENTIMENT_SYSTEM.to_string()
        }
    }

    fn emit(&self, name: &str, payload: Value) {
        (self.emit)(name, payload);
    }

    // ---------- seeding ----------

    pub async fn seed_world(&self, graph: &KnowledgeGraph) -> Result<()> {
        let personas = self.world.personas()?;
        if personas.is_empty() {
            return Err(EngineError::Value("seed_world needs personas in the world".into()));
        }
        self.emit("stage", json!({"stage": "follow_graph"}));
        let edges = build_follow_graph(&personas, graph, self.config.seed);
        for (follower, followee) in &edges {
            self.world.add_follow(follower, followee)?;
        }
        self.emit("stage", json!({"stage": "follow_graph_done", "edges": edges.len()}));

        let events = self.inject_events(0)?;
        let authors = if self.config.fair_turns {
            pick_opening_authors_fair(&personas, self.config.seed)
        } else {
            pick_opening_authors(&personas, self.config.seed)
        };
        self.emit("stage", json!({"stage": "opening_posts", "authors": authors.len()}));
        let ledger = Ledger::default();
        let batches = chunks(&authors, self.config.batch_size);
        let context = graph_digest(graph, 25);
        let mut prompts = Vec::with_capacity(batches.len());
        for batch in &batches {
            prompts.push(self.opening_prompt(batch, &context, &events)?);
        }
        let results = join_all(
            batches
                .iter()
                .zip(&prompts)
                .enumerate()
                .map(|(i, (batch, prompt))| self.opening_batch(i, batch, prompt, &ledger)),
        )
        .await;
        let mut posts: Vec<OpeningPost> = results.into_iter().flatten().flatten().collect();
        posts.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        let allowed: HashSet<&str> = authors.iter().map(|p| p.id.as_str()).collect();
        let mut written: HashSet<String> = HashSet::new();
        let mut post_ids: Vec<i64> = Vec::new();
        for op in &posts {
            if !allowed.contains(op.agent_id.as_str()) || written.contains(&op.agent_id) {
                continue;
            }
            written.insert(op.agent_id.clone());
            let action = Action::new(&op.agent_id, ActionType::Post).content(&op.content);
            if let Some(post_id) = self.world.apply(0, &action)? {
                post_ids.push(post_id);
            }
            self.remember(&op.agent_id, 0, &op.memory_note)?;
        }
        if !post_ids.is_empty() {
            self.emit("posts_added", json!({"round": 0, "post_ids": post_ids}));
        }
        let new_posts = self.world.posts(Some(0))?;
        let stats = RoundStats {
            round: 0,
            actions: self.action_counts(0)?,
            new_posts: new_posts.len() as i64,
            active_agents: authors.len() as i64,
            sentiment: self.sentiment(0, &new_posts, &ledger).await?,
            llm_calls: ledger.calls.get(),
            llm_seconds: py_round(ledger.seconds.get(), 2),
        };
        self.world.save_round_stats(&stats)?;
        self.world.snapshot_memories(0)?;
        self.emit("stage", json!({"stage": "opening_posts_done", "posts": new_posts.len()}));
        self.emit("round_done", self.round_done(&stats));
        Ok(())
    }

    fn opening_prompt(&self, batch: &[Persona], context: &str, events: &[GodEvent]) -> Result<String> {
        let mut parts = vec![
            format!("Question being simulated: {}", self.config.question),
            if context.is_empty() {
                String::new()
            } else {
                format!("Situation (from the source material):\n{context}")
            },
            news_block(events, 0),
            "Users who post now:".to_string(),
        ];
        for p in batch {
            parts.push(persona_block(p, &self.world.memory(&p.id)?));
        }
        Ok(parts.join("\n\n"))
    }

    async fn opening_batch(
        &self,
        index: usize,
        batch: &[Persona],
        prompt: &str,
        ledger: &Ledger,
    ) -> Option<Vec<OpeningPost>> {
        let system = self.system(OPENING_SYSTEM);
        match self.call::<OpeningPosts>(prompt, &system, "engine.seed", ledger).await {
            Ok(out) => {
                let by_id: HashMap<&str, &Persona> = batch.iter().map(|p| (p.id.as_str(), p)).collect();
                let organisation =
                    |op: &OpeningPost| by_id.get(op.agent_id.as_str()).copied().filter(|p| entity(p).is_some());
                let spots: Vec<usize> =
                    (0..out.posts.len()).filter(|&i| organisation(&out.posts[i]).is_some()).collect();
                let posts: Vec<(&Persona, &str)> = spots
                    .iter()
                    .filter_map(|&i| organisation(&out.posts[i]).map(|p| (p, out.posts[i].content.as_str())))
                    .collect();
                let flagged: HashSet<usize> =
                    self.invented(0, &posts, ledger).await.into_iter().map(|k| spots[k]).collect();
                Some(out.posts.into_iter().enumerate().filter(|(i, _)| !flagged.contains(i)).map(|(_, p)| p).collect())
            }
            Err(e) => {
                let agents: Vec<&str> = batch.iter().map(|p| p.id.as_str()).collect();
                self.emit(
                    "batch_failed",
                    json!({"round": 0, "batch": index, "agents": agents, "error": e.to_string()}),
                );
                None
            }
        }
    }

    // ---------- rounds ----------

    pub async fn run(&self) -> Result<Vec<RoundStats>> {
        let mut results = Vec::new();
        for r in 1..=self.config.rounds {
            results.push(self.run_round(r).await?);
        }
        Ok(results)
    }

    pub async fn run_round(&self, r: i64) -> Result<RoundStats> {
        let events = self.inject_events(r)?;
        let personas = self.world.personas()?;
        let news = events.iter().any(|e| e.round == r);
        let mut active = sample_active(&personas, r, self.config.seed, news)?;
        let mut turns: HashMap<String, &'static str> = HashMap::new();
        let first_turn_round = self.rules() && r <= FIRST_TURN_ROUNDS;
        let posted = if self.config.fair_turns || first_turn_round { self.world.authors()? } else { HashSet::new() };
        if self.config.fair_turns {
            let last_active = self.world.last_active_rounds()?;
            let (extra, given) = fair_turns(&personas, &active, &posted, &last_active, r);
            active.extend(extra);
            turns = given;
        }
        if first_turn_round {
            let (extra, given) = first_turns(&personas, &active, &posted, r);
            active.extend(extra);
            for (id, note) in given {
                turns.entry(id).or_insert(note);
            }
        }
        let batches = chunks(&active, self.config.batch_size);
        let current: Vec<&str> = events.iter().filter(|e| e.round == r).map(|e| e.content.as_str()).collect();
        let ids: Vec<&str> = active.iter().map(|p| p.id.as_str()).collect();
        self.emit(
            "round_start",
            json!({
                "round": r,
                "active": active.len(),
                "total_rounds": self.config.rounds,
                "active_agents": ids,
                "batches": batches.len(),
                "events": current,
            }),
        );
        let ledger = Ledger::default();
        let mut views: HashMap<String, AgentView> = HashMap::new();
        for p in &active {
            let mut view = self.view(p, r)?;
            view.turn = turns.get(&p.id).copied();
            views.insert(p.id.clone(), view);
        }
        // Every prompt is written before any batch is applied: in Python the batch tasks all start (and build
        // their prompts) before the first one is awaited.
        let mut batch_views: Vec<Vec<&AgentView>> = Vec::new();
        let mut prompts: Vec<String> = Vec::new();
        for batch in &batches {
            let bv: Vec<&AgentView> = batch.iter().map(|p| &views[&p.id]).collect();
            prompts.push(self.round_prompt(r, &bv, &events)?);
            batch_views.push(bv);
        }
        let tasks: Vec<_> = batch_views
            .iter()
            .zip(&prompts)
            .enumerate()
            .map(|(i, (bv, prompt))| self.decide_batch(r, i, bv, prompt, &ledger))
            .collect();
        let handles = self.handle_index()?;
        let mut new_posts = 0i64;
        // Batches decide concurrently but are applied in batch order (agent id order inside a batch), so the
        // result is deterministic while posts still stream out as soon as the earlier batches are in.
        in_order(tasks, |decisions| {
            let decisions: BTreeMap<String, AgentDecision> =
                decisions.unwrap_or_default().into_iter().map(|d| (d.agent_id.clone(), d)).collect();
            let mut post_ids: Vec<i64> = Vec::new();
            for (agent_id, decision) in &decisions {
                let mut proposed: Vec<ProposedAction> =
                    decision.actions.iter().take(MAX_ACTIONS_PER_AGENT).cloned().collect();
                if proposed.is_empty() {
                    proposed.push(ProposedAction::nothing());
                }
                for p in &proposed {
                    if let Some(post_id) = self.world.apply(r, &to_action(&views[agent_id], p, &handles))? {
                        post_ids.push(post_id);
                    }
                }
                self.remember(agent_id, r, &decision.memory_note)?;
            }
            new_posts += post_ids.len() as i64;
            if !post_ids.is_empty() {
                self.emit("posts_added", json!({"round": r, "post_ids": post_ids}));
            }
            Ok(())
        })
        .await?;

        let stats = RoundStats {
            round: r,
            actions: self.action_counts(r)?,
            new_posts,
            active_agents: active.len() as i64,
            sentiment: self.sentiment(r, &self.world.posts(Some(r))?, &ledger).await?,
            llm_calls: ledger.calls.get(),
            llm_seconds: py_round(ledger.seconds.get(), 2),
        };
        self.world.save_round_stats(&stats)?;
        self.world.snapshot_memories(r)?;
        self.emit("round_done", self.round_done(&stats));
        Ok(stats)
    }

    fn round_done(&self, stats: &RoundStats) -> Value {
        let mut payload = match serde_json::to_value(stats).expect("stats serialize") {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        payload.insert("total_rounds".into(), json!(self.config.rounds));
        Value::Object(payload)
    }

    fn view(&self, persona: &Persona, r: i64) -> Result<AgentView> {
        let feed = self.world.feed_for(&persona.id, r, self.config.feed_size)?;
        let mut view = AgentView::new(persona.clone(), feed);
        for post in &view.feed {
            view.allowed_posts.insert(post.id);
            view.visible_authors.insert(post.author_id.clone());
            for reference in [post.parent_id, post.repost_of].into_iter().flatten() {
                let shown = self.world.post(reference)?;
                view.allowed_posts.insert(shown.id);
                view.visible_authors.insert(shown.author_id);
            }
        }
        Ok(view)
    }

    async fn decide_batch(
        &self,
        r: i64,
        index: usize,
        views: &[&AgentView],
        prompt: &str,
        ledger: &Ledger,
    ) -> Option<Vec<AgentDecision>> {
        let system = self.system(ROUND_SYSTEM);
        let out = match self.call::<RoundDecisions>(prompt, &system, "engine.round", ledger).await {
            Ok(out) => out,
            Err(e) => {
                let agents: Vec<&str> = views.iter().map(|v| v.persona.id.as_str()).collect();
                self.emit(
                    "batch_failed",
                    json!({"round": r, "batch": index, "agents": agents, "error": e.to_string()}),
                );
                return None;
            }
        };
        let expected: HashSet<&str> = views.iter().map(|v| v.persona.id.as_str()).collect();
        let mut seen: HashSet<String> = HashSet::new();
        let mut decisions: Vec<AgentDecision> = out
            .decisions
            .into_iter()
            .filter(|d| expected.contains(d.agent_id.as_str()) && seen.insert(d.agent_id.clone()))
            .collect();
        // (decision, action) of every organisation action with text, in order
        let by_id: HashMap<&str, &Persona> = views.iter().map(|v| (v.persona.id.as_str(), &v.persona)).collect();
        let mut spots: Vec<(usize, usize)> = Vec::new();
        let mut posts: Vec<(&Persona, &str)> = Vec::new();
        for (i, d) in decisions.iter().enumerate() {
            let Some(persona) = by_id.get(d.agent_id.as_str()).filter(|p| entity(p).is_some()) else { continue };
            for (j, a) in d.actions.iter().take(MAX_ACTIONS_PER_AGENT).enumerate() {
                if !py_strip(&a.content).is_empty() {
                    spots.push((i, j));
                    posts.push((persona, &a.content));
                }
            }
        }
        let flagged = self.invented(r, &posts, ledger).await;
        for k in flagged {
            let (i, j) = spots[k];
            decisions[i].actions[j] = ProposedAction::nothing();
        }
        Some(decisions)
    }

    fn round_prompt(&self, r: i64, views: &[&AgentView], events: &[GodEvent]) -> Result<String> {
        let handles: HashMap<String, String> = self.world.personas()?.into_iter().map(|p| (p.id, p.handle)).collect();
        let mut blocks = vec![
            format!("Question being simulated: {}", self.config.question),
            format!("Round {r} of {}.", self.config.rounds),
            if self.rules() && !self.situation.is_empty() {
                format!("Situation (from the source material):\n{}", self.situation)
            } else {
                String::new()
            },
            news_block(events, r),
            format!(
                "Users to decide for: {}. Each block holds one user's private profile, memory and own feed.",
                views.len()
            ),
        ];
        for v in views {
            let p = &v.persona;
            let following: HashSet<String> = self.world.following(&p.id)?.into_iter().collect();
            let engaged = self.world.engagement_by(&p.id)?;
            blocks.push(
                [
                    {
                        let block = persona_block(p, &self.world.memory(&p.id)?);
                        match v.turn {
                            Some(note) => format!("{block}\n{note}"),
                            None => block,
                        }
                    },
                    format!(
                        "Follows {} accounts, followed by {}.",
                        following.len(),
                        self.world.followers(&p.id)?.len()
                    ),
                    own_posts_block(self.world, &p.id, r, 3)?,
                    feed_block(self.world, &v.feed, &handles, &following, &engaged)?,
                ]
                .join("\n"),
            );
        }
        let ids: Vec<&str> = views.iter().map(|v| v.persona.id.as_str()).collect();
        blocks.push(format!("Return exactly one decision per user: {}.", ids.join(", ")));
        Ok(blocks.into_iter().filter(|b| !b.is_empty()).collect::<Vec<_>>().join("\n\n"))
    }

    fn action_counts(&self, r: i64) -> Result<IndexMap<String, i64>> {
        let mut counts: IndexMap<String, i64> = IndexMap::new();
        for (_, action) in self.world.actions(Some(r))? {
            *counts.entry(action.r#type.value().to_string()).or_default() += 1;
        }
        Ok(counts)
    }

    fn handle_index(&self) -> Result<HashMap<String, String>> {
        let mut index = HashMap::new();
        for p in self.world.personas()? {
            index.insert(p.id.clone(), p.id.clone());
            index.insert(p.handle.to_lowercase(), p.id.clone());
        }
        Ok(index)
    }

    // ---------- shared helpers ----------

    async fn call<T: JsonModel>(
        &self,
        prompt: &str,
        system: &str,
        purpose: &str,
        ledger: &Ledger,
    ) -> std::result::Result<T, LlmError> {
        self.call_on(&self.config.agent_model, prompt, system, purpose, ledger).await
    }

    /// [`Self::call`] on another model (the planner's, for the fact check), counted in the same ledger.
    async fn call_on<T: JsonModel>(
        &self,
        model: &str,
        prompt: &str,
        system: &str,
        purpose: &str,
        ledger: &Ledger,
    ) -> std::result::Result<T, LlmError> {
        let started = Instant::now();
        let result = json::<T>(self.llm, prompt, system, model, purpose).await;
        ledger.calls.set(ledger.calls.get() + 1);
        ledger.seconds.set(ledger.seconds.get() + started.elapsed().as_secs_f64());
        result
    }

    async fn sentiment(&self, r: i64, posts: &[Post], ledger: &Ledger) -> Result<Option<f64>> {
        if posts.is_empty() {
            return Ok(None);
        }
        let mut lines = Vec::with_capacity(posts.len());
        for post in posts {
            let mut text = if post.content.is_empty() { "(no comment)".to_string() } else { post.content.clone() };
            if let Some(original) = post.repost_of {
                let original = self.world.post(original)?;
                text = py_strip(&format!("[repost of: {}] {}", original.content, post.content)).to_string();
            } else if let Some(parent) = post.parent_id {
                let parent = self.world.post(parent)?;
                text = format!("[reply to: {}] {text}", clip(&parent.content, 140));
            }
            lines.push(format!("#{}: {text}", post.id));
        }
        let prompt = format!("Question: {}\n\nPosts from round {r}:\n{}", self.config.question, lines.join("\n"));
        let out =
            match self.call::<SentimentScores>(&prompt, &self.sentiment_system(), "engine.sentiment", ledger).await {
                Ok(out) => out,
                Err(e) => {
                    self.emit(
                        "batch_failed",
                        json!({"round": r, "batch": "sentiment", "agents": [], "error": e.to_string()}),
                    );
                    return Ok(None);
                }
            };
        let wanted: HashSet<i64> = posts.iter().map(|p| p.id).collect();
        let mut per_post: IndexMap<i64, f64> = IndexMap::new();
        for s in out.scores.iter().filter(|s| wanted.contains(&s.post_id)) {
            per_post.insert(s.post_id, py_max(-1.0, py_min(1.0, s.score)));
        }
        let pairs: Vec<(i64, f64)> = per_post.iter().map(|(&k, &v)| (k, v)).collect();
        self.world.save_post_sentiment(&pairs)?;
        if pairs.is_empty() {
            return Ok(None);
        }
        Ok(Some(py_round(py_sum(pairs.iter().map(|p| p.1)) / pairs.len() as f64, 3)))
    }

    /// Stores this round's god events and returns every event up to and including round r.
    ///
    /// Events may also be added straight to the world while the run is in progress (from the UI); the world is
    /// re-read every round, and any event for round r that is not one of `config.events` is announced with
    /// `event_injected`.
    fn inject_events(&self, r: i64) -> Result<Vec<GodEvent>> {
        let planned: Vec<&str> =
            self.config.events.iter().filter(|e| e.round == r).map(|e| e.content.as_str()).collect();
        for content in &planned {
            self.world.add_event(&GodEvent::new(r, content))?;
        }
        let events: Vec<GodEvent> = self.world.events()?.into_iter().filter(|e| e.round <= r).collect();
        let mut remaining: HashMap<&str, usize> = HashMap::new();
        for content in &planned {
            *remaining.entry(content).or_default() += 1;
        }
        for event in events.iter().filter(|e| e.round == r) {
            match remaining.get_mut(event.content.as_str()) {
                Some(n) if *n > 0 => *n -= 1,
                _ => self.emit("event_injected", json!({"round": r, "content": event.content})),
            }
        }
        Ok(events)
    }

    fn remember(&self, agent_id: &str, r: i64, note: &str) -> Result<()> {
        let note = squash(note);
        if note.is_empty() {
            return Ok(());
        }
        let mem = self.world.memory(agent_id)?;
        let mut notes = mem.notes;
        notes.push(format!("R{r}: {}", clip(&note, MAX_NOTE_CHARS)));
        let notes = py_tail(notes, self.config.memory_size);
        self.world.save_memory(&AgentMemory { agent_id: agent_id.into(), notes })?;
        Ok(())
    }
}

/// Runs `tasks` concurrently and hands their outputs to `apply` in order, like awaiting a list of asyncio tasks
/// one by one: every task that can make progress does so before the next output is applied, so a task that
/// finishes early (and emits its events) does so before earlier tasks are applied. Dropping the future cancels
/// the tasks still running.
async fn in_order<F: Future>(tasks: Vec<F>, mut apply: impl FnMut(F::Output) -> Result<()>) -> Result<()> {
    let mut tasks: Vec<Pin<Box<MaybeDone<F>>>> = tasks.into_iter().map(|t| Box::pin(maybe_done(t))).collect();
    for i in 0..tasks.len() {
        poll_fn(|cx| {
            for task in &mut tasks[i..] {
                let _ = task.as_mut().poll(cx);
            }
            match *tasks[i] {
                MaybeDone::Done(_) => Poll::Ready(()),
                _ => Poll::Pending,
            }
        })
        .await;
        let output = tasks[i].as_mut().take_output().expect("task finished");
        apply(output)?;
    }
    Ok(())
}

/// `items[-k:]` (with Python's quirk that `[-0:]` is the whole list).
fn py_tail<T>(mut items: Vec<T>, k: i64) -> Vec<T> {
    let len = items.len() as i64;
    let start = if k > 0 { (len - k).max(0) } else { (-k).min(len) };
    items.drain(..start as usize);
    items
}

/// Python's `min(a, b)`: `a` unless `b` is smaller (NaN never is).
fn py_min(a: f64, b: f64) -> f64 {
    if b < a {
        b
    } else {
        a
    }
}

/// Python's `max(a, b)`: `a` unless `b` is larger.
fn py_max(a: f64, b: f64) -> f64 {
    if b > a {
        b
    } else {
        a
    }
}

// ---------- pure helpers (seeded, deterministic) ----------

fn by_id(personas: &[Persona]) -> Vec<Persona> {
    let mut ordered = personas.to_vec();
    ordered.sort_by(|a, b| a.id.cmp(&b.id));
    ordered
}

/// Each agent acts with probability `activity` (boosted on news rounds); never an empty round.
pub fn sample_active(personas: &[Persona], r: i64, seed: i64, news: bool) -> Result<Vec<Persona>> {
    // The "shoal:" salts predate the rename to Tellfold. Changing them would reshuffle every seeded crowd, so
    // they stay and a seed still reproduces the runs made before the rename.
    let mut rng = PyRandom::from_str_seed(&format!("shoal:active:{seed}:{r}"));
    let boost = if news { EVENT_ACTIVITY_BOOST } else { 0.0 };
    let ordered = by_id(personas);
    let mut active: Vec<Persona> =
        ordered.iter().filter(|p| rng.random() < py_min(1.0, p.activity + boost)).cloned().collect();
    if active.is_empty() && !ordered.is_empty() {
        let weights: Vec<f64> = ordered.iter().map(|p| p.activity + 0.01).collect();
        let i = rng
            .choice_weighted(&weights)
            .ok_or_else(|| EngineError::Value("Total of weights must be greater than zero".into()))?;
        active = vec![ordered[i].clone()];
    }
    rng.shuffle(&mut active); // mix batch composition so the same agents are not always decided together
    Ok(active)
}

/// Who else acts this round in a `fair_turns` run, and who is given the floor.
///
/// `active` is this round's [`sample_active`] draw, `posted` the authors of every post so far, `last_active` the
/// last round each agent acted in. Two rules add people (in id order, after the draw):
/// - a camp nobody in it has posted for yet always has someone in the round: its most active member, the one idle
///   longest on a tie;
/// - someone whose camp holds its view strongly is never idle for more than [`STRONG_GAP`] rounds in a row.
///
/// Returns the added personas and the floor notes: [`TURN_UNHEARD`] for every active member of an unheard camp,
/// [`TURN_STRONG`] for every other active strong voice who has not posted yet. People without a camp (entity
/// accounts, older casts) are left to the draw.
pub fn fair_turns(
    personas: &[Persona],
    active: &[Persona],
    posted: &HashSet<String>,
    last_active: &HashMap<String, i64>,
    r: i64,
) -> (Vec<Persona>, HashMap<String, &'static str>) {
    let ordered = by_id(personas);
    let mut chosen: HashSet<String> = active.iter().map(|p| p.id.clone()).collect();
    let mut extra: Vec<Persona> = Vec::new();
    let last = |p: &Persona| last_active.get(&p.id).copied().unwrap_or(0);
    let mut unheard: HashSet<&str> = HashSet::new();
    for camp in crate::personas::CAMPS {
        let members = camp_members(&ordered, camp);
        if members.is_empty() || members.iter().any(|p| posted.contains(&p.id)) {
            continue;
        }
        unheard.insert(camp);
        if members.iter().any(|p| chosen.contains(&p.id)) {
            continue;
        }
        // most active first, then idle longest, then id
        let pick = first_of(&members, |a, b| {
            b.activity.partial_cmp(&a.activity).unwrap_or(std::cmp::Ordering::Equal).then(last(a).cmp(&last(b)))
        });
        chosen.insert(pick.id.clone());
        extra.push(pick.clone());
    }
    for p in &ordered {
        let strong = p.camp.as_deref().is_some_and(crate::personas::is_strong);
        if strong && !chosen.contains(&p.id) && r - last(p) > STRONG_GAP {
            chosen.insert(p.id.clone());
            extra.push(p.clone());
        }
    }
    extra.sort_by(|a, b| a.id.cmp(&b.id));
    let mut turns = HashMap::new();
    for p in ordered.iter().filter(|p| chosen.contains(&p.id)) {
        let Some(camp) = p.camp.as_deref() else { continue };
        if unheard.contains(camp) {
            turns.insert(p.id.clone(), TURN_UNHEARD);
        } else if crate::personas::is_strong(camp) && !posted.contains(&p.id) {
            turns.insert(p.id.clone(), TURN_STRONG);
        }
    }
    (extra, turns)
}

/// Who else acts in round `r` under the crowd rules, so that everyone has posted by [`FIRST_TURN_ROUNDS`]: in the
/// last of those rounds everyone who has not posted yet joins the draw (in id order, after it). Every active person
/// who has not posted yet, joined or drawn, is given the floor with [`TURN_FIRST`]; `posted` holds the authors of
/// every post so far. Organisation accounts count too: they are given the floor, not new facts.
pub fn first_turns(
    personas: &[Persona],
    active: &[Persona],
    posted: &HashSet<String>,
    r: i64,
) -> (Vec<Persona>, HashMap<String, &'static str>) {
    let drawn: HashSet<&str> = active.iter().map(|p| p.id.as_str()).collect();
    let unposted: Vec<Persona> = by_id(personas).into_iter().filter(|p| !posted.contains(&p.id)).collect();
    let extra: Vec<Persona> = if r >= FIRST_TURN_ROUNDS {
        unposted.iter().filter(|p| !drawn.contains(p.id.as_str())).cloned().collect()
    } else {
        Vec::new()
    };
    let turns = unposted
        .iter()
        .filter(|p| drawn.contains(p.id.as_str()) || r >= FIRST_TURN_ROUNDS)
        .map(|p| (p.id.clone(), TURN_FIRST))
        .collect();
    (extra, turns)
}

/// [`pick_opening_authors`], plus in a `fair_turns` run the most active and influential member of every camp the
/// draw left out, so each camp opens the conversation.
pub fn pick_opening_authors_fair(personas: &[Persona], seed: i64) -> Vec<Persona> {
    let mut chosen = pick_opening_authors(personas, seed);
    let ordered = by_id(personas);
    for camp in crate::personas::CAMPS {
        let members = camp_members(&ordered, camp);
        if members.is_empty() || chosen.iter().any(|p| p.camp.as_deref() == Some(camp)) {
            continue;
        }
        let pick = first_of(&members, |a, b| {
            (b.activity + b.influence).partial_cmp(&(a.activity + a.influence)).unwrap_or(std::cmp::Ordering::Equal)
        });
        chosen.push(pick.clone());
    }
    chosen.sort_by(|a, b| a.id.cmp(&b.id));
    chosen
}

/// The members of `camp` in id order (`ordered` is already sorted by id).
fn camp_members<'a>(ordered: &'a [Persona], camp: &str) -> Vec<&'a Persona> {
    ordered.iter().filter(|p| p.camp.as_deref() == Some(camp)).collect()
}

/// The first of `members` under `order`; ties keep id order. `members` is never empty.
fn first_of<'a>(members: &[&'a Persona], order: impl Fn(&Persona, &Persona) -> std::cmp::Ordering) -> &'a Persona {
    members.iter().copied().min_by(|a, b| order(a, b)).expect("a camp with members")
}

/// Opening posts come mostly from entity accounts and the most active, influential people.
pub fn pick_opening_authors(personas: &[Persona], seed: i64) -> Vec<Persona> {
    let mut rng = PyRandom::from_str_seed(&format!("shoal:opening:{seed}"));
    let m = py_round_int(personas.len() as f64 * OPENING_SHARE).max(1) as usize;
    let weight = |p: &Persona| (0.1 + p.activity + p.influence) * if entity(p).is_some() { 2.0 } else { 1.0 };
    let mut chosen = weighted_sample(&by_id(personas), weight, m, &mut rng);
    chosen.sort_by(|a, b| a.id.cmp(&b.id));
    chosen
}

/// Influence-weighted preferential attachment with a bit of homophily.
///
/// Entity accounts attract more followers but follow few. Accounts whose entities are related in the knowledge
/// graph tend to follow each other; people sharing traits do too.
pub fn build_follow_graph(personas: &[Persona], graph: &KnowledgeGraph, seed: i64) -> Vec<(String, String)> {
    let mut rng = PyRandom::from_str_seed(&format!("shoal:follows:{seed}"));
    let ordered = by_id(personas);
    let mut related: HashSet<(&str, &str)> = HashSet::new();
    for r in &graph.relations {
        related.insert((&r.source, &r.target));
        related.insert((&r.target, &r.source));
    }
    let lower_traits = |p: &Persona| -> HashSet<String> { p.traits.iter().map(|t| t.to_lowercase()).collect() };
    let mut edges = Vec::new();
    for follower in &ordered {
        let others: Vec<Persona> = ordered.iter().filter(|p| p.id != follower.id).cloned().collect();
        if others.is_empty() {
            break;
        }
        let wanted = if entity(follower).is_some() {
            1 + py_round_int(follower.activity * 3.0)
        } else {
            2 + py_round_int(follower.activity * 6.0)
        };
        let follower_traits = lower_traits(follower);
        let weight = |t: &Persona| -> f64 {
            let mut w = (0.15 + t.influence).powi(2) * if entity(t).is_some() { 2.5 } else { 1.0 };
            if let (Some(f), Some(e)) = (entity(follower), entity(t)) {
                if related.contains(&(f, e)) {
                    w *= 3.0;
                }
            }
            let shared = follower_traits.intersection(&lower_traits(t)).count();
            w * (1.0 + 0.5 * shared as f64)
        };
        let k = (wanted.max(0) as usize).min(others.len());
        for target in weighted_sample(&others, weight, k, &mut rng) {
            edges.push((follower.id.clone(), target.id));
        }
    }
    edges
}

/// Weighted sampling without replacement (Efraimidis-Spirakis keys).
fn weighted_sample<T: Clone>(items: &[T], weight: impl Fn(&T) -> f64, k: usize, rng: &mut PyRandom) -> Vec<T> {
    let mut keyed: Vec<(f64, usize)> =
        items.iter().enumerate().map(|(i, x)| (rng.random().powf(1.0 / py_max(weight(x), 1e-6)), i)).collect();
    // keyed.sort(reverse=True): largest key first, then the larger index
    keyed.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(b.1.cmp(&a.1)));
    keyed.into_iter().take(k).map(|(_, i)| items[i].clone()).collect()
}

/// Converts one proposal, downgrading anything outside this agent's own view to `nothing`.
pub fn to_action(view: &AgentView, p: &ProposedAction, handles: &HashMap<String, String>) -> Action {
    let agent_id = view.persona.id.as_str();
    let kind = py_strip(&p.r#type).to_lowercase();
    let kind = action_alias(&kind).to_string();
    let content = py_strip(&p.content);
    let nothing = |problem: String| {
        Action::new(agent_id, ActionType::Nothing)
            .reasoning(py_strip(&format!("[invalid {kind}: {problem}] {}", p.reasoning)))
    };
    let Ok(action_type) = serde_json::from_value::<ActionType>(json!(kind)) else {
        return nothing("unknown action type".into());
    };
    if matches!(action_type, ActionType::Reply | ActionType::Repost | ActionType::Like | ActionType::Dislike)
        && !p.target_post_id.is_some_and(|id| view.allowed_posts.contains(&id))
    {
        let shown = p.target_post_id.map_or("None".to_string(), |id| id.to_string());
        return nothing(format!("post {shown} not in own feed"));
    }
    if action_type == ActionType::Follow {
        let raw = py_strip(p.target_agent.as_deref().unwrap_or("")).trim_start_matches('@').to_lowercase();
        return match handles.get(&raw).filter(|t| view.visible_authors.contains(*t)) {
            Some(target) => Action::new(agent_id, action_type).agent(target).reasoning(&p.reasoning),
            None => {
                let shown = py_repr(&p.target_agent.as_ref().map_or(Value::Null, |a| json!(a)));
                nothing(format!("follow target {shown} not in own feed"))
            }
        };
    }
    let mut action = Action::new(agent_id, action_type).reasoning(&p.reasoning);
    if matches!(action_type, ActionType::Post | ActionType::Reply | ActionType::Repost) {
        action.content = content.into();
    }
    if action_type != ActionType::Post {
        action.target_post_id = p.target_post_id;
    }
    action
}

fn chunks<T: Clone>(items: &[T], size: i64) -> Vec<Vec<T>> {
    items.chunks(size.max(1) as usize).map(<[T]>::to_vec).collect()
}

/// Squashes whitespace and trims to `limit` code points with "…".
pub fn clip(text: &str, limit: usize) -> String {
    let text = squash(text);
    if char_len(&text) <= limit {
        text
    } else {
        format!("{}…", rstrip(py_prefix(&text, limit - 1)))
    }
}

fn activity_label(activity: f64) -> &'static str {
    if activity < 0.3 {
        "low (mostly lurks)"
    } else if activity < 0.6 {
        "medium (reads a lot, chimes in now and then)"
    } else {
        "high (posts and replies often)"
    }
}

pub fn persona_block(p: &Persona, memory: &AgentMemory) -> String {
    let mut lines = vec![
        format!("=== {} @{} ===", p.id, p.handle),
        format!("Name: {}{}", p.name, if entity(p).is_some() { " (organisation / public account)" } else { "" }),
        format!("Public bio: {}", p.bio),
        format!("Private background: {}", p.background),
        format!("Starting stance on the topic: {}", p.stance),
        format!("Traits: {}", if p.traits.is_empty() { "none noted".to_string() } else { p.traits.join(", ") }),
        format!("Activity level: {}", activity_label(p.activity)),
    ];
    if !memory.notes.is_empty() {
        lines.push("Memory (oldest first):".into());
        lines.extend(memory.notes.iter().map(|n| format!("- {n}")));
    }
    lines.join("\n")
}

fn own_posts_block(world: &World, agent_id: &str, r: i64, limit: usize) -> Result<String> {
    let own: Vec<Post> = world.posts(None)?.into_iter().filter(|p| p.author_id == agent_id && p.round < r).collect();
    let own = &own[own.len().saturating_sub(limit)..];
    if own.is_empty() {
        return Ok(String::new());
    }
    let mut lines = vec!["Your recent posts (for consistency; you cannot react to these):".to_string()];
    lines.extend(own.iter().map(|p| {
        format!(
            "- round {}: {} [likes {}, dislikes {}, reposts {}, replies {}]",
            p.round,
            clip(if p.content.is_empty() { "(plain repost)" } else { &p.content }, 160),
            p.likes,
            p.dislikes,
            p.reposts,
            p.replies
        )
    }));
    Ok(lines.join("\n"))
}

fn feed_block(
    world: &World,
    feed: &[Post],
    handles: &HashMap<String, String>,
    following: &HashSet<String>,
    engaged: &IndexMap<i64, Vec<String>>,
) -> Result<String> {
    if feed.is_empty() {
        return Ok("Feed: (empty - nothing to react to yet; only post, follow nobody, or do nothing)".into());
    }
    let handle = |id: &str| handles.get(id).cloned().unwrap_or_else(|| id.to_string());
    let mut lines = vec!["Feed (the only posts this user may reference):".to_string()];
    for post in feed {
        let mut author = format!("@{} ({})", handle(&post.author_id), post.author_id);
        if following.contains(&post.author_id) {
            author.push_str(" [you follow]");
        }
        let done = engaged
            .get(&post.id)
            .map(|kinds| format!(" [you already {} this]", kinds.join(" and ")))
            .unwrap_or_default();
        lines.push(format!(
            "[#{}] {author}, round {} - likes {}, dislikes {}, reposts {}, replies {}{done}",
            post.id, post.round, post.likes, post.dislikes, post.reposts, post.replies
        ));
        if let Some(original) = post.repost_of {
            let original = world.post(original)?;
            if !post.content.is_empty() {
                lines.push(format!("  {}", clip(&post.content, 280)));
            }
            lines.push(format!(
                "  reposted [#{}] @{} ({}): {}",
                original.id,
                handle(&original.author_id),
                original.author_id,
                clip(&original.content, 280)
            ));
            continue;
        }
        if let Some(parent) = post.parent_id {
            let parent = world.post(parent)?;
            lines.push(format!(
                "  replying to [#{}] @{} ({}): {}",
                parent.id,
                handle(&parent.author_id),
                parent.author_id,
                clip(&parent.content, 140)
            ));
        }
        lines.push(format!("  {}", clip(&post.content, 280)));
    }
    Ok(lines.join("\n"))
}

pub fn news_block(events: &[GodEvent], r: i64) -> String {
    let current: Vec<&str> = events.iter().filter(|e| e.round == r).map(|e| e.content.as_str()).collect();
    let earlier: Vec<&str> = events.iter().filter(|e| e.round < r).map(|e| e.content.as_str()).collect();
    let mut lines = Vec::new();
    if !current.is_empty() {
        lines.push("BREAKING NEWS this round (every user has just seen this):".to_string());
        lines.extend(current.iter().map(|c| format!("- {c}")));
    }
    if !earlier.is_empty() {
        lines.push("Earlier news:".to_string());
        lines.extend(earlier.iter().map(|c| format!("- {c}")));
    }
    lines.join("\n")
}

pub fn graph_digest(graph: &KnowledgeGraph, limit: usize) -> String {
    let names: HashMap<&str, &str> = graph.entities.iter().map(|e| (e.id.as_str(), e.name.as_str())).collect();
    let mut lines: Vec<String> = graph
        .entities
        .iter()
        .take(limit)
        .map(|e| format!("- {} ({}): {}", e.name, e.kind, clip(&e.summary, 160)))
        .collect();
    lines.extend(graph.relations.iter().take(limit).filter_map(|r| {
        match (names.get(r.source.as_str()), names.get(r.target.as_str())) {
            (Some(s), Some(t)) => Some(format!("- {s} {} {t}", r.label)),
            _ => None,
        }
    }));
    lines.join("\n")
}
