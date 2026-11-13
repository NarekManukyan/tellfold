//! The case folders under `bench/cases` and the rules every case must follow (`--validate`).

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use chrono::{Datelike, Months, NaiveDate};
use regex::Regex;
use serde_json::Value;
use tellfold_models::json::{py_repr, py_str};

use crate::{truthy, BACKDOWNS, CATEGORIES, DIRECTIONS};

/// Words that suggest input.md describes the reaction instead of the announcement. Warnings, not errors.
pub const LEAK_WORDS: &[&str] = &[
    "backlash",
    "outrage",
    "outcry",
    "u-turn",
    "reversed",
    "reversal",
    "backtrack",
    "uproar",
    "furious",
    "criticised",
    "criticized",
    "petition",
    "boycott",
    "walked back",
    "climbdown",
    "controversial",
];
/// Phrases that can only be about the benchmark itself. A seed holds only the announcement; case metadata lives in
/// rubric.json. Errors. The bare word "benchmark" only warns, since an announcement can be about a benchmark rate.
pub const META_PHRASES: &[&str] =
    &["benchmark seed", "accuracy test", "control case", "training cutoff", "known to the model"];
pub const META_WARN_WORDS: &[&str] = &["benchmark"];

/// One case folder: `input.md` (the seed), `outcome.md` and `rubric.json` (empty when missing).
#[derive(Clone, Debug)]
pub struct Case {
    pub slug: String,
    pub dir: PathBuf,
    pub input: PathBuf,
    pub rubric: Value,
}

impl Case {
    /// The rubric field, or null.
    pub fn r(&self, key: &str) -> &Value {
        self.rubric.get(key).unwrap_or(&Value::Null)
    }
}

