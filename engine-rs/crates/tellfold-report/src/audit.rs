//! The citation audit: every `#id`, `@handle` and quoted fragment in a text checked against the run.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use tellfold_engine::pytext::{decimal_int, is_word};
use tellfold_models::text::{py_isspace, py_split};
use tellfold_models::{CitationAudit, Misquote, Post, Report};

use crate::pyre;

pub const UNVERIFIED: &str = "[unverified]";
const MIN_QUOTE_WORDS: usize = 3;

/// Letter -> that run's posts, for citations in a study comparison ("A#12").
pub type RunPosts = HashMap<String, IndexMap<i64, Post>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Post,
    Handle,
    Quote,
    Ref,
}

#[derive(Debug, Clone)]
struct Citation {
    kind: Kind,
    /// byte span in the text, replaced by [`UNVERIFIED`] when the citation is marked
    start: usize,
    end: usize,
    ok: bool,
    post_id: Option<i64>,
    handle: Option<String>,
    quote: Option<String>,
    /// "A#12", for kind Ref and for quotes paired with a prefixed ref
    reference: Option<String>,
}

impl Citation {
    fn new(kind: Kind, start: usize, end: usize, ok: bool) -> Self {
        Citation { kind, start, end, ok, post_id: None, handle: None, quote: None, reference: None }
    }
}

/// Every prose field of a report that can carry citations.
pub fn report_texts(report: &Report) -> Vec<String> {
    let mut texts = vec![report.summary.clone(), report.prediction.clone()];
    texts.extend(report.reasons.iter().cloned());
    texts.extend(report.sections.iter().map(|s| s.body_markdown.clone()));
    texts.extend(report.camps.iter().map(|c| c.summary.clone()));
    texts.extend(report.risks.iter().cloned());
    texts.extend(report.caveats.iter().cloned());
    for r in &report.recommendations {
        texts.extend([r.title.clone(), r.why.clone(), r.expected_effect.clone(), r.evidence.join(" ")]);
    }
    texts
}

/// The report with `f` applied to every prose field that can carry citations (not recommendation evidence).
pub fn map_texts(report: &Report, f: &mut dyn FnMut(&str) -> String) -> Report {
    let mut out = report.clone();
    out.summary = f(&report.summary);
    out.prediction = f(&report.prediction);
    out.reasons = report.reasons.iter().map(|t| f(t)).collect();
    for s in &mut out.sections {
        s.body_markdown = f(&s.body_markdown);
    }
    for c in &mut out.camps {
        c.summary = f(&c.summary);
    }
    out.risks = report.risks.iter().map(|t| f(t)).collect();
    out.caveats = report.caveats.iter().map(|t| f(t)).collect();
    for r in &mut out.recommendations {
        r.title = f(&r.title);
        r.why = f(&r.why);
        r.expected_effect = f(&r.expected_effect);
    }
    out
}

/// Case, whitespace and punctuation folded away, so a quote matches however the model punctuated it.
fn normalise(text: &str) -> String {
    let lowered = text.to_lowercase().replace(['\u{2019}', '\u{2018}'], "'");
    let replaced: String = lowered.chars().map(|c| if is_word(c) || py_isspace(c) { c } else { ' ' }).collect();
    py_split(&replaced).collect::<Vec<_>>().join(" ")
}

/// Every piece of the quote (split at "..." elisions) appears, as whole words, in one of the sources.
fn quote_matches(quote: &str, sources: &[&str]) -> bool {
    let mut parts = Vec::new();
    let mut last = 0;
    for m in pyre::ellipsis().find_iter(quote).flatten() {
        parts.push(&quote[last..m.start()]);
        last = m.end();
    }
    parts.push(&quote[last..]);
    let parts: Vec<String> = parts.into_iter().map(normalise).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return true;
    }
    sources.iter().any(|source| {
        let hay = format!(" {} ", normalise(source));
        parts.iter().all(|part| hay.contains(&format!(" {part} ")))
    })
}

