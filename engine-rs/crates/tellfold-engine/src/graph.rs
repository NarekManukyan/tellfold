//! Knowledge-graph extraction (port of `tellfold/graph.py`): concurrent LLM passes over chunk groups, then a
//! deterministic merge.

use std::collections::{HashMap, HashSet};

use futures::stream::{FuturesUnordered, StreamExt};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use tellfold_llm::{json, Llm, LlmError};
use tellfold_models::json::py_sum;
use tellfold_models::lax;
use tellfold_models::text::{py_split, py_strip};
use tellfold_models::{Chunk, Entity, KnowledgeGraph, Relation};
use unicode_normalization::UnicodeNormalization;

use crate::pytext::{char_len, is_combining, is_word, squash, words};

pub const CHUNKS_PER_CALL: usize = 4;
/// A folder of documents can hold hundreds of chunks; mapping them all took over a hundred slow calls before the
/// simulation even started. Past this many calls, only the chunks that matter most to the question are read.
pub const MAX_EXTRACT_CALLS: usize = 16;
pub const MAX_EXTRACT_CHUNKS: usize = MAX_EXTRACT_CALLS * CHUNKS_PER_CALL;
pub const SUMMARY_CAP: usize = 480;
pub const EVIDENCE_CAP: usize = 220;
pub const KINDS: [&str; 7] = ["person", "organization", "group", "product", "place", "concept", "event"];

fn kind_synonym(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "company" | "org" | "institution" | "agency" | "media" | "outlet" | "government" | "party" | "union" => {
            "organization"
        }
        "individual" => "person",
        "people" | "community" | "demographic" => "group",
        "location" | "country" | "city" | "region" => "place",
        "policy" | "idea" | "law" => "concept",
        "technology" | "service" | "brand" => "product",
        "incident" => "event",
        _ => return None,
    })
}

const LEGAL_SUFFIXES: [&str; 15] = [
    "inc",
    "incorporated",
    "corp",
    "corporation",
    "co",
    "company",
    "ltd",
    "limited",
    "llc",
    "plc",
    "gmbh",
    "ag",
    "sa",
    "nv",
    "bv",
];
const ARTICLES: [&str; 3] = ["the", "a", "an"];

// ---------- LLM output schema (names, not ids: ids are assigned during the merge) ----------

