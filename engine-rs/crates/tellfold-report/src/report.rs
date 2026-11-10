//! The report call: prompt, finalisation, citation repair and marking, and the Markdown version.

use std::collections::HashSet;

use indexmap::IndexMap;
use tellfold_engine::pytext::decimal_int;
use tellfold_llm::{json, Llm};
use tellfold_models::json::{py_round, py_sum};
use tellfold_models::text::{py_split, py_strip};
use tellfold_models::{Camp, CitationAudit, KnowledgeGraph, Recommendation, RecommendationAction, Report, SimConfig};
use tellfold_store::world::World;

use crate::audit::{all_ok, audit_citations, map_texts, mark_unverified_texts, UNVERIFIED};
use crate::evidence::{clip, collect_evidence, digest_text, rounds_done, scores_by_author, Evidence};
use crate::pyre;
use crate::{fill, ReportError};

pub const REPORT_SYSTEM: &str = "\
You are the lead analyst of a social-simulation study. A population of language-model agents, each playing a \
distinct persona with its own background and starting stance, reacted to a scenario on a simulated microblogging \
platform over several rounds. You receive a digest computed directly from the simulation database: round-by-round \
numbers, the most engaged posts and threads, the most-followed agents, how vocal agents' positions moved, and the \
measured effect of injected news events. You write the final briefing that answers the study question.

Ground rules:
- Use only the digest. Never invent posts, agents, quotes, numbers or events. When evidence is thin or mixed, say so.
- Cite inline: agents as @handle and posts as #id (for example \"@some_handle's reply #41\"). Every substantive claim \
needs at least one citation; quote short fragments of posts when they make a point vivid.
- Answer the question first, then explain how the simulated crowd got there.
- Keep two things apart: what the simulated agents did, and what that suggests about the real world.
- Look for structure, not just averages: camps and who leads them, minority views that gained or lost ground, \
turning points, and agents who changed their mind.
- Calibrate confidence. \"high\" only when the signal is consistent across rounds and across different kinds of \
agents and survives injected events; \"medium\" for a clear lean with notable dissent or volatility; \"low\" when \
sentiment flipped, activity was sparse, or the crowd split evenly.
- Recommendations are hypotheses from a simulated crowd, not proven fixes: each one must rest on specific posts or \
agents in the digest and name something concrete from the scenario. Never give generic advice.
- Write every prose field in {language}, including the title, reasons, camp names and recommendations. Keep \
@handles, #ids, persona ids, JSON keys and the action and confidence values exactly as given.";

pub const REPORT_TASK: &str = "\
STUDY QUESTION: {question}