fn post_sources(post_id: i64, posts: &IndexMap<i64, Post>) -> Vec<&str> {
    let post = &posts[&post_id];
    let mut sources = vec![post.content.as_str()];
    if let Some(original) = post.repost_of.and_then(|id| posts.get(&id)) {
        sources.push(&original.content);
    }
    sources
}

fn sentence_bounds(masked: &str, start: usize, end: usize) -> (usize, usize) {
    // `finditer(masked, 0, start)`: the text ends at `start`, so "$" matches there
    let lo = pyre::sentence_end().find_iter(&masked[..start]).flatten().last().map_or(0, |m| m.end());
    let hi = pyre::sentence_end().find_from_pos(masked, end).ok().flatten().map_or(masked.len(), |m| m.end());
    (lo, hi)
}

fn citations(
    text: &str,
    posts: &IndexMap<i64, Post>,
    handles: &HashSet<String>,
    runs: Option<&RunPosts>,
) -> Vec<Citation> {
    let quotes: Vec<(usize, usize, String)> = pyre::quote()
        .captures_iter(text)
        .flatten()
        .map(|c| {
            let whole = c.get(0).expect("match");
            let inner = (1..c.len()).filter_map(|i| c.get(i)).map(|g| g.as_str()).find(|g| !g.is_empty()).unwrap_or("");
            (whole.start(), whole.end(), inner.to_string())
        })
        .collect();
    let inside_quote = |i: usize| quotes.iter().any(|(a, b, _)| *a <= i && i < *b);

    let mut found = Vec::new();
    for c in pyre::post_ref().captures_iter(text).flatten() {
        let m = c.get(0).expect("match");
        if !inside_quote(m.start()) {
            let pid = decimal_int(&c[1]).unwrap_or(-1);
            let mut cite = Citation::new(Kind::Post, m.start(), m.end(), posts.contains_key(&pid));
            cite.post_id = Some(pid);
            found.push(cite);
        }
    }
    if let Some(runs) = runs {
        for c in pyre::prefixed_ref().captures_iter(text).flatten() {
            let m = c.get(0).expect("match");
            if !inside_quote(m.start()) {
                let (letter, pid) = (c[1].to_string(), decimal_int(&c[2]).unwrap_or(-1));
                let ok = runs.get(&letter).is_some_and(|p| p.contains_key(&pid));
                let mut cite = Citation::new(Kind::Ref, m.start(), m.end(), ok);
                cite.post_id = Some(pid);
                cite.reference = Some(format!("{letter}#{pid}"));
                found.push(cite);
            }
        }
    }
    for c in pyre::handle_ref().captures_iter(text).flatten() {
        let m = c.get(0).expect("match");
        if !inside_quote(m.start()) {
            let handle = c[1].to_string();
            let mut cite = Citation::new(Kind::Handle, m.start(), m.end(), handles.contains(&handle.to_lowercase()));
            cite.handle = Some(handle);
            found.push(cite);
        }
    }
    // Sentence ends inside quotes don't count, so a quote is paired with the #ids of the sentence around it.
    let mut masked = text.as_bytes().to_vec();
    for (a, b, _) in &quotes {
        masked[*a..*b].fill(0);
    }
    let masked = String::from_utf8(masked).expect("quotes cover whole characters");
    let empty = IndexMap::new();
    for (a, b, quote) in &quotes {
        if py_split(quote).count() < MIN_QUOTE_WORDS {
            continue;
        }
        let (lo, hi) = sentence_bounds(&masked, *a, *b);
        let sentence = &masked[lo..hi];
        // (posts of the run the ref points into, post id, "A#12" or None)
        let mut cited: Vec<(&IndexMap<i64, Post>, i64, Option<String>)> = pyre::post_ref()
            .captures_iter(sentence)
            .flatten()
            .map(|c| (posts, decimal_int(&c[1]).unwrap_or(-1), None))
            .collect();
        if let Some(runs) = runs {
            cited.extend(pyre::prefixed_ref().captures_iter(sentence).flatten().map(|c| {
                let pid = decimal_int(&c[2]).unwrap_or(-1);
                (runs.get(&c[1]).unwrap_or(&empty), pid, Some(format!("{}#{pid}", &c[1])))
            }));
        }
        cited.retain(|(source, pid, _)| source.contains_key(pid));
        let Some(first) = cited.first() else {
            continue; // not tied to a post that exists (an unknown #id is reported on its own)
        };
        let ok = cited.iter().any(|(source, pid, _)| quote_matches(quote, &post_sources(*pid, source)));
        let mut cite = Citation::new(Kind::Quote, *a, *b, ok);
        cite.post_id = Some(first.1);
        cite.quote = Some(tellfold_models::text::py_strip(quote).to_string());
        cite.reference = first.2.clone();
        found.push(cite);
    }
    found
}

