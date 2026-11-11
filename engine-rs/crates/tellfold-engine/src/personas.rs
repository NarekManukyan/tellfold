//! Persona generation (port of `tellfold/personas.py`): a planned cast (entity voices and a varied public) filled
//! in by the LLM in batches.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use tellfold_llm::{json, Llm, LlmError};
use tellfold_models::json::{py_round, py_round_int, py_sum};
use tellfold_models::lax;
use tellfold_models::text::{py_prefix, py_strip};
use tellfold_models::{Entity, Grounding, KnowledgeGraph, Persona, SimConfig, MATERIAL_KINDS};
use unicode_normalization::UnicodeNormalization;

use crate::material::{count_kinds, Cluster, MaterialItem, QUOTES_PER_PERSONA};
use crate::rng::PyRandom;

pub const BATCH_SIZE: usize = 10;
pub const ENTITY_SHARE: f64 = 0.3;
pub const EMBODIED_KINDS: [&str; 2] = ["organization", "person"];
pub const HANDLE_MAX: usize = 20;
pub const MAX_TRAITS: usize = 5;
pub const DIGEST_ENTITIES: usize = 30;
pub const DIGEST_RELATIONS: usize = 25;

/// (starting leaning, weight). Most people hold moderate or no strong view; a minority is vocal.
pub const STANCE_MIX: [(&str, f64); 6] = [
    ("leans supportive, holds the view moderately", 0.21),
    ("leans critical, holds the view moderately", 0.21),
    ("undecided or conflicted, can see both sides", 0.18),
    ("indifferent, barely follows the topic and only reacts if it touches daily life", 0.16),
    ("strongly supportive and vocal about it", 0.12),
    ("strongly critical and vocal about it", 0.12),
];
/// An engagement label and its activity band.
pub type Engagement = (&'static str, (f64, f64));

/// ((label, activity band), weight). Most accounts mostly read; few post a lot.
pub const ENGAGEMENT_MIX: [(Engagement, f64); 3] = [
    (("lurker: mostly reads, rarely posts", (0.05, 0.3)), 0.4),
    (("occasional: posts or replies now and then", (0.3, 0.6)), 0.4),
    (("very active: posts and argues often", (0.6, 0.95)), 0.2),
];
pub const AGE_BANDS: [&str; 6] = ["18-24", "25-34", "35-44", "45-54", "55-64", "65+"];

/// The camp key of each [`STANCE_MIX`] leaning, in the same order. A persona's `camp` is one of these.
pub const CAMPS: [&str; 6] =
    ["lean_support", "lean_critical", "undecided", "indifferent", "strong_support", "strong_critical"];

/// The camp of a [`STANCE_MIX`] leaning.
pub fn camp_of(stance: &str) -> Option<&'static str> {
    STANCE_MIX.iter().position(|(s, _)| *s == stance).map(|i| CAMPS[i])
}

/// Whether a camp holds its view strongly (in either direction).
pub fn is_strong(camp: &str) -> bool {
    camp.starts_with("strong_")
}

/// Persona `source` values.
pub const SOURCE_CAST: &str = "cast";
pub const SOURCE_GROUP: &str = "group";
pub const SOURCE_EDITED: &str = "edited";
/// Written from the user's own audience material (see [`generate_grounded_cast`]).
pub const SOURCE_MATERIAL: &str = "material";
/// Every `source` a persona may have; [`tidy_cast`] drops any other.
pub const SOURCES: [&str; 4] = [SOURCE_CAST, SOURCE_GROUP, SOURCE_EDITED, SOURCE_MATERIAL];
/// Characters of a quote shown to the model in a slot brief.
const BRIEF_QUOTE_CHARS: usize = 300;

/// The most people one described group may add, and the most a cast may hold.
pub const MAX_GROUP: usize = 20;
pub const MAX_CAST: usize = 200;

