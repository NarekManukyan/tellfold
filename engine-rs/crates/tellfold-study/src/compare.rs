//! Compare the runs of a study: a digest per run, then one cited `compare.write` call (port of
//! `tellfold/compare.py`).
//!
//! Every run gets a letter (A, B, ...) in study order, and the model cites posts as `A#12` / `B#7` so each citation
//! points into the right run. The citation audit of the report crate checks those refs, @handles and quotes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::de::DeserializeOwned;
use tellfold_engine::pytext::{char_len, rstrip};
use tellfold_llm::ingest::load_documents;
use tellfold_llm::{json, Llm};
use tellfold_models::json::py_round;
use tellfold_models::text::{py_prefix, py_strip};
use tellfold_models::{
    CampDigest, CompareReport, CompareResult, Post, Report, RoundPoint, RunDigest, ShiftDigest, SimConfig,
    VariantObjections, VariantSummary,
};
use tellfold_report::audit::{audit_texts, mark_unverified_texts, RunPosts, UNVERIFIED};
use tellfold_report::evidence::{clip, collect_evidence, engagement, signed, Evidence, SNIPPET_CHARS};
use tellfold_report::{fill, pyre};
use tellfold_store::files::{now_iso, SEEDS_DIR};
use tellfold_store::world::World;

use crate::Error;

pub const COMPARE_FILE: &str = "compare.json";
/// Characters of each version's text shown to the model.
pub const SEED_CHARS: usize = 4000;
pub const TOP_POSTS: usize = 8;
pub const TOP_SHIFTS: usize = 5;

/// One run of a study, as the comparison sees it.
#[derive(Debug, Clone)]
pub struct Member {
    pub letter: String,
    pub run_id: String,
    pub variant: String,
    pub run_dir: PathBuf,
    /// A read-only example: open its world without writing.
    pub immutable: bool,
}

impl Member {
    pub fn new(letter: &str, run_id: &str, variant: &str, run_dir: &Path) -> Self {
        Member {
            letter: letter.into(),
            run_id: run_id.into(),
            variant: variant.into(),
            run_dir: run_dir.into(),
            immutable: false,
        }
    }
}

pub(crate) struct Loaded {
    pub member: Member,
    pub digest: RunDigest,
    pub config: Option<SimConfig>,
    pub report: Option<Report>,
    pub evidence: Option<Evidence>,
}