fn d_concept() -> String {
    "concept".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractedEntity {
    pub name: String,
    #[serde(default = "d_concept")]
    pub kind: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default, deserialize_with = "lax::int_list")]
    pub chunk_ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractedRelation {
    pub source: String,
    pub target: String,
    pub label: String,
    #[serde(default)]
    pub evidence: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Extraction {
    #[serde(default)]
    pub entities: Vec<ExtractedEntity>,
    #[serde(default)]
    pub relations: Vec<ExtractedRelation>,
}

tellfold_llm::json_model!(Extraction, "../../../schemas/Extraction.json");

pub const SYSTEM_PROMPT: &str = "\
You are an analyst preparing the cast for a social-reaction simulation. You read source documents and \
map the actors who would plausibly respond in public to a specific question: people, organizations, \
interest groups, communities, media outlets, regulators, competitors, customers. You also capture the \
products, places, events and concepts those actors argue about, but actors come first.

Rules:
- Only extract what the text supports. Never invent people, numbers or quotes.
- Use the most complete proper name as `name` (e.g. \"European Commission\", not \"the Commission\"); put \
short forms, acronyms and nicknames that the text uses in `aliases`.
- `kind` is exactly one of: person, organization, group, product, place, concept, event. Media outlets \
and government bodies are organizations; loose collectives (e.g. \"small landlords\", \"students\") are groups.
- `summary`: one or two factual sentences on who/what it is and, where the text shows it, its interest \
or likely position regarding the question.
- `chunk_ids`: the ids of the chunks that mention it.
- Relations connect two extracted entities by their `name`, with a short present-tense verb phrase as \
`label` (\"regulates\", \"competes with\", \"funds\", \"criticizes\") and a brief paraphrase of the supporting \
text as `evidence`.
- Skip generic nouns that are not identifiable actors or topics (\"people\", \"the market\", \"experts\").
- Write summaries and evidence in the language of the source text. Reply with JSON only.";

fn lower_words(text: &str) -> HashSet<String> {
    words(text, 3).into_iter().map(str::to_lowercase).collect()
}

/// All chunks when they fit. Otherwise the opening chunk of each document (up to half the budget), then the
/// chunks that share the most words with the question, rarer words counting more. Kept in document order.
pub fn select_chunks(chunks: &[Chunk], question: &str, limit: usize) -> Vec<Chunk> {
    if chunks.len() <= limit {
        return chunks.to_vec();
    }
    let words: Vec<HashSet<String>> = chunks.iter().map(|c| lower_words(&c.text)).collect();
    let mut seen_in: HashMap<&str, usize> = HashMap::new();
    for ws in &words {
        for w in ws {
            *seen_in.entry(w.as_str()).or_default() += 1;
        }
    }
    let asked = lower_words(question);
    let total = chunks.len();
    let relevance = |i: usize| -> f64 {
        // Python sums over a set, in hash order; sorting keeps the (compensated) sum reproducible
        let mut shared: Vec<&String> = asked.intersection(&words[i]).collect();
        shared.sort();
        py_sum(shared.into_iter().map(|w| (total as f64 / seen_in[w.as_str()] as f64).ln()))
    };

    let mut picked: HashSet<usize> = HashSet::new();
    let mut sources: HashSet<&str> = HashSet::new();
    for (i, chunk) in chunks.iter().enumerate() {
        if !sources.contains(chunk.source.as_str()) && picked.len() < limit / 2 {
            sources.insert(&chunk.source);
            picked.insert(i);
        }
    }
    let scores: Vec<f64> = (0..total).map(relevance).collect();
    let mut order: Vec<usize> = (0..total).collect();
    order.sort_by(|&a, &b| (-scores[a]).partial_cmp(&-scores[b]).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b)));
    for i in order {
        if picked.len() >= limit {
            break;
        }
        picked.insert(i);
    }
    let mut kept: Vec<usize> = picked.into_iter().collect();
    kept.sort_unstable();
    kept.into_iter().map(|i| chunks[i].clone()).collect()
}

pub type Progress<'a> = &'a dyn Fn(&str);

pub async fn extract_graph(
    llm: &dyn Llm,
    chunks: &[Chunk],
    question: &str,
    model: &str,
    on_progress: Option<Progress<'_>>,
) -> Result<KnowledgeGraph, LlmError> {
    let groups: Vec<&[Chunk]> = chunks.chunks(CHUNKS_PER_CALL).collect();
    let mut results: Vec<Option<Extraction>> = vec![None; groups.len()];
    let report = |message: &str| {
        if let Some(f) = on_progress {
            f(message);
        }
    };
    let mut errors: Vec<String> = Vec::new();
    let mut pending: FuturesUnordered<_> = groups
        .iter()
        .enumerate()
        .map(|(index, group)| async move {
            let prompt = extraction_prompt(group, question);
            (index, json::<Extraction>(llm, &prompt, SYSTEM_PROMPT, model, "graph.extract").await)
        })
        .collect();
    let mut done = 0;
    while let Some((index, result)) = pending.next().await {
        let group = groups[index];
        let outcome = match result {
            Ok(result) => {
                let count = result.entities.len();
                results[index] = Some(scope_chunk_ids(result, group));
                format!("{count} entities")
            }
            Err(e) => {
                errors.push(e.to_string());
                format!("failed: {e}")
            }
        };
        done += 1;
        report(&format!(
            "extract {done}/{}: chunks {}-{} {outcome}",
            groups.len(),
            group[0].id,
            group[group.len() - 1].id
        ));
    }
    let succeeded: Vec<Extraction> = results.into_iter().flatten().collect();
    if !groups.is_empty() && succeeded.is_empty() {
        let last = errors.last().map(|e| format!(": {e}")).unwrap_or_default();
        return Err(LlmError(format!("graph extraction failed for every chunk group{last}")));
    }
    let graph = merge_extractions(&succeeded);
    report(&format!("graph: {} entities, {} relations", graph.entities.len(), graph.relations.len()));
    Ok(graph)
}

pub fn extraction_prompt(group: &[Chunk], question: &str) -> String {
    let body = group
        .iter()
        .map(|c| format!("<chunk id=\"{}\" source=\"{}\">\n{}\n</chunk>", c.id, c.source, c.text))
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        "Question the simulation will try to answer:\n{question}\n\n\
Extract the entities and relations from the chunks below. Favour the actors whose reaction to the \
question matters, and the things they would react to. A typical chunk yields 3-12 entities.\n\n\
{body}\n\n\
Return a JSON object:\n\
{{\"entities\": [{{\"name\": str, \"kind\": str, \"summary\": str, \"aliases\": [str], \"chunk_ids\": [int]}}],\n  \
\"relations\": [{{\"source\": entity name, \"target\": entity name, \"label\": str, \"evidence\": str}}]}}"
    )
}

/// Keep only chunk ids from this group; fall back to the whole group when none are cited.
fn scope_chunk_ids(mut result: Extraction, group: &[Chunk]) -> Extraction {
    let allowed: Vec<i64> = group.iter().map(|c| c.id).collect();
    for entity in &mut result.entities {
        let cited: Vec<i64> = entity.chunk_ids.iter().copied().filter(|i| allowed.contains(i)).collect();
        entity.chunk_ids = if cited.is_empty() { allowed.clone() } else { cited };
    }
    result
}

// ---------- merge ----------

/// Lowercase, accent-free, dash-separated id; keeps non-latin letters so any language works.
pub fn slugify(name: &str) -> String {
    let text: String = name.nfkd().filter(|&c| !is_combining(c)).collect::<String>().to_lowercase();
    let text = text.replace('&', " and ").replace(['\'', '\u{2019}'], "");
    // re.sub(r"[\W_]+", "-", text).strip("-")
    let mut out = String::with_capacity(text.len());
    let mut in_gap = false;
    for c in text.chars() {
        if is_word(c) && c != '_' {
            out.push(c);
            in_gap = false;
        } else if !in_gap {
            out.push('-');
            in_gap = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Looser key used to detect duplicates: drops leading articles and legal-form suffixes.
pub fn match_key(name: &str) -> String {
    let slug = slugify(name);
    let mut words: Vec<&str> = slug.split('-').collect();
    while words.len() > 1 && ARTICLES.contains(&words[0]) {
        words.remove(0);
    }
    while words.len() > 1 && LEGAL_SUFFIXES.contains(words.last().expect("not empty")) {
        words.pop();
    }
    words.into_iter().filter(|w| !w.is_empty()).collect::<Vec<_>>().join("-")
}

pub fn normalize_kind(kind: &str) -> String {
    let kind = py_strip(kind).to_lowercase();
    if KINDS.contains(&kind.as_str()) {
        return kind;
    }
    kind_synonym(&kind).or_else(|| kind_synonym(kind.trim_end_matches('s'))).unwrap_or("concept").to_string()
}

/// Dedupe entities across extraction passes (by name or alias), then resolve and dedupe relations.
pub fn merge_extractions(parts: &[Extraction]) -> KnowledgeGraph {
    let mentions: Vec<&ExtractedEntity> =
        parts.iter().flat_map(|p| &p.entities).filter(|e| !match_key(&e.name).is_empty()).collect();
    let clusters = cluster(&mentions);
    let mut ranked: Vec<(usize, Entity)> = Vec::new();
    let mut key_to_id: HashMap<String, String> = HashMap::new();
    let mut used_ids: HashSet<String> = HashSet::new();
    for members in &clusters {
        let entity = merge_cluster(members, &used_ids);
        used_ids.insert(entity.id.clone());
        for m in members {
            for key in keys(m) {
                key_to_id.entry(key).or_insert_with(|| entity.id.clone());
            }
        }
        ranked.push((members.len(), entity));
    }
    // most-mentioned first, then widest coverage; stable sort keeps first-seen order for ties
    ranked.sort_by_key(|(n, e)| (std::cmp::Reverse(*n), std::cmp::Reverse(e.source_chunk_ids.len())));
    let entities = ranked.into_iter().map(|(_, e)| e).collect();
    let raw: Vec<&ExtractedRelation> = parts.iter().flat_map(|p| &p.relations).collect();
    KnowledgeGraph { entities, relations: merge_relations(&raw, &key_to_id) }
}

fn keys(entity: &ExtractedEntity) -> Vec<String> {
    let mut keys = vec![match_key(&entity.name)];
    keys.extend(entity.aliases.iter().map(|a| match_key(a)).filter(|k| char_len(k) >= 2));
    keys.retain(|k| !k.is_empty());
    keys
}

/// Union-find over shared name/alias keys; clusters ordered by first appearance.
fn cluster<'a>(mentions: &[&'a ExtractedEntity]) -> Vec<Vec<&'a ExtractedEntity>> {
    let mut parent: Vec<usize> = (0..mentions.len()).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut owner: HashMap<String, usize> = HashMap::new();
    for (i, mention) in mentions.iter().enumerate() {
        for key in keys(mention) {
            if let Some(&o) = owner.get(&key) {
                let (a, b) = (find(&mut parent, o), find(&mut parent, i));
                parent[a.max(b)] = a.min(b);
            } else {
                owner.insert(key, i);
            }
        }
    }
    let mut clusters: IndexMap<usize, Vec<&ExtractedEntity>> = IndexMap::new();
    for (i, mention) in mentions.iter().enumerate() {
        let root = find(&mut parent, i);
        clusters.entry(root).or_default().push(mention);
    }
    clusters.sort_keys();
    clusters.into_values().collect()
}

/// `Counter(values).most_common(1)[0][0]`: the most frequent value, the first seen on ties.
pub(crate) fn most_common<I: IntoIterator<Item = String>>(values: I) -> String {
    let mut counts: IndexMap<String, usize> = IndexMap::new();
    for v in values {
        *counts.entry(v).or_default() += 1;
    }
    let mut best: Option<(&String, usize)> = None;
    for (v, &n) in &counts {
        if best.is_none_or(|(_, b)| n > b) {
            best = Some((v, n));
        }
    }
    best.map(|(v, _)| v.clone()).unwrap_or_default()
}

fn merge_cluster(members: &[&ExtractedEntity], used_ids: &HashSet<String>) -> Entity {
    let name = most_common(members.iter().map(|m| py_strip(&m.name).to_string()));
    let kind = most_common(members.iter().map(|m| normalize_kind(&m.kind)));
    let slug = slugify(&name);
    let base = if slug.is_empty() { "entity".to_string() } else { slug };
    let (mut entity_id, mut n) = (base.clone(), 2);
    while used_ids.contains(&entity_id) {
        entity_id = format!("{base}-{n}");
        n += 1;
    }
    let chunk_ids: std::collections::BTreeSet<i64> = members.iter().flat_map(|m| m.chunk_ids.iter().copied()).collect();
    Entity {
        id: entity_id,
        name,
        kind,
        summary: merge_summaries(members.iter().map(|m| m.summary.as_str())),
        source_chunk_ids: chunk_ids.into_iter().collect(),
    }
}

fn merge_summaries<'a>(summaries: impl Iterator<Item = &'a str>) -> String {
    let mut seen: HashSet<String> = HashSet::new();
    let mut kept: Vec<String> = Vec::new();
    for summary in summaries {
        let summary = squash(summary);
        if !summary.is_empty() && seen.insert(summary.to_lowercase()) {
            kept.push(summary);
        }
    }
    cap(&kept.join(" "), SUMMARY_CAP)
}

/// Trims to `limit` code points, at a sentence end when one falls in the second half, else at a word with "…".
pub fn cap(text: &str, limit: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= limit {
        return text.to_string();
    }
    let cut = &chars[..limit - 1];
    let rfind = |pat: [char; 2]| cut.windows(2).rposition(|w| w == pat);
    let sentence_end = [rfind(['.', ' ']), rfind(['!', ' ']), rfind(['?', ' '])].into_iter().flatten().max();
    if let Some(end) = sentence_end {
        if end >= limit / 2 {
            return cut[..=end].iter().collect();
        }
    }
    let cut: String = cut.iter().collect();
    let head = match cut.rfind(' ') {
        Some(i) => &cut[..i],
        None => cut.as_str(),
    };
    format!("{}…", head.trim_end_matches([',', ';', ':']))
}

fn merge_relations(raw: &[&ExtractedRelation], key_to_id: &HashMap<String, String>) -> Vec<Relation> {
    let mut merged: IndexMap<(String, String, String), Relation> = IndexMap::new();
    for rel in raw {
        let source = key_to_id.get(&match_key(&rel.source));
        let target = key_to_id.get(&match_key(&rel.target));
        let label = py_split(&rel.label).collect::<Vec<_>>().join(" ").to_lowercase();
        let (Some(source), Some(target)) = (source, target) else { continue };
        if source == target || label.is_empty() {
            continue;
        }
        let evidence = cap(&squash(&rel.evidence), EVIDENCE_CAP);
        let key = (source.clone(), target.clone(), label.clone());
        match merged.get_mut(&key) {
            None => {
                merged.insert(key, Relation { source: source.clone(), target: target.clone(), label, evidence });
            }
            Some(existing) if existing.evidence.is_empty() => existing.evidence = evidence,
            Some(_) => {}
        }
    }
    merged.into_values().collect()
}
