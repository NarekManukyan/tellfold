//! Suggest a better version of the seed text: one `report.rewrite` call, its citations checked like the report's
//! (port of `tellfold/rewrite.py`).
//!
//! The model gets the original text, the report (camps, reasons, risks) and the posts the report cites, and returns
//! a `RewriteSuggestion`: the revised text, each change with the posts that motivated it, and what it kept on
//! purpose. The 2A flow ("Test this version") then runs the revised text past the same people.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use indexmap::IndexSet;
use tellfold_engine::pytext::decimal_int;
use tellfold_llm::{json, Llm};
use tellfold_models::text::py_strip;
use tellfold_models::{Report, RewriteChange, RewriteSuggestion, SimConfig};
use tellfold_store::files::{now_iso, SEEDS_DIR};
use tellfold_store::world::World;

use crate::audit::{audit_texts, mark_unverified_texts};
use crate::evidence::{collect_evidence, post_line, Evidence};
use crate::pyre;
use crate::report::confidence_value;
use crate::{fill, ReportError};

pub const REWRITE_FILE: &str = "rewrite.json";
pub const TEXT_SUFFIXES: [&str; 3] = ["md", "markdown", "txt"];
/// The pasted description the Mac app sends as a seed document.
pub const DESCRIPTION_NAME: &str = "description.md";
pub const MAX_CITED_POSTS: usize = 24;
pub const MAX_TEXT_CHARS: usize = 12000;

/// (name, text) of the seed document the suggestion rewrites: the pasted description, else the first text file.
/// The Mac app picks the same file (`StudyLogic.editableSeed`).
pub fn editable_seed(run_dir: &Path) -> Result<(String, String), ReportError> {
    let folder = run_dir.join(SEEDS_DIR);
    let mut files: Vec<PathBuf> = Vec::new();
    if folder.is_dir() {
        for entry in std::fs::read_dir(&folder)? {
            let path = entry?.path();
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let suffix = path.extension().map(|e| e.to_string_lossy().to_lowercase());
            if path.is_file() && suffix.is_some_and(|s| TEXT_SUFFIXES.contains(&s.as_str())) && !name.starts_with('.') {
                files.push(path);
            }
        }
    }
    files.sort();
    let name_of = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let chosen = files.iter().find(|p| name_of(p).to_lowercase() == DESCRIPTION_NAME).or(files.first());
    let Some(chosen) = chosen else {
        return Err(ReportError::Rewrite(
            "this run has no text document to rewrite (only .md and .txt seeds can be rewritten)".into(),
        ));
    };
    let bytes = std::fs::read(chosen)?;
    // `read_text` decodes in universal-newline mode: `\r\n` and a lone `\r` both become `\n`.
    let decoded = String::from_utf8_lossy(&bytes).replace("\r\n", "\n").replace('\r', "\n");
    let text = py_strip(&decoded).to_string();
    if text.is_empty() {
        return Err(ReportError::Rewrite(format!("{} is empty", name_of(chosen))));
    }
    Ok((name_of(chosen), text))
}

pub const REWRITE_SYSTEM: &str = "\
You are an editor. A simulated crowd of people read an announcement on a social platform, and an analyst wrote a \
report on how they reacted. You rewrite the announcement so the same people would take it better, without changing \
what is being announced: keep the decision, the facts and the numbers unless the crowd showed they were the problem \
and the text can honestly change them.

Rules:
- Change only what the crowd's reaction gives a reason to change. Keep the author's voice, format and length.
- Every change cites the posts that motivated it, by id. In \"why\", cite posts as #id and people as @handle, and \
quote their exact words when it helps.
- List what you kept on purpose even though someone objected, and say why.
- Write the revised text and every explanation in {language}.";

pub const REWRITE_TASK: &str = "\
STUDY QUESTION: {question}

ORIGINAL TEXT ({name})
{text}

THE REPORT
{report}

POSTS THE REPORT CITES AND THE MOST ENGAGED POSTS
{posts}

Return one JSON object:
- revised_text: the whole announcement, rewritten. Plain text or the same markdown as the original.
- changes: 2-6 objects {\"what\", \"why\", \"cites\"}: what changed (a few words), why (one or two sentences on the \
crowd's reaction, with #id citations) and cites (the post ids, as numbers).
- kept_on_purpose: 0-4 short notes on what you did not change, and why.";

/// `f"{n:,}"`.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn report_text(report: &Report) -> String {
    let confidence = confidence_value(report.confidence);
    let mut lines =
        vec![report.title.clone(), report.summary.clone(), format!("Prediction ({confidence}): {}", report.prediction)];
    lines.extend(report.reasons.iter().map(|r| format!("- reason: {r}")));
    lines.extend(report.camps.iter().map(|c| format!("- camp {} ({} people): {}", c.name, c.members.len(), c.summary)));
    lines.extend(report.risks.iter().map(|r| format!("- risk: {r}")));
    lines.join("\n")
}