Return one JSON object with these fields:
- title: a specific headline (at most 12 words) that states the finding. Not \"Simulation report\".
- summary: 3-5 sentences that answer the study question directly, with citations.
- prediction: one paragraph naming the single most likely outcome, the conditions it depends on, and the strongest \
counter-signal.
- confidence: \"low\", \"medium\" or \"high\", following the calibration rules.
- sections: 3-5 objects {\"title\", \"body_markdown\"}. Cover how the conversation evolved round by round, the camps \
and the voices leading them, turning points (including the injected events and their measured effect), and \
dissenting views worth watching. body_markdown may use short paragraphs, \"- \" bullet lists and **bold**; no \
tables, links or headings.
- key_voices: 3-8 persona ids (like \"a04\", not handles) whose posts most shaped the outcome.
- reasons: 2-4 one-line reasons (at most 20 words each) that explain the title, each with at least one citation. \
They are the first thing a reader sees under the headline.
- camps: 2-5 objects {\"name\", \"summary\", \"members\"} that split the crowd into opinion groups by where each agent \
ended up. name is a short plain label (2-5 words, e.g. \"Stayed on free\", \"Turned against Plus\"); summary is one \
sentence with citations; members lists persona ids. Every agent in the digest belongs to exactly one camp; put \
organisations with the camp their posts support, or in their own camp when they mostly stayed neutral.
- risks: 2-5 concrete ways the real-world outcome could differ from the prediction.
- caveats: 2-5 honest limits of this run as evidence, tied to its numbers where possible (population size and mix, \
all agents share one underlying model and can converge, a compressed timeline, the seed documents frame what agents \
know, sentiment is model-scored).
- recommendations: 3-6 objects {\"action\", \"title\", \"why\", \"addresses\", \"evidence\", \"expected_effect\", \
\"confidence\"}: what to do next, drawn only from how this crowd reacted. action is one of:
  \"keep\" - something the crowd clearly valued; protect it.
  \"change\" - something that drew pushback but can be fixed, including confusing or provoking wording.
  \"add\" - something the crowd asked for or missed.
  \"drop\" - something the crowd clearly rejected; don't do it. Include at least one when the crowd clearly rejected \
something.
  \"test\" - an open question the crowd split on; check it with real people before deciding.
  title: a short imperative, at most 10 words, naming the specific feature, price, rule, wording or group (e.g. \
\"Drop the annual-only billing\", \"Keep the free tier for students\"). why: one or two sentences with citations, \
saying what in the reactions supports it. addresses: the camp names (exactly as in camps) or persona ids it \
responds to. evidence: 1-4 citations, each exactly \"#id\" or \"@handle\", of the posts or agents behind it. \
expected_effect: one sentence on how the crowd's reaction would likely change, and for which camp. confidence: \
\"low\", \"medium\" or \"high\" for this recommendation alone, following the calibration rules.
  No generic advice such as \"communicate clearly\" or \"listen to feedback\". A recommendation without evidence is \
removed. These are hypotheses from a simulated crowd, to be tested with real people before acting on them.

EVIDENCE DIGEST
{digest}";

pub const MAX_RECOMMENDATIONS: usize = 6;

/// `{"keep": "Keep", ...}`: the same words are in the web Studio and the Mac app.
pub fn action_label(action: RecommendationAction) -> &'static str {
    match action {
        RecommendationAction::Keep => "Keep",
        RecommendationAction::Change => "Change",
        RecommendationAction::Add => "Add",
        RecommendationAction::Drop => "Drop",
        RecommendationAction::Test => "Test",
    }
}

pub fn action_meaning(action: RecommendationAction) -> &'static str {
    match action {
        RecommendationAction::Keep => "The crowd valued this. Protect it.",
        RecommendationAction::Change => "This drew pushback that a change could fix.",
        RecommendationAction::Add => "The crowd asked for this or missed it.",
        RecommendationAction::Drop => "The crowd clearly rejected this. Don't do it.",
        RecommendationAction::Test => "The crowd split on this. Check it with real people first.",
    }
}

pub fn action_value(action: RecommendationAction) -> &'static str {
    match action {
        RecommendationAction::Keep => "keep",
        RecommendationAction::Change => "change",
        RecommendationAction::Add => "add",
        RecommendationAction::Drop => "drop",
        RecommendationAction::Test => "test",
    }
}

pub const RECOMMENDATIONS_NOTE: &str =
    "Suggestions drawn from how the simulated crowd reacted. Treat them as ideas to test with real people, not proof.";

fn lower_handles(ev: &Evidence) -> IndexMap<String, String> {
    ev.people.values().map(|p| (p.handle.to_lowercase(), p.id.clone())).collect()
}

fn handle_set(ev: &Evidence) -> HashSet<String> {
    ev.people.values().map(|p| p.handle.to_lowercase()).collect()
}

/// A persona id or handle (with or without "@") as a persona id.
fn agent_id(raw: &str, ev: &Evidence, by_handle: &IndexMap<String, String>) -> Option<String> {
    let key = py_strip(raw).trim_start_matches('@');
    if ev.people.contains_key(key) {
        Some(key.to_string())
    } else {
        by_handle.get(&key.to_lowercase()).cloned()
    }
}

pub async fn build_report(
    llm: &dyn Llm,
    world: &World,
    graph: &KnowledgeGraph,
    config: &SimConfig,
    model: &str,
) -> Result<Report, ReportError> {
    let ev = collect_evidence(world)?;
    let digest = digest_text(&ev, Some(graph));
    let prompt = fill(REPORT_TASK, &[("question", &config.question), ("digest", &digest)]);
    let system = fill(REPORT_SYSTEM, &[("language", &config.language)]);
    let draft = json::<Report>(llm, &prompt, &system, model, "report.write").await?;
    let mut report = finalize(draft.clone(), &ev, config);
    let handles = handle_set(&ev);
    let mut audit = audit_citations(&report, &ev.posts, handles.iter().map(String::as_str));
    let mut repaired = false;
    if audit.problems() > 0 {
        // One repair call, like the JSON repair: show the model what did not match and ask again.
        repaired = true;
        let repair = citation_repair_prompt(&prompt, &draft, &audit, &ev);
        match json::<Report>(llm, &repair, &system, model, "report.repair").await {
            Ok(draft) => {
                report = finalize(draft, &ev, config);
                audit = audit_citations(&report, &ev.posts, handles.iter().map(String::as_str));
            }
            Err(e) => eprintln!("citation repair call failed; marking the unmatched citations instead: {e}"),
        }
    }
    Ok(mark_unverified(report, audit, &ev, repaired))
}

/// Repairs what the model may get wrong and adds the caveats that can be stated with certainty.
fn finalize(mut report: Report, ev: &Evidence, config: &SimConfig) -> Report {
    let by_handle = lower_handles(ev);
    let mut voices: Vec<String> = Vec::new();
    for raw in &report.key_voices {
        if let Some(id) = agent_id(raw, ev, &by_handle) {
            if !voices.contains(&id) {
                voices.push(id);
            }
        }
    }
    if voices.is_empty() {
        let mut agents: Vec<_> = ev.agents.iter().collect();
        agents.sort_by_key(|a| -a.engagement);
        voices = agents.into_iter().take(3).filter(|a| a.posts != 0).map(|a| a.persona.id.clone()).collect();
    }
    let camps = finalize_camps(&report.camps, ev, &by_handle);
    let mut caveats = fact_caveats(ev, config);
    caveats.extend(report.caveats);
    report.recommendations = finalize_recommendations(&report.recommendations, ev, &camps, &by_handle);
    report.key_voices = voices;
    report.camps = camps;
    report.caveats = caveats;
    report
}

/// Every agent in exactly one camp (first mention wins, leftovers in "Other"), with a measured sentiment.
pub fn finalize_camps(camps: &[Camp], ev: &Evidence, by_handle: &IndexMap<String, String>) -> Vec<Camp> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<Camp> = Vec::new();
    for camp in camps {
        let mut members: Vec<String> = Vec::new();
        for raw in &camp.members {
            if let Some(id) = agent_id(raw, ev, by_handle) {
                if seen.insert(id.clone()) {
                    members.push(id);
                }
            }
        }
        let name = py_strip(&camp.name);
        if !members.is_empty() && !name.is_empty() {
            out.push(Camp { name: name.to_string(), members, ..camp.clone() });
        }
    }
    if !out.is_empty() {
        let rest: Vec<String> = ev.people.keys().filter(|id| !seen.contains(*id)).cloned().collect();
        if !rest.is_empty() {
            out.push(Camp {
                name: "Other".into(),
                summary: "Agents the report did not place in a camp.".into(),
                members: rest,
                sentiment: None,
            });
        }
    }
    let scores = scores_by_author(ev);
    for camp in &mut out {
        let values: Vec<f64> = camp.members.iter().flat_map(|id| scores.get(id).cloned().unwrap_or_default()).collect();
        camp.sentiment =
            (!values.is_empty()).then(|| py_round(py_sum(values.iter().copied()) / values.len() as f64, 3));
    }
    out
}

/// The citations in one evidence entry, as "#12" / "@handle". Bare numbers, handles and persona ids are accepted.
fn evidence_refs(raw: &str, ev: &Evidence, by_handle: &IndexMap<String, String>) -> Vec<String> {
    let handle = |h: &str| match by_handle.get(&h.to_lowercase()) {
        Some(id) => format!("@{}", ev.people[id].handle),
        None => format!("@{h}"), // an unknown handle is left for the audit
    };
    let text = py_strip(raw);
    let mut refs: Vec<String> = pyre::post_ref().captures_iter(text).flatten().map(|c| format!("#{}", &c[1])).collect();
    refs.extend(pyre::handle_ref().captures_iter(text).flatten().map(|c| handle(&c[1])));
    if !refs.is_empty() {
        return refs;
    }
    if let Some(pid) = decimal_int(text) {
        return vec![format!("#{pid}")];
    }
    if let Some(p) = ev.people.get(text) {
        return vec![format!("@{}", p.handle)];
    }
    if by_handle.contains_key(&text.to_lowercase()) {
        return vec![handle(text)];
    }
    Vec::new()
}

fn unique_refs(texts: &[&str], ev: &Evidence, by_handle: &IndexMap<String, String>) -> Vec<String> {
    let mut refs: Vec<String> = Vec::new();
    for raw in texts {
        for r in evidence_refs(raw, ev, by_handle) {
            if !refs.iter().any(|x| x.to_lowercase() == r.to_lowercase()) {
                refs.push(r);
            }
        }
    }
    refs
}

/// Evidence as "#id" / "@handle" citations (taken from the prose when no citation parses from the list), addresses
/// as camp names or persona ids, no repeated step and no step without evidence. Whether the citations exist is the
/// audit's job.
fn finalize_recommendations(
    recs: &[Recommendation],
    ev: &Evidence,
    camps: &[Camp],
    by_handle: &IndexMap<String, String>,
) -> Vec<Recommendation> {
    let camp_names: IndexMap<String, String> = camps.iter().map(|c| (c.name.to_lowercase(), c.name.clone())).collect();
    let mut seen: HashSet<(&str, String)> = HashSet::new();
    let mut out = Vec::new();
    for rec in recs {
        let key = (action_value(rec.action), py_split(&rec.title.to_lowercase()).collect::<Vec<_>>().join(" "));
        if key.1.is_empty() || seen.contains(&key) {
            continue; // untitled, or the same step twice
        }
        seen.insert(key);
        let listed: Vec<&str> = rec.evidence.iter().map(String::as_str).collect();
        let mut evidence = unique_refs(&listed, ev, by_handle);
        if evidence.is_empty() {
            evidence = unique_refs(&[&rec.why, &rec.expected_effect], ev, by_handle);
        }
        if evidence.is_empty() {
            continue; // dropping a recommendation without evidence
        }
        let mut addresses: Vec<String> = Vec::new();
        for raw in &rec.addresses {
            let target =
                agent_id(raw, ev, by_handle).or_else(|| camp_names.get(&py_strip(raw).to_lowercase()).cloned());
            if let Some(t) = target {
                if !addresses.contains(&t) {
                    addresses.push(t);
                }
            }
        }
        out.push(Recommendation { title: py_strip(&rec.title).to_string(), evidence, addresses, ..rec.clone() });
    }
    out.truncate(MAX_RECOMMENDATIONS);
    out
}

fn fact_caveats(ev: &Evidence, config: &SimConfig) -> Vec<String> {
    let mut caveats = vec![format!(
        "Scale: {} simulated agents over {} rounds produced {} posts; agent decisions came from the '{}' model. \
Treat this as a scenario exploration, not a poll or a measurement of real people.",
        ev.people.len(),
        rounds_done(ev),
        ev.posts.len(),
        config.agent_model
    )];
    let unscored: Vec<String> =
        ev.stats.iter().filter(|s| s.sentiment.is_none()).map(|s| s.round.to_string()).collect();
    if !unscored.is_empty() {
        caveats.push(format!("Sentiment could not be scored for round(s) {}.", unscored.join(", ")));
    }
    if (rounds_done(ev) as i64) < config.rounds {
        caveats.push(format!("Only {} of {} planned rounds completed.", rounds_done(ev), config.rounds));
    }
    caveats
}

fn citation_repair_prompt(prompt: &str, draft: &Report, audit: &CitationAudit, ev: &Evidence) -> String {
    let mut problems: Vec<String> =
        audit.unknown_posts.iter().map(|pid| format!("- #{pid} is not a post in this run.")).collect();
    problems.extend(audit.unknown_handles.iter().map(|h| format!("- @{h} is not an agent in this run.")));
    problems.extend(audit.misquotes.iter().map(|q| {
        format!(
            "- The quote \"{}\" is not in post #{}, which says: \"{}\"",
            clip(&q.quote, 160),
            q.post_id,
            clip(&ev.posts[&q.post_id].content, 240)
        )
    }));
    let mut value = serde_json::to_value(draft).expect("report serializes");
    value.as_object_mut().expect("object").remove("citation_audit");
    let previous = tellfold_models::json::to_string(&value);
    format!(
        "{prompt}\n\n---\nYour previous report cites things that are not in the digest:\n{}\n\nPrevious report:\n{previous}\n\n\
Return the whole report again with every citation fixed: cite a post or agent that exists in the digest, \
quote the post's exact words, or drop the claim. Keep everything else as it was.",
        problems.join("\n")
    )
}

/// The one-line summary shown under the confidence badge; None for older reports and reports without citations.
pub fn citation_line(audit: Option<&CitationAudit>) -> Option<String> {
    let audit = audit.filter(|a| a.checked != 0)?;
    Some(if audit.marked != 0 {
        format!(
            "{} of {} citations checked against the posts, {} marked",
            audit.checked - audit.marked,
            audit.checked,
            audit.marked
        )
    } else if audit.checked == 1 {
        "The 1 citation was checked against the posts".into()
    } else {
        format!("All {} citations checked against the posts", audit.checked)
    })
}

/// Replaces every citation that still doesn't match with [unverified] and says so in a caveat.
fn mark_unverified(report: Report, audit: CitationAudit, ev: &Evidence, repaired: bool) -> Report {
    let handles = handle_set(ev);
    let mut marked = 0usize;
    let mut report = report;
    if audit.problems() > 0 {
        report = map_texts(&report, &mut |text: &str| {
            let (text, count) = mark_unverified_texts(text, &ev.posts, &handles, None);
            marked += count;
            text
        });
        // Evidence is a list of citations, so a wrong one is removed rather than replaced, and a recommendation
        // left with no evidence is dropped.
        let mut recs = Vec::new();
        for mut rec in std::mem::take(&mut report.recommendations) {
            let before = rec.evidence.len();
            rec.evidence.retain(|e| all_ok(e, &ev.posts, &handles));
            marked += before - rec.evidence.len();
            if !rec.evidence.is_empty() {
                recs.push(rec);
            }
        }
        report.recommendations = recs;
    }
    if marked != 0 {
        let noun = if marked == 1 { "citation" } else { "citations" };
        report.caveats.push(format!("{marked} {noun} could not be matched to posts and were marked {UNVERIFIED}."));
    }
    report.citation_audit = Some(CitationAudit { repaired, marked: marked as i64, ..audit });
    report
}

// ---------------------------------------------------------------- markdown

fn recommendation_markdown(rec: &Recommendation) -> String {
    let rating = rec.confidence.map(|c| format!(", {} confidence", confidence_value(c))).unwrap_or_default();
    let mut lines = vec![format!("- **{}** ({}{rating})", rec.title, action_label(rec.action))];
    if !rec.why.is_empty() {
        lines.push(format!("  {}", rec.why));
    }
    if !rec.expected_effect.is_empty() {
        lines.push(format!("  Likely effect: {}", rec.expected_effect));
    }
    if !rec.addresses.is_empty() {
        lines.push(format!("  For: {}", rec.addresses.join(", ")));
    }
    lines.push(format!("  Based on: {}", rec.evidence.join(", ")));
    lines.join("\n")
}

pub fn confidence_value(c: tellfold_models::Confidence) -> &'static str {
    match c {
        tellfold_models::Confidence::Low => "low",
        tellfold_models::Confidence::Medium => "medium",
        tellfold_models::Confidence::High => "high",
    }
}

pub fn render_markdown(report: &Report) -> String {
    let mut parts = vec![
        format!("# {}", report.title),
        report.summary.clone(),
        format!("## Prediction\n\n{}\n\n**Confidence:** {}", report.prediction, confidence_value(report.confidence)),
    ];
    parts.extend(report.sections.iter().map(|s| format!("## {}\n\n{}", s.title, s.body_markdown)));
    if !report.recommendations.is_empty() {
        let recs = report.recommendations.iter().map(recommendation_markdown).collect::<Vec<_>>().join("\n");
        parts.push(format!("## What to do next\n\n{RECOMMENDATIONS_NOTE}\n\n{recs}"));
    }
    if !report.key_voices.is_empty() {
        parts.push(format!("## Key voices\n\n{}", report.key_voices.join(", ")));
    }
    for (title, items) in [("Risks", &report.risks), ("Caveats", &report.caveats)] {
        if !items.is_empty() {
            let list = items.iter().map(|i| format!("- {i}")).collect::<Vec<_>>().join("\n");
            parts.push(format!("## {title}\n\n{list}"));
        }
    }
    parts.join("\n\n") + "\n"
}