/// Every case folder under `root` in name order, or only the slugs in `only` (an unknown slug is an error).
pub fn load_cases(only: Option<&[String]>, root: &Path) -> Result<Vec<Case>, String> {
    let mut folders: Vec<PathBuf> = std::fs::read_dir(root)
        .map_err(|e| format!("{}: {e}", root.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    folders.sort();
    let mut cases = Vec::new();
    for folder in folders {
        let slug = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if only.is_some_and(|only| !only.contains(&slug)) {
            continue;
        }
        let rubric_path = folder.join("rubric.json");
        let rubric = if rubric_path.exists() {
            let text = std::fs::read_to_string(&rubric_path).map_err(|e| format!("{}: {e}", rubric_path.display()))?;
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", rubric_path.display()))?
        } else {
            Value::Object(Default::default())
        };
        cases.push(Case { slug, input: folder.join("input.md"), dir: folder, rubric });
    }
    if let Some(only) = only {
        let mut missing: Vec<&String> = only.iter().filter(|s| !cases.iter().any(|c| &c.slug == *s)).collect();
        missing.sort();
        missing.dedup();
        if !missing.is_empty() {
            let names: Vec<&str> = missing.iter().map(|s| s.as_str()).collect();
            return Err(format!("unknown case(s): {}", names.join(", ")));
        }
    }
    Ok(cases)
}

/// `('a', 'b')`: how Python prints a tuple of strings.
fn tuple_repr(items: &[&str]) -> String {
    let parts: Vec<String> = items.iter().map(|s| py_repr(&Value::String((*s).into()))).collect();
    format!("({})", parts.join(", "))
}

fn non_empty_str(v: &Value) -> bool {
    v.as_str().is_some_and(|s| !s.is_empty())
}

fn one_of(v: &Value, allowed: &[&str]) -> bool {
    v.as_str().is_some_and(|s| allowed.contains(&s))
}

fn check_rubric_fields(r: &Value, slug: &str) -> Vec<String> {
    let get = |k: &str| r.get(k).unwrap_or(&Value::Null);
    let mut errors = Vec::new();
    if get("slug").as_str() != Some(slug) {
        errors.push(format!("rubric slug {} is not the folder name", py_repr(get("slug"))));
    }
    for key in ["title", "question", "announced_on", "model_cutoff"] {
        if !non_empty_str(get(key)) {
            errors.push(format!("rubric.{key} missing"));
        }
    }
    if !one_of(get("category"), CATEGORIES) {
        errors.push(format!("rubric.category {} not one of {}", py_repr(get("category")), tuple_repr(CATEGORIES)));
    }
    if !one_of(get("direction"), DIRECTIONS) {
        errors.push(format!("rubric.direction {} not one of {}", py_repr(get("direction")), tuple_repr(DIRECTIONS)));
    }
    errors
}

/// An ISO date, as `date.fromisoformat` reads one here.
fn iso_date(v: &Value, suffix: &str) -> Result<NaiveDate, String> {
    let text = match v {
        Value::String(s) => format!("{s}{suffix}"),
        Value::Null if suffix.is_empty() => String::new(),
        Value::Null => suffix.to_string(),
        other => return Err(format!("fromisoformat: argument must be str, not {}", py_type(other))),
    };
    NaiveDate::parse_from_str(&text, "%Y-%m-%d")
        .map_err(|_| format!("Invalid isoformat string: {}", py_repr(&Value::String(text))))
}

fn py_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// known_to_model must agree with the announcement date and the model's cutoff month.
fn check_dates(r: &Value) -> Vec<String> {
    let get = |k: &str| r.get(k).unwrap_or(&Value::Null);
    // `or ""` in Python: a missing or empty date is an empty string
    let falsy_to_null = |v: &Value| if truthy(v) { v.clone() } else { Value::Null };
    let dates = iso_date(&falsy_to_null(get("announced_on")), "")
        .and_then(|announced| iso_date(&falsy_to_null(get("model_cutoff")), "-01").map(|cutoff| (announced, cutoff)));
    let (announced, cutoff) = match dates {
        Ok(d) => d,
        Err(e) => return vec![format!("bad date: {e}")],
    };
    let next_month = cutoff.with_day(1).and_then(|d| d.checked_add_months(Months::new(1))).expect("a valid month");
    let after = announced >= next_month;
    if get("known_to_model") != &Value::Bool(!after) {
        let known = if after { "False" } else { "True" };
        return vec![format!(
            "rubric.known_to_model should be {known} for {announced} and cutoff {}",
            cutoff.format("%Y-%m")
        )];
    }
    Vec::new()
}

fn check_objections(r: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    let empty = Vec::new();
    let objections = r.get("objections").and_then(Value::as_array).unwrap_or(&empty);
    if !(3..=5).contains(&objections.len()) {
        errors.push(format!("{} objections; the rubric needs 3-5", objections.len()));
    }
    let ids: Vec<&Value> = objections.iter().map(|o| o.get("id").unwrap_or(&Value::Null)).collect();
    let unique = ids.iter().enumerate().all(|(i, id)| !ids[..i].contains(id));
    if !unique || !ids.iter().all(|id| truthy(id)) {
        errors.push("objection ids must be present and unique".into());
    }
    for o in objections {
        let id = py_str(o.get("id").unwrap_or(&Value::Null));
        if !truthy(o.get("text").unwrap_or(&Value::Null)) {
            errors.push(format!("objection {id} has no text"));
        }
        let source = o.get("source").map(py_str).unwrap_or_default();
        if !source.starts_with("http") {
            errors.push(format!("objection {id} has no source URL"));
        }
    }
    errors
}

fn check_backdown(r: &Value) -> Vec<String> {
    let null = Value::Null;
    let b = r.get("backdown").filter(|b| truthy(b)).unwrap_or(&null);
    let kind = b.get("kind").unwrap_or(&Value::Null);
    if !one_of(kind, BACKDOWNS) {
        return vec![format!("backdown.kind {} not one of {}", py_repr(kind), tuple_repr(BACKDOWNS))];
    }
    let changed = kind.as_str() != Some("none");
    if b.get("happened") != Some(&Value::Bool(changed)) {
        return vec!["backdown.happened must be true exactly when kind is partial or full".into()];
    }
    let source = b.get("source").filter(|s| truthy(s)).map(py_str).unwrap_or_default();
    if changed && !source.starts_with("http") {
        return vec!["a backdown needs a source URL".into()];
    }
    Vec::new()
}

fn word_pattern(word: &str) -> Regex {
    Regex::new(&format!(r"\b{}\b", regex::escape(word))).expect("escaped word")
}

/// The seed must be the announcement only: nothing about the benchmark, ideally nothing about the reaction.
fn check_input(path: &Path) -> (Vec<String>, Vec<String>) {
    let Ok(text) = std::fs::read_to_string(path) else { return (Vec::new(), Vec::new()) };
    let lowered = text.to_lowercase();
    let (mut errors, mut warnings) = (Vec::new(), Vec::new());
    let word_count = lowered.split_whitespace().count();
    if !(120..=700).contains(&word_count) {
        warnings.push(format!("input.md has {word_count} words (aim for 200-500)"));
    }
    let found = |words: &[&'static str]| -> Vec<&'static str> {
        words.iter().copied().filter(|w| word_pattern(w).is_match(&lowered)).collect()
    };
    let meta = found(META_PHRASES);
    if !meta.is_empty() {
        errors.push(format!("input.md mentions {}: keep case metadata in rubric.json", meta.join(", ")));
    }
    let maybe_meta = found(META_WARN_WORDS);
    if !maybe_meta.is_empty() {
        warnings.push(format!("input.md mentions {}: check it is about the announcement", maybe_meta.join(", ")));
    }
    let leaks = found(LEAK_WORDS);
    if !leaks.is_empty() {
        warnings.push(format!("input.md mentions {}: check it does not describe the reaction", leaks.join(", ")));
    }
    (errors, warnings)
}

static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https?://").expect("pattern"));

/// (errors, warnings) for one case folder.
pub fn validate_case(case: &Case) -> (Vec<String>, Vec<String>) {
    let mut errors: Vec<String> = ["input.md", "outcome.md", "rubric.json"]
        .iter()
        .filter(|name| !case.dir.join(name).exists())
        .map(|name| format!("missing {name}"))
        .collect();
    let r = &case.rubric;
    if !truthy(r) {
        return (errors, Vec::new());
    }
    errors.extend(check_rubric_fields(r, &case.slug));
    errors.extend(check_dates(r));
    errors.extend(check_objections(r));
    errors.extend(check_backdown(r));
    let mut warnings =
        if truthy(case.r("pushback_groups")) { Vec::new() } else { vec!["no pushback_groups".to_string()] };
    let (input_errors, input_warnings) = check_input(&case.input);
    errors.extend(input_errors);
    warnings.extend(input_warnings);
    if let Ok(outcome) = std::fs::read_to_string(case.dir.join("outcome.md")) {
        if URL.find_iter(&outcome).count() < 2 {
            errors.push("outcome.md cites fewer than two sources".into());
        }
    }
    (errors, warnings)
}

/// Prints every error and warning; true when no case has an error.
pub fn validate_all(cases: &[Case]) -> bool {
    let mut ok = true;
    for case in cases {
        let (errors, warnings) = validate_case(case);
        for e in &errors {
            println!("error   {}: {e}", case.slug);
        }
        for w in &warnings {
            println!("warning {}: {w}", case.slug);
        }
        ok = ok && errors.is_empty();
    }
    println!("{} cases, {}", cases.len(), if ok { "all valid" } else { "some invalid" });
    ok
}

/// The seed's own H1, written as of the day. The rubric title is written with hindsight, so the judge never sees it.
pub fn seed_headline(case: &Case) -> String {
    let text = std::fs::read_to_string(&case.input).unwrap_or_default();
    text.lines().find_map(|line| line.strip_prefix("# ")).map(|h| h.trim().to_string()).unwrap_or_default()
}