fn d_05() -> f64 {
    0.5
}
fn d_03() -> f64 {
    0.3
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonaDraft {
    pub slot: String,
    pub handle: String,
    pub name: String,
    pub bio: String,
    pub background: String,
    pub stance: String,
    #[serde(default)]
    pub traits: Vec<String>,
    #[serde(default = "d_05", deserialize_with = "lax::float")]
    pub activity: f64,
    #[serde(default = "d_03", deserialize_with = "lax::float")]
    pub influence: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PersonaBatch {
    #[serde(default)]
    pub personas: Vec<PersonaDraft>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VoicePick {
    /// most relevant first
    #[serde(default)]
    pub entity_ids: Vec<String>,
}

/// One person of a group the user described, with the camp the model placed them in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupDraft {
    pub slot: String,
    pub handle: String,
    pub name: String,
    pub bio: String,
    pub background: String,
    pub stance: String,
    #[serde(default)]
    pub traits: Vec<String>,
    #[serde(default = "d_05", deserialize_with = "lax::float")]
    pub activity: f64,
    #[serde(default = "d_03", deserialize_with = "lax::float")]
    pub influence: f64,
    #[serde(default)]
    pub camp: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GroupBatch {
    #[serde(default)]
    pub personas: Vec<GroupDraft>,
}

tellfold_llm::json_model!(PersonaBatch, "../../../schemas/PersonaBatch.json");
// Rust-only (the Python engine has no groups), so it lives with the crate: golden/make.py rewrites schemas/.
tellfold_llm::json_model!(GroupBatch, "../schemas/GroupBatch.json");
tellfold_llm::json_model!(VoicePick, "../../../schemas/VoicePick.json");

pub const VOICES_SYSTEM: &str = "\
You choose which organisations and public figures get their own account in a social-media simulation \
about a question. Pick only entities that would plausibly post publicly about this topic, in roughly \
this order: the subject itself (the company or person the question is about), direct competitors, \
journalists, critics, advocacy or professional bodies, regulators, and well-known commentators. \
Leave out background vendors and infrastructure that ordinary people never hear from on this topic: \
cloud hosts, analytics, crash reporting, payment processors, SDK and API providers, app stores and \
platforms, unless the question is specifically about them. Fewer, credible voices beat many odd ones. \
Reply with JSON only.";

#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub label: String,
    pub brief: String,
    pub entity_id: Option<String>,
    pub activity_band: (f64, f64),
    /// the planned camp (public slots only)
    pub camp: Option<String>,
    /// the slice of the user's material the slot is written from (grounded public slots only)
    pub grounding: Option<Grounding>,
}

pub const SYSTEM_PROMPT: &str = "\
You are the casting director for a social-media simulation that forecasts how the public reacts to a \
real-world question. You write believable, specific people and organisational accounts, each with a \
private inner life that will drive how they post, reply, like and argue.

Realism rules:
- Real publics are mostly not experts and mostly not extreme. Respect each slot's assigned stance and \
engagement exactly; do not drift everyone toward strong opinions.
- Make people concrete and different from one another: age, occupation, place, income, family situation, \
values, media habits, writing style. Avoid stereotypes and caricatures; give critics and supporters \
equally intelligent reasons.
- `background` (private, 2-4 sentences): who they are and the personal stakes, interests or experiences \
that explain their stance.
- `stance` (private, 1-2 sentences): their starting attitude toward the question's subject, why, and what \
might change their mind.
- `bio` (public, at most 160 characters): what their profile would actually say, in their own voice.
- `traits`: 2-5 short behavioural descriptors (e.g. \"sarcastic\", \"data-driven\", \"joins pile-ons\").
- `activity` (0-1): how often they act in a given round; stay inside the slot's band.
- `influence` (0-1): reach of the account. Major organisations and public figures 0.6-0.95; ordinary \
people usually 0.05-0.35; local micro-influencers up to 0.5.
- Entity slots speak as that organisation's official account or that public figure, consistent with the \
documented facts; do not invent scandals or positions the material does not support.
- `handle`: lowercase ASCII letters, digits and underscores, at most 20 characters, plausible for the \
person, and different from every handle already taken.
Reply with JSON only.";

pub async fn generate_personas(
    llm: &dyn Llm,
    graph: &KnowledgeGraph,
    question: &str,
    n: usize,
    model: &str,
    language: &str,
) -> Result<Vec<Persona>, LlmError> {
    generate_cast(llm, graph, question, n, model, language, false).await
}

/// [`generate_personas`], and with `fair` the cast of a `fair_turns` run: vocal stances never get a lurker's
/// activity, and every persona records its camp and source.
pub async fn generate_cast(
    llm: &dyn Llm,
    graph: &KnowledgeGraph,
    question: &str,
    n: usize,
    model: &str,
    language: &str,
    fair: bool,
) -> Result<Vec<Persona>, LlmError> {
    if n == 0 {
        return Ok(Vec::new());
    }
    let voices = select_voices(llm, graph, question, n, model).await;
    let slots = plan_cast(graph, question, n, voices.as_deref(), fair);
    fill_cast(llm, graph, &slots, question, model, language, fair).await
}

/// The user's audience material, read and clustered ([`crate::material`]), for [`generate_grounded_cast`].
#[derive(Debug, Clone, Copy)]
pub struct Ground<'a> {
    pub items: &'a [MaterialItem],
    pub clusters: &'a [Cluster],
}

/// [`generate_cast`] with the public built from the user's material: the camps follow the material's spread, and
/// every member of the public is written from real quotes of one cluster, records it as its `grounding` and has
/// source "material". Entity voices still come from the graph.
#[allow(clippy::too_many_arguments)]
pub async fn generate_grounded_cast(
    llm: &dyn Llm,
    graph: &KnowledgeGraph,
    question: &str,
    n: usize,
    model: &str,
    language: &str,
    fair: bool,
    ground: Ground<'_>,
) -> Result<Vec<Persona>, LlmError> {
    if n == 0 {
        return Ok(Vec::new());
    }
    let voices = select_voices(llm, graph, question, n, model).await;
    let slots = plan_grounded_cast(graph, question, n, voices.as_deref(), fair, ground);
    fill_cast(llm, graph, &slots, question, model, language, fair).await
}

/// Writes the planned slots in batches, retrying the ones a batch left out.
async fn fill_cast(
    llm: &dyn Llm,
    graph: &KnowledgeGraph,
    slots: &[Slot],
    question: &str,
    model: &str,
    language: &str,
    fair: bool,
) -> Result<Vec<Persona>, LlmError> {
    let digest = graph_digest(graph);
    let mut filled: HashMap<String, Persona> = HashMap::new();
    let mut filled_order: Vec<String> = Vec::new();
    let mut taken: HashSet<String> = HashSet::new();
    let mut pending: Vec<Slot> = slots.to_vec();
    let mut last_error = String::new();
    // every batch may be retried once, plus one spare
    let mut calls_left = 2 * slots.len().div_ceil(BATCH_SIZE) + 1;
    while !pending.is_empty() && calls_left > 0 {
        calls_left -= 1;
        let batch: Vec<Slot> = pending.iter().take(BATCH_SIZE).cloned().collect();
        let existing: Vec<&Persona> = filled_order.iter().map(|label| &filled[label]).collect();
        let prompt = batch_prompt(&batch, &digest, question, language, &existing);
        let result = match json::<PersonaBatch>(llm, &prompt, SYSTEM_PROMPT, model, "personas.batch").await {
            Ok(result) => result,
            Err(e) => {
                last_error = format!(": {e}");
                continue;
            }
        };
        for (slot, draft) in match_drafts(&batch, result.personas) {
            let mut persona = to_persona(slot, &draft, &mut taken);
            if slot.grounding.is_some() {
                persona.camp = slot.camp.clone();
                persona.source = Some(SOURCE_MATERIAL.into());
                persona.grounding = slot.grounding.clone();
            } else if fair {
                persona.camp = slot.camp.clone();
                persona.source = Some(SOURCE_CAST.into());
            }
            if filled.insert(slot.label.clone(), persona).is_none() {
                filled_order.push(slot.label.clone());
            }
        }
        pending.retain(|s| !filled.contains_key(&s.label));
    }
    if filled.is_empty() {
        return Err(LlmError(format!("persona generation produced no personas{last_error}")));
    }
    Ok(renumber(slots.iter().filter_map(|s| filled.remove(&s.label)).collect()))
}

/// Writes `count` people for a group the user described in words ("three parents of Year 7 children who take the
/// bus"), for adding to the cast of the run `config` describes, with its planner model and language. `existing` is
/// the cast so far, so handles and people do not repeat.
///
/// Ids are placeholders ("g1"..): [`tidy_cast`] numbers the whole cast once the group is added.
pub async fn generate_group(
    llm: &dyn Llm,
    graph: &KnowledgeGraph,
    config: &SimConfig,
    description: &str,
    count: usize,
    existing: &[Persona],
) -> Result<Vec<Persona>, LlmError> {
    let (question, model, language) =
        (config.question.as_str(), config.planner_model.as_str(), config.language.as_str());
    let description = py_strip(description);
    if description.is_empty() {
        return Err(LlmError("describe the group to add".into()));
    }
    let count = count.clamp(1, MAX_GROUP);
    let slots: Vec<Slot> = (0..count)
        .map(|i| Slot {
            label: format!("g{}", i + 1),
            brief: format!(
                "one member of the group the user asked for ({description}); a distinct individual with their own \
age, situation and voice"
            ),
            entity_id: None,
            activity_band: (0.05, 0.95),
            camp: None,
            grounding: None,
        })
        .collect();
    let digest = graph_digest(graph);
    let mut taken: HashSet<String> = existing.iter().map(|p| p.handle.clone()).collect();
    let mut made: Vec<Persona> = Vec::new();
    let mut pending: Vec<Slot> = slots.clone();
    let mut last_error = String::new();
    let mut calls_left = 2 * slots.len().div_ceil(BATCH_SIZE) + 1;
    while !pending.is_empty() && calls_left > 0 {
        calls_left -= 1;
        let batch: Vec<Slot> = pending.iter().take(BATCH_SIZE).cloned().collect();
        let mut shown: Vec<&Persona> = existing.iter().collect();
        shown.extend(made.iter());
        let prompt = group_prompt(&batch, &digest, question, description, language, &shown);
        let result = match json::<GroupBatch>(llm, &prompt, SYSTEM_PROMPT, model, "personas.group").await {
            Ok(result) => result,
            Err(e) => {
                last_error = format!(": {e}");
                continue;
            }
        };
        let camps: HashMap<String, String> =
            result.personas.iter().map(|d| (py_strip(&d.slot).to_string(), d.camp.clone())).collect();
        let drafts: Vec<PersonaDraft> = result
            .personas
            .into_iter()
            .map(|d| PersonaDraft {
                slot: d.slot,
                handle: d.handle,
                name: d.name,
                bio: d.bio,
                background: d.background,
                stance: d.stance,
                traits: d.traits,
                activity: d.activity,
                influence: d.influence,
            })
            .collect();
        let mut filled: HashSet<String> = HashSet::new();
        for (slot, draft) in match_drafts(&batch, drafts) {
            let mut persona = to_persona(slot, &draft, &mut taken);
            let camp = camps.get(py_strip(&draft.slot)).map(|c| py_strip(c).to_lowercase());
            persona.camp = camp.filter(|c| CAMPS.contains(&c.as_str()));
            persona.source = Some(SOURCE_GROUP.into());
            filled.insert(slot.label.clone());
            made.push(persona);
        }
        pending.retain(|s| !filled.contains(&s.label));
    }
    if made.is_empty() {
        return Err(LlmError(format!("could not write the group{last_error}")));
    }
    Ok(made)
}

fn group_prompt(
    batch: &[Slot],
    digest: &str,
    question: &str,
    description: &str,
    language: &str,
    existing: &[&Persona],
) -> String {
    let body = batch_prompt_body(batch, digest, question, language, existing);
    format!(
        "{body}The user described this group in their own words: \"{description}\". Follow the description \
exactly (who they are, how many, what they do); where it says nothing about their views, let them differ as a real \
group would. `camp` is each person's starting leaning toward the question's subject, one of: {}.\n\n\
Return a JSON object:\n\
{{\"personas\": [{{\"slot\": slot id, \"handle\": str, \"name\": str, \"bio\": str, \"background\": str,\n  \
\"stance\": str, \"traits\": [str], \"activity\": float, \"influence\": float, \"camp\": str}}]}}",
        CAMPS.join(", ")
    )
}

/// Cleans a cast a client sent (edited, extended or loaded from a saved audience): text trimmed, at most
/// [`MAX_CAST`] people, unique valid handles, activity and influence in 0..1, known camps and sources only, and ids
/// renumbered a01..aNN in order. Callers refuse a cast over [`MAX_CAST`] before they get here.
pub fn tidy_cast(personas: Vec<Persona>) -> Vec<Persona> {
    let mut taken: HashSet<String> = HashSet::new();
    let tidy: Vec<Persona> = personas
        .into_iter()
        .take(MAX_CAST)
        .map(|mut p| {
            let grounding = p.grounding.take().and_then(tidy_grounding);
            let handle = unique_handle(&p.handle, &p.name, &taken);
            taken.insert(handle.clone());
            let name = py_strip(&p.name);
            let unit = |x: f64, default: f64| if x.is_finite() { py_round(clamp(x, 0.0, 1.0), 3) } else { default };
            let keep = |v: Option<String>| v.map(|s| py_strip(&s).to_string()).filter(|s| !s.is_empty());
            Persona {
                name: if name.is_empty() { handle.clone() } else { name.to_string() },
                handle,
                bio: py_strip(&p.bio).into(),
                background: py_strip(&p.background).into(),
                stance: py_strip(&p.stance).into(),
                traits: p
                    .traits
                    .iter()
                    .map(|t| py_strip(t))
                    .filter(|t| !t.is_empty())
                    .take(MAX_TRAITS)
                    .map(String::from)
                    .collect(),
                activity: unit(p.activity, 0.6),
                influence: unit(p.influence, 0.5),
                entity_id: keep(p.entity_id),
                camp: keep(p.camp).filter(|c| CAMPS.contains(&c.as_str())),
                source: keep(p.source).filter(|s| SOURCES.contains(&s.as_str())),
                grounding,
                ..p
            }
        })
        .collect();
    renumber(tidy)
}

/// A client-sent grounding with known kinds, a short theme and at most a handful of quotes; None when nothing is left.
fn tidy_grounding(g: Grounding) -> Option<Grounding> {
    let counts: indexmap::IndexMap<String, i64> = MATERIAL_KINDS
        .iter()
        .filter_map(|k| g.counts.get(*k).filter(|n| **n > 0).map(|n| (k.to_string(), *n)))
        .collect();
    let quotes: Vec<String> = g
        .quotes
        .iter()
        .map(|q| py_strip(q))
        .filter(|q| !q.is_empty())
        .take(2 * QUOTES_PER_PERSONA)
        .map(|q| q.chars().take(crate::material::MAX_ITEM_CHARS + 1).collect())
        .collect();
    let theme: String = py_strip(&g.theme).chars().take(120).collect();
    (!counts.is_empty() || !quotes.is_empty()).then_some(Grounding { theme, counts, quotes })
}

/// Asks the planner which entities would really speak up about the question.
///
/// None when the call fails, so [`plan_slots`] falls back to ranking by graph prominence.
pub async fn select_voices(
    llm: &dyn Llm,
    graph: &KnowledgeGraph,
    question: &str,
    n: usize,
    model: &str,
) -> Option<Vec<Entity>> {
    let candidates = embodiable(graph);
    let limit = entity_limit(n);
    if candidates.is_empty() || limit == 0 {
        return Some(Vec::new());
    }
    let listing = candidates
        .iter()
        .take(40)
        .map(|e| format!("- {} [{}]: {}: {}", e.id, e.kind, e.name, py_prefix(&e.summary, 160)))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!(
        "Question: {question}\n\nCandidate entities (id [kind]: name: summary):\n{listing}\n\n\
Return up to {limit} entity_ids from the list, most relevant first. Return fewer, or none, \
if few would genuinely post about this."
    );
    let pick = json::<VoicePick>(llm, &prompt, VOICES_SYSTEM, model, "personas.cast").await.ok()?;
    let mut chosen: Vec<Entity> = Vec::new();
    for entity_id in &pick.entity_ids {
        if let Some(entity) = candidates.iter().rev().find(|e| &e.id == entity_id) {
            if !chosen.contains(entity) {
                chosen.push(entity.clone());
            }
        }
    }
    chosen.truncate(limit);
    Some(chosen)
}

fn entity_limit(n: usize) -> usize {
    if n >= 2 {
        py_round_int(n as f64 * ENTITY_SHARE).max(1) as usize
    } else {
        0
    }
}

/// Decides who the cast is before asking the LLM: which entities get a voice, and the public's mix.
///
/// `voices` is the planner's pick from [`select_voices`]; None ranks by graph prominence instead.
pub fn plan_slots(graph: &KnowledgeGraph, question: &str, n: usize, voices: Option<&[Entity]>) -> Vec<Slot> {
    plan_cast(graph, question, n, voices, false)
}

/// [`plan_slots`]; with `fair`, a strongly held stance that drew a lurker's engagement swaps it with the first
/// moderate or neutral slot that drew more, so the people most likely to speak up are not planned to stay silent.
/// The random draws are the same either way.
pub fn plan_cast(graph: &KnowledgeGraph, question: &str, n: usize, voices: Option<&[Entity]>, fair: bool) -> Vec<Slot> {
    plan(graph, question, n, voices, fair, None)
}

/// [`plan_cast`] with the public drawn from the user's material: stances apportioned by how many items each camp
/// holds, and each public slot given one cluster of its camp (clusters apportioned by size) and up to
/// [`QUOTES_PER_PERSONA`] of its items, so people of one cluster are written from different quotes while it has
/// enough. Without any read item it is [`plan_cast`].
pub fn plan_grounded_cast(
    graph: &KnowledgeGraph,
    question: &str,
    n: usize,
    voices: Option<&[Entity]>,
    fair: bool,
    ground: Ground<'_>,
) -> Vec<Slot> {
    plan(graph, question, n, voices, fair, Some(ground))
}

/// Each camp's stance weighted by its items, camps without any left out; None when no item was read.
fn camp_mix(ground: &Ground<'_>) -> Option<Vec<(&'static str, f64)>> {
    let mix: Vec<(&'static str, f64)> = CAMPS
        .iter()
        .zip(STANCE_MIX.iter())
        .map(|(camp, (stance, _))| {
            let items: usize = ground.clusters.iter().filter(|c| c.camp == *camp).map(|c| c.items.len()).sum();
            (*stance, items as f64)
        })
        .filter(|(_, w)| *w > 0.0)
        .collect();
    (!mix.is_empty()).then_some(mix)
}

/// Largest-remainder shares of `count` over `weights`, ties to the earlier weight.
fn apportion(weights: &[usize], count: usize) -> Vec<usize> {
    let total: usize = weights.iter().sum();
    if total == 0 {
        return vec![0; weights.len()];
    }
    let mut shares: Vec<usize> = weights.iter().map(|w| w * count / total).collect();
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by_key(|&i| (std::cmp::Reverse(weights[i] * count % total), i));
    let short = count - shares.iter().sum::<usize>();
    for &i in order.iter().take(short) {
        shares[i] += 1;
    }
    shares
}

/// For every public slot, the cluster it is drawn from and its quotes (item indexes).
fn assign_clusters(
    ground: &Ground<'_>,
    camps: &[Option<&str>],
    rng: &mut PyRandom,
) -> Vec<Option<(usize, Vec<usize>)>> {
    let mut out: Vec<Option<(usize, Vec<usize>)>> = vec![None; camps.len()];
    let mut decks: Vec<(Vec<usize>, usize)> = ground
        .clusters
        .iter()
        .map(|c| {
            let mut deck = c.items.clone();
            rng.shuffle(&mut deck);
            (deck, 0)
        })
        .collect();
    for camp in CAMPS {
        let slots: Vec<usize> = (0..camps.len()).filter(|&j| camps[j] == Some(camp)).collect();
        let clusters: Vec<usize> = (0..ground.clusters.len()).filter(|&c| ground.clusters[c].camp == camp).collect();
        if slots.is_empty() || clusters.is_empty() {
            continue;
        }
        let sizes: Vec<usize> = clusters.iter().map(|&c| ground.clusters[c].items.len()).collect();
        let shares = apportion(&sizes, slots.len());
        let mut next = slots.into_iter();
        for (&c, &share) in clusters.iter().zip(&shares) {
            for j in next.by_ref().take(share) {
                let (deck, cursor) = &mut decks[c];
                let take = QUOTES_PER_PERSONA.min(deck.len());
                let quotes: Vec<usize> = (0..take).map(|k| deck[(*cursor + k) % deck.len()]).collect();
                *cursor = (*cursor + take) % deck.len().max(1);
                out[j] = Some((c, quotes));
            }
        }
    }
    out
}

fn plan(
    graph: &KnowledgeGraph,
    question: &str,
    n: usize,
    voices: Option<&[Entity]>,
    fair: bool,
    ground: Option<Ground<'_>>,
) -> Vec<Slot> {
    let candidates: Vec<Entity> = match voices {
        Some(v) => v.to_vec(),
        None => embodiable(graph),
    };
    let n_entity = candidates.len().min(entity_limit(n));
    let mut slots: Vec<Slot> = candidates[..n_entity]
        .iter()
        .enumerate()
        .map(|(i, e)| Slot {
            label: agent_id(i, n),
            entity_id: Some(e.id.clone()),
            brief: format!(
                "ENTITY VOICE for {} ({}, id {}): {}",
                e.name,
                e.kind,
                e.id,
                if e.summary.is_empty() { "see material" } else { &e.summary }
            ),
            activity_band: (0.0, 1.0),
            camp: None,
            grounding: None,
        })
        .collect();
    let n_public = n - n_entity;
    let mut rng = PyRandom::from_str_seed(&format!("{question}|{n}"));
    let mix = ground.as_ref().and_then(camp_mix);
    let ground = ground.filter(|_| mix.is_some());
    let stances = allocate(mix.as_deref().unwrap_or(&STANCE_MIX), n_public, &mut rng);
    let mut engagement = allocate(&ENGAGEMENT_MIX, n_public, &mut rng);
    if fair {
        let strong = |j: usize| camp_of(stances[j]).is_some_and(is_strong);
        let lurker = ENGAGEMENT_MIX[0].0;
        for j in 0..n_public {
            if strong(j) && engagement[j] == lurker {
                if let Some(k) = (0..n_public).find(|&k| !strong(k) && engagement[k] != lurker) {
                    engagement.swap(j, k);
                }
            }
        }
    }
    let mut ages: Vec<&str> = (0..n_public).map(|i| AGE_BANDS[i % AGE_BANDS.len()]).collect();
    rng.shuffle(&mut ages);
    let groups: Vec<&str> = graph.entities.iter().filter(|e| e.kind == "group").map(|e| e.name.as_str()).collect();
    let picks = ground.map(|g| {
        let camps: Vec<Option<&str>> = stances.iter().map(|s| camp_of(s)).collect();
        assign_clusters(&g, &camps, &mut rng)
    });
    for j in 0..n_public {
        let (eng_label, band) = engagement[j];
        if let (Some(g), Some(Some((c, quotes)))) = (ground, picks.as_ref().map(|p| &p[j])) {
            let cluster = &g.clusters[*c];
            let texts: Vec<String> = quotes.iter().map(|&i| g.items[i].text.clone()).collect();
            let shown = texts
                .iter()
                .map(|t| format!("\"{}\"", py_prefix(t, BRIEF_QUOTE_CHARS)))
                .collect::<Vec<_>>()
                .join(" / ");
            let brief = format!(
                "a member of the real audience the user's material comes from; age {}; starting stance: {}; \
engagement: {eng_label} (activity {:.2}-{:.2}); what drives them: {}; write their background and stance from these \
real comments by people like them, keeping their concerns and plain wording (square brackets hide personal \
details: invent no names or contact details for them): {shown}",
                ages[j], stances[j], band.0, band.1, cluster.theme
            );
            let counts = count_kinds(cluster.items.iter().map(|&i| g.items[i].kind.as_str()));
            slots.push(Slot {
                label: agent_id(n_entity + j, n),
                brief,
                entity_id: None,
                activity_band: band,
                camp: Some(cluster.camp.clone()),
                grounding: Some(Grounding { theme: cluster.theme.clone(), counts, quotes: texts }),
            });
            continue;
        }
        let segment = if !groups.is_empty() && j % 2 == 0 {
            groups[j % groups.len()]
        } else {
            "any affected segment not yet covered"
        };
        let brief = format!(
            "ordinary member of the public; age {}; part of: {segment}; starting stance: {}; engagement: {eng_label} \
(activity {:.2}-{:.2})",
            ages[j], stances[j], band.0, band.1
        );
        slots.push(Slot {
            label: agent_id(n_entity + j, n),
            brief,
            entity_id: None,
            activity_band: band,
            camp: camp_of(stances[j]).map(String::from),
            grounding: None,
        });
    }
    slots
}

pub fn graph_digest(graph: &KnowledgeGraph) -> String {
    let names: HashMap<&str, &str> = graph.entities.iter().map(|e| (e.id.as_str(), e.name.as_str())).collect();
    let lines: Vec<String> = graph
        .entities
        .iter()
        .take(DIGEST_ENTITIES)
        .map(|e| format!("- {} [{}]: {}", e.name, e.kind, e.summary))
        .collect();
    let rels: Vec<String> = graph
        .relations
        .iter()
        .filter_map(|r| match (names.get(r.source.as_str()), names.get(r.target.as_str())) {
            (Some(s), Some(t)) => Some(format!("- {s} {} {t}", r.label)),
            _ => None,
        })
        .take(DIGEST_RELATIONS)
        .collect();
    let mut parts = vec!["Key entities:".to_string()];
    if lines.is_empty() {
        parts.push("- (none extracted)".into());
    } else {
        parts.extend(lines);
    }
    if !rels.is_empty() {
        parts.push("Relations:".into());
        parts.extend(rels);
    }
    parts.join("\n")
}

/// Organizations and people, most-connected and most-mentioned first.
fn embodiable(graph: &KnowledgeGraph) -> Vec<Entity> {
    let mut degree: HashMap<&str, usize> = HashMap::new();
    for r in &graph.relations {
        *degree.entry(&r.source).or_default() += 1;
        *degree.entry(&r.target).or_default() += 1;
    }
    let mut picks: Vec<Entity> =
        graph.entities.iter().filter(|e| EMBODIED_KINDS.contains(&e.kind.as_str())).cloned().collect();
    picks
        .sort_by_key(|e| std::cmp::Reverse(degree.get(e.id.as_str()).copied().unwrap_or(0) + e.source_chunk_ids.len()));
    picks
}

/// Largest-remainder apportionment of `count` items over weighted categories, then shuffled.
fn allocate<V: Copy>(mix: &[(V, f64)], count: usize, rng: &mut PyRandom) -> Vec<V> {
    let total = py_sum(mix.iter().map(|(_, w)| *w));
    let quotas: Vec<f64> = mix.iter().map(|(_, w)| count as f64 * w / total).collect();
    let mut counts: Vec<usize> = quotas.iter().map(|q| q.trunc() as usize).collect();
    let mut by_remainder: Vec<usize> = (0..mix.len()).collect();
    // sorted(..., reverse=True) is stable: equal remainders keep their order
    by_remainder.sort_by(|&a, &b| {
        let (ra, rb) = (quotas[a] - counts[a] as f64, quotas[b] - counts[b] as f64);
        rb.partial_cmp(&ra).unwrap_or(std::cmp::Ordering::Equal)
    });
    let short = count.saturating_sub(counts.iter().sum());
    for &i in by_remainder.iter().take(short) {
        counts[i] += 1;
    }
    let mut items: Vec<V> = mix.iter().zip(&counts).flat_map(|((v, _), &c)| std::iter::repeat_n(*v, c)).collect();
    rng.shuffle(&mut items);
    items
}

fn batch_prompt(batch: &[Slot], digest: &str, question: &str, language: &str, existing: &[&Persona]) -> String {
    format!(
        "{}Return a JSON object:\n\
{{\"personas\": [{{\"slot\": slot id, \"handle\": str, \"name\": str, \"bio\": str, \"background\": str,\n  \
\"stance\": str, \"traits\": [str], \"activity\": float, \"influence\": float}}]}}",
        batch_prompt_body(batch, digest, question, language, existing)
    )
}

/// Everything in a persona batch prompt before its reply format, which [`group_prompt`] replaces.
fn batch_prompt_body(batch: &[Slot], digest: &str, question: &str, language: &str, existing: &[&Persona]) -> String {
    let slot_lines = batch.iter().map(|s| format!("- slot {}: {}", s.label, s.brief)).collect::<Vec<_>>().join("\n");
    let taken = if existing.is_empty() {
        "- (none yet)".to_string()
    } else {
        existing
            .iter()
            .map(|p| format!("- @{} ({}): {}", p.handle, p.name, py_prefix(&p.bio, 70)))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "Question being simulated:\n{question}\n\n\
Background material (from the seed documents):\n{digest}\n\n\
Accounts already created (do not reuse these handles or near-duplicate these people):\n{taken}\n\n\
Write exactly one persona for each slot below, in the same order, honouring every slot's brief:\n{slot_lines}\n\n\
Write `name`, `bio`, `background`, `stance` and `traits` in {language}; pick names that fit the setting \
of the material. Handles stay lowercase ASCII.\n\n"
    )
}

/// Pairs drafts with slots by echoed slot id, then hands leftovers to unclaimed slots in order.
fn match_drafts(batch: &[Slot], drafts: Vec<PersonaDraft>) -> Vec<(&Slot, PersonaDraft)> {
    let labels: HashSet<&str> = batch.iter().map(|s| s.label.as_str()).collect();
    let mut pairs: HashMap<String, PersonaDraft> = HashMap::new();
    let mut leftovers: std::collections::VecDeque<PersonaDraft> = Default::default();
    for draft in drafts {
        let label = py_strip(&draft.slot).to_string();
        if labels.contains(label.as_str()) && !pairs.contains_key(&label) {
            pairs.insert(label, draft);
        } else {
            leftovers.push_back(draft);
        }
    }
    for slot in batch {
        if !pairs.contains_key(&slot.label) {
            if let Some(draft) = leftovers.pop_front() {
                pairs.insert(slot.label.clone(), draft);
            }
        }
    }
    batch.iter().filter_map(|s| pairs.remove(&s.label).map(|d| (s, d))).collect()
}

fn to_persona(slot: &Slot, draft: &PersonaDraft, taken: &mut HashSet<String>) -> Persona {
    let handle = unique_handle(&draft.handle, &draft.name, taken);
    taken.insert(handle.clone());
    let (low, high) = slot.activity_band;
    let traits: Vec<String> =
        draft.traits.iter().map(|t| py_strip(t)).filter(|t| !t.is_empty()).take(MAX_TRAITS).map(String::from).collect();
    let name = py_strip(&draft.name);
    Persona {
        id: slot.label.clone(),
        name: if name.is_empty() { handle.clone() } else { name.to_string() },
        handle,
        bio: py_strip(&draft.bio).into(),
        background: py_strip(&draft.background).into(),
        stance: py_strip(&draft.stance).into(),
        traits,
        activity: py_round(clamp(draft.activity, low, high), 3),
        influence: py_round(clamp(draft.influence, 0.0, 1.0), 3),
        entity_id: slot.entity_id.clone(),
        camp: None,
        source: None,
        grounding: None,
    }
}

/// Sanitizes to `[a-z0-9_]{1,20}`; derives from the name if needed; suffixes a number on collisions.
pub fn unique_handle(raw: &str, name: &str, taken: &HashSet<String>) -> String {
    let base =
        [handle_base(raw), handle_base(name)].into_iter().find(|b| !b.is_empty()).unwrap_or_else(|| "user".into());
    let (mut candidate, mut n) = (base.clone(), 2);
    while taken.contains(&candidate) {
        let suffix = format!("_{n}");
        let keep = HANDLE_MAX.saturating_sub(suffix.len()).min(base.len());
        candidate = format!("{}{suffix}", &base[..keep]);
        n += 1;
    }
    candidate
}

fn handle_base(text: &str) -> String {
    let ascii: String = text.trim_start_matches('@').nfkd().filter(char::is_ascii).collect::<String>().to_lowercase();
    // re.sub(r"[^a-z0-9_]+", "_") then re.sub(r"_+", "_"): every run of non-word characters and underscores is one "_"
    let mut out = String::with_capacity(ascii.len());
    for c in ascii.chars() {
        let c = if c.is_ascii_lowercase() || c.is_ascii_digit() { c } else { '_' };
        if !(c == '_' && out.ends_with('_')) {
            out.push(c);
        }
    }
    let trimmed = out.trim_matches('_');
    trimmed[..trimmed.len().min(HANDLE_MAX)].trim_end_matches('_').to_string()
}

fn clamp(value: f64, low: f64, high: f64) -> f64 {
    if value.is_nan() {
        return low;
    }
    // min(high, max(low, value)), keeping Python's choice of operand on ties
    let v = if value > low { value } else { low };
    if v < high {
        v
    } else {
        high
    }
}

/// Zero-based index to "a01".. (wider padding once there are 100+ agents).
pub fn agent_id(index: usize, total: usize) -> String {
    let width = total.to_string().len().max(2);
    format!("a{:0width$}", index + 1)
}

/// Guarantees contiguous ids a01..aNN even if some slots could not be filled.
fn renumber(personas: Vec<Persona>) -> Vec<Persona> {
    let n = personas.len();
    personas.into_iter().enumerate().map(|(i, p)| Persona { id: agent_id(i, n), ..p }).collect()
}