pub fn digest_run(member: &Member, state: &str) -> Result<RunDigest, Error> {
    Ok(load(member, state)?.digest)
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

pub(crate) fn load(member: &Member, state: &str) -> Result<Loaded, Error> {
    let config: Option<SimConfig> = read_json(&member.run_dir.join("config.json"));
    let report: Option<Report> = read_json(&member.run_dir.join("report.json"));
    let db = member.run_dir.join("world.db");
    let evidence = if db.is_file() {
        let seed = config.as_ref().map_or(0, |c| c.seed);
        let world = if member.immutable { World::open_read_only(&db, seed)? } else { World::open(&db, seed)? };
        let ev = collect_evidence(&world);
        world.close()?;
        Some(ev?)
    } else {
        None
    };
    let mut digest = RunDigest {
        letter: member.letter.clone(),
        run_id: member.run_id.clone(),
        variant: member.variant.clone(),
        state: state.into(),
        headline: report.as_ref().map(|r| r.title.clone()),
        rounds: Vec::new(),
        final_sentiment: None,
        camps: Vec::new(),
        shifts: Vec::new(),
        objections: Vec::new(),
        posts: 0,
    };
    if let Some(ev) = &evidence {
        let mut per_round: HashMap<i64, i64> = HashMap::new();
        for post in ev.posts.values() {
            *per_round.entry(post.round).or_default() += 1;
        }
        digest.rounds = ev
            .stats
            .iter()
            .map(|s| RoundPoint {
                round: s.round,
                sentiment: s.sentiment,
                posts: per_round.get(&s.round).copied().unwrap_or(s.new_posts),
            })
            .collect();
        digest.final_sentiment = digest.rounds.iter().filter_map(|p| p.sentiment).next_back().map(|s| py_round(s, 3));
        digest.shifts = shifts(ev);
        digest.posts = ev.posts.len() as i64;
    }
    if let Some(r) = &report {
        digest.camps = r
            .camps
            .iter()
            .map(|c| CampDigest {
                name: c.name.clone(),
                size: c.members.len() as i64,
                sentiment: c.sentiment,
                summary: c.summary.clone(),
            })
            .collect();
        digest.objections = r.risks.clone();
    }
    Ok(Loaded { member: member.clone(), digest, config, report, evidence })
}

fn shifts(ev: &Evidence) -> Vec<ShiftDigest> {
    let mut out: Vec<ShiftDigest> = ev
        .shifts
        .iter()
        .map(|s| {
            let (first, last) = (ev.post_sentiment.get(&s.first.id), ev.post_sentiment.get(&s.last.id));
            ShiftDigest {
                agent_id: s.persona.id.clone(),
                handle: s.persona.handle.clone(),
                first_post: s.first.id,
                last_post: s.last.id,
                change: match (first, last) {
                    (Some(f), Some(l)) => Some(py_round(l - f, 3)),
                    _ => None,
                },
            }
        })
        .collect();
    // a stable sort on -abs(change), like Python's
    out.sort_by(|a, b| {
        let key = |d: &ShiftDigest| d.change.unwrap_or(0.0).abs();
        key(b).partial_cmp(&key(a)).unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(TOP_SHIFTS);
    out
}

fn seed_text(run_dir: &Path) -> String {
    let folder = run_dir.join(SEEDS_DIR);
    if !folder.is_dir() {
        return String::new();
    }
    let Ok(entries) = std::fs::read_dir(&folder) else {
        return String::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
        .collect();
    files.sort();
    let Ok(docs) = load_documents(&files) else {
        return String::new();
    };
    let text = docs.iter().map(|(name, body)| format!("[{name}]\n{body}")).collect::<Vec<_>>().join("\n\n");
    if char_len(&text) <= SEED_CHARS {
        text
    } else {
        format!("{} [...]", rstrip(py_prefix(&text, SEED_CHARS)))
    }
}

/// A run's own report cites `#12`; inside a comparison that becomes `A#12`.
fn prefixed(text: &str, letter: &str) -> String {
    pyre::post_ref().replace_all(text, |c: &fancy_regex::Captures<'_>| format!("{letter}#{}", &c[1])).into_owned()
}

fn post_line(post: &Post, ev: &Evidence, letter: &str) -> String {
    let score = ev.post_sentiment.get(&post.id).copied();
    let body = if py_strip(&post.content).is_empty() {
        "(no comment)".to_string()
    } else {
        clip(&post.content, SNIPPET_CHARS)
    };
    format!(
        "{letter}#{} r{} @{} [engagement {}, sentiment {}]: \"{body}\"",
        post.id,
        post.round,
        ev.handle(&post.author_id),
        engagement(post),
        signed(score)
    )
}

fn run_block(loaded: &Loaded, letters: &HashMap<String, String>) -> String {
    let (m, d, ev) = (&loaded.member, &loaded.digest, loaded.evidence.as_ref());
    let mut lines = vec![format!("## {} = {}", m.letter, m.variant)];
    if let Some(config) = &loaded.config {
        if let (Some(from), Some(round)) = (&config.branched_from, config.branch_round) {
            let source = letters.get(from).unwrap_or(from);
            lines.push(format!(
                "A BRANCH of {source}: rounds 0 to {round} are identical to {source}; only what happened after \
                 round {round} differs."
            ));
        }
    }
    let seed = seed_text(&m.run_dir);
    if !seed.is_empty() {
        lines.push("TEXT THE CROWD READ:".into());
        lines.push(seed);
    }
    if let Some(config) = loaded.config.as_ref().filter(|c| !c.events.is_empty()) {
        let events: Vec<String> = config.events.iter().map(|e| format!("round {}: {}", e.round, e.content)).collect();
        lines.push(format!("NEWS EVENTS: {}", events.join("; ")));
    }
    lines.push(format!("final sentiment {}; {} posts", signed(d.final_sentiment), d.posts));
    if !d.rounds.is_empty() {
        let rounds: Vec<String> =
            d.rounds.iter().map(|p| format!("r{} {} ({} posts)", p.round, signed(p.sentiment), p.posts)).collect();
        lines.push(format!("by round: {}", rounds.join(", ")));
    }
    if !d.camps.is_empty() {
        lines.push("camps:".into());
        lines.extend(d.camps.iter().map(|c| {
            format!(
                "- {}: {} people, sentiment {}. {}",
                c.name,
                c.size,
                signed(c.sentiment),
                prefixed(&c.summary, &m.letter)
            )
        }));
    }
    if let Some(r) = &loaded.report {
        lines.push(format!("its report: {}. {}", r.title, prefixed(&r.summary, &m.letter)));
        lines.extend(r.reasons.iter().map(|x| format!("- reason: {}", prefixed(x, &m.letter))));
    }
    if !d.objections.is_empty() {
        lines.push("objections (risks from its report):".into());
        lines.extend(d.objections.iter().map(|x| format!("- {}", prefixed(x, &m.letter))));
    }
    if let Some(ev) = ev {
        let mut top: Vec<&Post> = ev.posts.values().collect();
        top.sort_by_key(|p| (std::cmp::Reverse(engagement(p)), p.id));
        top.truncate(TOP_POSTS);
        if !top.is_empty() {
            lines.push("top posts by engagement:".into());
            lines.extend(top.iter().map(|p| post_line(p, ev, &m.letter)));
        }
        if !d.shifts.is_empty() {
            lines.push("stance shifts (first vs latest post of vocal agents):".into());
            for s in &d.shifts {
                let (first, last) = (&ev.posts[&s.first_post], &ev.posts[&s.last_post]);
                lines.push(format!(
                    "- @{} moved {}: {}#{} \"{}\" -> {}#{} \"{}\"",
                    s.handle,
                    signed(s.change),
                    m.letter,
                    first.id,
                    clip(&first.content, 160),
                    m.letter,
                    last.id,
                    clip(&last.content, 160)
                ));
            }
        }
    }
    lines.join("\n")
}

pub const COMPARE_SYSTEM: &str = "\
You compare how one simulated crowd reacted to different versions of the same announcement, or to the same \
announcement with different news events (a branch that continues a run from a given round). The same people (the \
same personas, with the same backgrounds and starting stances) read each version on a simulated microblogging \
platform over several rounds. You receive a digest per version, computed from each run's database: the text the \
crowd read, sentiment by round, opinion camps, the run's own report, its top posts and stance shifts.

Ground rules:
- Use only the digests. Never invent posts, people, quotes or numbers.
- Every run has a letter. Cite posts with the letter of their run, like A#12 or B#7, never a bare #12. Cite \
people as @handle. Quote short fragments of posts exactly when they make a point vivid.
- Explain differences by what changed in the text or in the events, and say when a difference is small enough to be chance: \
one run per version is a single sample.
- Write every prose field in {language}. Keep the version labels, @handles and refs exactly as given.";

pub const COMPARE_TASK: &str = "\
STUDY QUESTION: {question}

VERSIONS: {legend}

Return one JSON object with these fields:
- headline: one sentence (at most 16 words) that says which version the crowd took better, or that it was a tie.
- better_variant: the exact label of the version the crowd took better ({labels}), or null when it is a tie or \
the evidence is too thin.
- why: 2-4 sentences explaining the verdict, with citations like A#12 and B#7 from both runs.
- per_variant: one object per version, {\"variant\", \"summary\", \"objections\"}: variant is its exact label; summary \
is 1-2 cited sentences on how the crowd took it; objections lists its 1-4 main objections.
- objections_only_in: objects {\"variant\", \"objections\"} with the objections that came up against one version \
and not the other. Leave a version out when it has none.
- caveats: 1-3 honest limits of this comparison.

DIGESTS
{digests}";

/// Digests every member run, makes one `compare.write` call, audits and marks its citations.
pub async fn build_compare(
    llm: &dyn Llm,
    study_id: &str,
    members: &[Member],
    question: &str,
    model: &str,
    language: &str,
    states: Option<&HashMap<String, String>>,
) -> Result<CompareResult, Error> {
    if members.len() < 2 {
        return Err(Error::Invalid("a comparison needs at least two runs".into()));
    }
    let loaded = members
        .iter()
        .map(|m| load(m, states.and_then(|s| s.get(&m.run_id)).map_or("", String::as_str)))
        .collect::<Result<Vec<_>, _>>()?;
    let legend = members.iter().map(|m| format!("{} = {}", m.letter, m.variant)).collect::<Vec<_>>().join("; ");
    let labels = members.iter().map(|m| format!("\"{}\"", m.variant)).collect::<Vec<_>>().join(", ");
    let letters: HashMap<String, String> = members.iter().map(|m| (m.run_id.clone(), m.letter.clone())).collect();
    let digests = loaded.iter().map(|x| run_block(x, &letters)).collect::<Vec<_>>().join("\n\n");
    let prompt =
        fill(COMPARE_TASK, &[("question", question), ("legend", &legend), ("labels", &labels), ("digests", &digests)]);
    let system = fill(COMPARE_SYSTEM, &[("language", language)]);
    let draft = json::<CompareReport>(llm, &prompt, &system, model, "compare.write").await?;
    let report = audit_and_mark(finalize(draft, members), &loaded);
    Ok(CompareResult {
        study_id: study_id.into(),
        created_at: now_iso(),
        runs: loaded.into_iter().map(|x| x.digest).collect(),
        report,
    })
}

/// Labels the model got slightly wrong are mapped back; unknown ones are dropped. One run per version is a sample
/// of one, which can be said with certainty.
fn finalize(mut report: CompareReport, members: &[Member]) -> CompareReport {
    let label = |raw: Option<&str>| -> Option<String> {
        let key = py_strip(raw.unwrap_or("")).trim_matches('"').to_lowercase();
        members
            .iter()
            .find(|m| {
                let (v, l) = (m.variant.to_lowercase(), m.letter.to_lowercase());
                key == v || key == l || key == format!("{l} = {v}")
            })
            .map(|m| m.variant.clone())
    };
    let mut per_variant: IndexMap<String, VariantSummary> = IndexMap::new();
    for v in &report.per_variant {
        if let Some(name) = label(Some(&v.variant)) {
            per_variant.entry(name.clone()).or_insert_with(|| VariantSummary { variant: name, ..v.clone() });
        }
    }
    let mut only_in: IndexMap<String, VariantObjections> = IndexMap::new();
    for v in &report.objections_only_in {
        if let Some(name) = label(Some(&v.variant)).filter(|_| !v.objections.is_empty()) {
            only_in.entry(name.clone()).or_insert_with(|| VariantObjections { variant: name, ..v.clone() });
        }
    }
    let fact = "Each version was played once by the same crowd; part of any difference is chance. Use Check it on \
                each version to see what holds.";
    report.better_variant = label(report.better_variant.as_deref());
    report.per_variant = members.iter().filter_map(|m| per_variant.get(&m.variant).cloned()).collect();
    report.objections_only_in = members.iter().filter_map(|m| only_in.get(&m.variant).cloned()).collect();
    report.caveats.insert(0, fact.into());
    report
}

fn texts(report: &CompareReport) -> Vec<String> {
    let mut out = vec![report.headline.clone(), report.why.clone()];
    for v in &report.per_variant {
        out.push(v.summary.clone());
        out.extend(v.objections.iter().cloned());
    }
    for v in &report.objections_only_in {
        out.extend(v.objections.iter().cloned());
    }
    out.extend(report.caveats.iter().cloned());
    out
}

fn audit_and_mark(mut report: CompareReport, loaded: &[Loaded]) -> CompareReport {
    let runs: RunPosts = loaded
        .iter()
        .map(|x| (x.member.letter.clone(), x.evidence.as_ref().map(|e| e.posts.clone()).unwrap_or_default()))
        .collect();
    let handles: HashSet<String> = loaded
        .iter()
        .filter_map(|x| x.evidence.as_ref())
        .flat_map(|e| e.people.values().map(|p| p.handle.to_lowercase()))
        .collect();
    let none = IndexMap::new();
    let mut audit = audit_texts(&texts(&report), &none, &handles, Some(&runs));
    let mut marked = 0usize;
    if audit.problems() > 0 {
        let mut mark = |text: &mut String| {
            let (out, count) = mark_unverified_texts(text, &none, &handles, Some(&runs));
            marked += count;
            *text = out;
        };
        mark(&mut report.headline);
        mark(&mut report.why);
        for v in &mut report.per_variant {
            mark(&mut v.summary);
            v.objections.iter_mut().for_each(&mut mark);
        }
        for v in &mut report.objections_only_in {
            v.objections.iter_mut().for_each(&mut mark);
        }
        report.caveats.iter_mut().for_each(&mut mark);
    }
    if marked > 0 {
        let noun = if marked == 1 { "citation" } else { "citations" };
        report.caveats.push(format!("{marked} {noun} could not be matched to posts and were marked {UNVERIFIED}."));
    }
    audit.marked = marked as i64;
    report.citation_audit = Some(audit);
    report
}