/// Checks every #id, @handle and quoted fragment in the report against the run's posts and agents.
pub fn audit_citations<'a>(
    report: &Report,
    posts: &IndexMap<i64, Post>,
    handles: impl IntoIterator<Item = &'a str>,
) -> CitationAudit {
    let handles: HashSet<String> = handles.into_iter().map(str::to_lowercase).collect();
    audit_texts(&report_texts(report), posts, &handles, None)
}

/// The audit of any cited prose: `handles` are lowercased; `runs` checks "A#12" refs in a study comparison.
pub fn audit_texts(
    texts: &[String],
    posts: &IndexMap<i64, Post>,
    handles: &HashSet<String>,
    runs: Option<&RunPosts>,
) -> CitationAudit {
    let found: Vec<Citation> = texts.iter().flat_map(|t| citations(t, posts, handles, runs)).collect();
    let mut audit = CitationAudit { checked: found.len() as i64, ..CitationAudit::default() };
    for c in found.into_iter().filter(|c| !c.ok) {
        match c.kind {
            Kind::Post => {
                let pid = c.post_id.expect("post id");
                if !audit.unknown_posts.contains(&pid) {
                    audit.unknown_posts.push(pid);
                }
            }
            Kind::Ref => {
                let r = c.reference.expect("ref");
                if !audit.unknown_refs.contains(&r) {
                    audit.unknown_refs.push(r);
                }
            }
            Kind::Handle => {
                let h = c.handle.expect("handle");
                if !audit.unknown_handles.contains(&h) {
                    audit.unknown_handles.push(h);
                }
            }
            Kind::Quote => {
                let q = Misquote {
                    post_id: c.post_id.expect("post id"),
                    quote: c.quote.expect("quote"),
                    r#ref: c.reference,
                };
                if !audit.misquotes.iter().any(|m| m.post_id == q.post_id && m.r#ref == q.r#ref && m.quote == q.quote) {
                    audit.misquotes.push(q);
                }
            }
        }
    }
    audit
}

/// `text` with every citation that does not match replaced by [unverified], and how many were replaced.
pub fn mark_unverified_texts(
    text: &str,
    posts: &IndexMap<i64, Post>,
    handles: &HashSet<String>,
    runs: Option<&RunPosts>,
) -> (String, usize) {
    let mut bad: Vec<Citation> = citations(text, posts, handles, runs).into_iter().filter(|c| !c.ok).collect();
    bad.sort_by_key(|c| std::cmp::Reverse(c.start));
    let mut out = text.to_string();
    for c in &bad {
        out = format!("{}{UNVERIFIED}{}", &out[..c.start], &out[c.end..]);
    }
    (out, bad.len())
}

/// Whether every citation in `text` matches.
pub(crate) fn all_ok(text: &str, posts: &IndexMap<i64, Post>, handles: &HashSet<String>) -> bool {
    citations(text, posts, handles, None).iter().all(|c| c.ok)
}