fn cited_posts(report: &Report, ev: &Evidence) -> Vec<i64> {
    let mut texts: Vec<&str> = vec![&report.summary, &report.prediction];
    texts.extend(report.reasons.iter().map(String::as_str));
    texts.extend(report.sections.iter().map(|s| s.body_markdown.as_str()));
    texts.extend(report.camps.iter().map(|c| c.summary.as_str()));
    texts.extend(report.risks.iter().map(String::as_str));
    let mut ids: IndexSet<i64> = IndexSet::new();
    for text in texts {
        for c in pyre::post_ref().captures_iter(text).flatten() {
            if let Some(pid) = decimal_int(&c[1]) {
                ids.insert(pid);
            }
        }
    }
    ids.extend(ev.top_posts.iter().map(|p| p.id));
    ids.into_iter().filter(|pid| ev.posts.contains_key(pid)).take(MAX_CITED_POSTS).collect()
}

pub fn rewrite_prompt(name: &str, text: &str, config: &SimConfig, report: &Report, ev: &Evidence) -> String {
    let posts = cited_posts(report, ev).iter().map(|pid| post_line(&ev.posts[pid], ev)).collect::<Vec<_>>().join("\n");
    let posts = if posts.is_empty() { "none".to_string() } else { posts };
    let report = report_text(report);
    fill(
        REWRITE_TASK,
        &[("question", &config.question), ("name", name), ("text", text), ("report", &report), ("posts", &posts)],
    )
}

pub async fn build_rewrite(
    llm: &dyn Llm,
    run_dir: &Path,
    config: &SimConfig,
    report: &Report,
    model: &str,
    read_only: bool,
) -> Result<RewriteSuggestion, ReportError> {
    let (name, text) = editable_seed(run_dir)?;
    let chars = text.chars().count();
    if chars > MAX_TEXT_CHARS {
        // "Test this version" replaces the whole document with the revised text, so a rewrite of only its start
        // would silently drop the rest
        return Err(ReportError::Rewrite(format!(
            "{name} is too long to rewrite ({} characters; the limit is {}), because the suggestion replaces the \
             whole text",
            thousands(chars),
            thousands(MAX_TEXT_CHARS)
        )));
    }
    let db = run_dir.join("world.db");
    let world = if read_only { World::open_read_only(&db, config.seed)? } else { World::open(&db, config.seed)? };
    let ev = collect_evidence(&world);
    world.close()?;
    let ev = ev?;
    let prompt = rewrite_prompt(&name, &text, config, report, &ev);
    let system = fill(REWRITE_SYSTEM, &[("language", &config.language)]);
    let draft = json::<RewriteSuggestion>(llm, &prompt, &system, model, "report.rewrite").await?;
    let mut out = audit_rewrite(draft, &ev);
    out.source_name = name;
    out.created_at = now_iso();
    Ok(out)
}

/// Unknown ids leave `cites` and count as unknown posts; citations in `why` that don't match are marked.
pub fn audit_rewrite(draft: RewriteSuggestion, ev: &Evidence) -> RewriteSuggestion {
    let handles: HashSet<String> = ev.people.values().map(|p| p.handle.to_lowercase()).collect();
    let mut texts: Vec<String> = draft.changes.iter().map(|c| c.why.clone()).collect();
    texts.extend(draft.kept_on_purpose.iter().cloned());
    let mut audit = audit_texts(&texts, &ev.posts, &handles, None);
    let unknown: Vec<i64> =
        draft.changes.iter().flat_map(|c| c.cites.iter().copied()).filter(|pid| !ev.posts.contains_key(pid)).collect();
    let problems = audit.problems() > 0;
    let mut marked = 0usize;
    let mut mark = |text: &str| -> String {
        let (text, count) = mark_unverified_texts(text, &ev.posts, &handles, None);
        marked += count;
        text
    };
    let changes: Vec<RewriteChange> = draft
        .changes
        .iter()
        .filter(|c| !py_strip(&c.what).is_empty())
        .map(|c| RewriteChange {
            what: py_strip(&c.what).to_string(),
            why: if problems { mark(&c.why) } else { c.why.clone() },
            cites: c
                .cites
                .iter()
                .copied()
                .collect::<IndexSet<i64>>()
                .into_iter()
                .filter(|p| ev.posts.contains_key(p))
                .collect(),
        })
        .collect();
    let kept: Vec<String> =
        if problems { draft.kept_on_purpose.iter().map(|k| mark(k)).collect() } else { draft.kept_on_purpose.clone() };
    audit.checked += draft.changes.iter().map(|c| c.cites.len() as i64).sum::<i64>();
    let mut unknown_posts: IndexSet<i64> = audit.unknown_posts.iter().copied().collect();
    unknown_posts.extend(unknown.iter().copied());
    audit.unknown_posts = unknown_posts.into_iter().collect();
    audit.marked = (marked + unknown.len()) as i64;
    RewriteSuggestion { changes, kept_on_purpose: kept, citation_audit: Some(audit), ..draft }
}
