//! The prompts, the judge that grades a report blind, the zero-shot baseline and their offline stand-ins.

use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::{json, Value};
use tellfold_models::json::{py_dumps, py_dumps_ascii, py_dumps_pretty};

use crate::cases::{seed_headline, Case};
use crate::process::{network_error, run_captured, tail, wait_for_network, NETWORK_TRIES};
use crate::{sentiment_direction, truthy, BACKDOWNS, DIRECTIONS};

pub fn judge_schema_base() -> Value {
    let mut backdowns: Vec<&str> = BACKDOWNS.to_vec();
    backdowns.push("unclear");
    json!({
        "type": "object",
        "properties": {
            "predicted_direction": {"type": "string", "enum": DIRECTIONS},
            "direction_evidence": {"type": "string"},
            "predicted_backdown": {"type": "string", "enum": backdowns},
            "backdown_evidence": {"type": "string"},
            "objections": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "verdict": {"type": "string", "enum": ["covered", "partial", "missed"]},
                        "evidence": {"type": "string"},
                    },
                    "required": ["id", "verdict", "evidence"],
                },
            },
            "groups": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {"group": {"type": "string"}, "named": {"type": "boolean"}},
                    "required": ["group", "named"],
                },
            },
            "extra_objections": {"type": "array", "items": {"type": "string"}},
        },
        "required": ["predicted_direction", "predicted_backdown", "objections", "groups"],
    })
}

pub fn zero_shot_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "summary": {"type": "string"},
            "prediction": {"type": "string"},
            "confidence": {"type": "string", "enum": ["low", "medium", "high"]},
            "camps": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {"name": {"type": "string"}, "summary": {"type": "string"}},
                    "required": ["name", "summary"],
                },
            },
            "risks": {"type": "array", "items": {"type": "string"}},
        },
        "required": ["summary", "prediction", "confidence", "camps", "risks"],
    })
}

static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^## (System|User)[ \t]*$").expect("pattern"));
static GROUPS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\{\{#groups\}\}(.*?)\{\{/groups\}\}").expect("pattern"));

/// The `## System` and `## User` sections of a prompt file.
pub fn split_prompt(path: &Path) -> Result<(String, String), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    split_prompt_text(&text).ok_or_else(|| {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        format!("{name} needs exactly one '## System' and one '## User' heading")
    })
}

fn split_prompt_text(text: &str) -> Option<(String, String)> {
    // headings only: the files' introductions mention "## System" and "## User" in running text too
    let headings: Vec<_> = HEADING.captures_iter(text).collect();
    if headings.len() != 2 {
        return None;
    }
    let section = |i: usize| {
        let start = headings[i].get(0).expect("match").end();
        let end = headings.get(i + 1).map_or(text.len(), |h| h.get(0).expect("match").start());
        (headings[i][1].to_string(), text[start..end].trim().to_string())
    };
    let (a, b) = (section(0), section(1));
    match (a.0.as_str(), b.0.as_str()) {
        ("System", "User") => Some((a.1, b.1)),
        ("User", "System") => Some((b.1, a.1)),
        _ => None,
    }
}

/// The first JSON object in a reply, ignoring text before and after it.
pub fn extract_json(text: &str) -> Result<Value, String> {
    let start = text.find('{').ok_or("no JSON object in the reply")?;
    let mut values = serde_json::Deserializer::from_str(&text[start..]).into_iter::<Value>();
    match values.next() {
        Some(Ok(value)) => Ok(value),
        Some(Err(e)) => Err(e.to_string()),
        None => Err("no JSON object in the reply".into()),
    }
}

/// Calls and tokens one model or judge used.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    pub calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
}

impl Usage {
    pub fn to_json(&self) -> Value {
        json!({"calls": self.calls, "input_tokens": self.input_tokens, "output_tokens": self.output_tokens,
               "cost_usd": self.cost_usd})
    }
}

/// One structured call: the zero-shot answer, or what [`LlmJudge`] asks.
pub trait Model {
    fn call(&mut self, system: &str, user: &str, schema: &Value) -> Result<Value, String>;
    fn usage(&self) -> Usage;
}

/// Grades one report (or zero-shot answer) against a case.
pub trait Judge {
    fn grade(&mut self, case: &Case, report: &Value, final_sentiment: Option<f64>) -> Result<Value, String>;
    fn usage(&self) -> Usage;
}

/// One structured call through the local Claude Code CLI, with the same isolation flags the engine uses.
pub struct ClaudeModel {
    pub cli: String,
    pub model: String,
    pub timeout: Duration,
    pub usage: Usage,
}

impl ClaudeModel {
    pub fn new(cli: &str, model: &str) -> Self {
        Self::with_timeout(cli, model, Duration::from_secs(600))
    }

    pub fn with_timeout(cli: &str, model: &str, timeout: Duration) -> Self {
        Self { cli: cli.into(), model: model.into(), timeout, usage: Usage::default() }
    }

    fn count(&mut self, reply: &Value) {
        let usage = reply.get("usage").filter(|u| truthy(u)).cloned().unwrap_or(json!({}));
        let int = |k: &str| usage.get(k).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))).unwrap_or(0);
        self.usage.calls += 1;
        self.usage.input_tokens +=
            int("input_tokens") + int("cache_read_input_tokens") + int("cache_creation_input_tokens");
        self.usage.output_tokens += int("output_tokens");
        self.usage.cost_usd += reply.get("total_cost_usd").and_then(Value::as_f64).unwrap_or(0.0);
    }

    /// The structured output of one reply, or why it is not usable.
    fn parse(&mut self, stdout: &str, schema: &Value) -> Result<Value, String> {
        let reply: Value = serde_json::from_str(stdout).map_err(|e| e.to_string())?;
        if !reply.is_object() {
            return Err(format!("reply is a {}, not an object", type_name(&reply)));
        }
        if truthy(reply.get("is_error").unwrap_or(&Value::Null)) {
            let result = tellfold_models::json::py_str(reply.get("result").unwrap_or(&Value::Null));
            return Err(crate::process::head(&result, 300));
        }
        self.count(&reply);
        let data = match reply.get("structured_output").filter(|d| truthy(d)) {
            Some(data) => data.clone(),
            None => extract_json(reply.get("result").and_then(Value::as_str).unwrap_or(""))?,
        };
        let Some(object) = data.as_object() else {
            return Err(format!("structured output is a {}, not an object", type_name(&data)));
        };
        // the free-text fallback is not checked against the schema by the CLI
        let missing: Vec<&str> = schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|k| !object.contains_key(*k))
            .collect();
        if !missing.is_empty() {
            return Err(format!("reply has no {}", missing.join(", ")));
        }
        Ok(data)
    }
}

fn type_name(v: &Value) -> &'static str {
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

impl Model for ClaudeModel {
    fn call(&mut self, system: &str, user: &str, schema: &Value) -> Result<Value, String> {
        let argv: Vec<String> = [
            self.cli.as_str(), "-p", "--output-format", "json", "--system-prompt", system, "--tools", "",
            "--max-turns", "3", "--model", &self.model, "--strict-mcp-config", "--setting-sources=",
            "--no-session-persistence", "--json-schema", &py_dumps(schema),
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut last_error = String::new();
        let (mut tries, mut waits) = (2, 0);
        while tries > 0 {
            tries -= 1;
            let Some(out) = run_captured(&argv, user, self.timeout)? else {
                last_error = format!("no reply within {:.0}s", self.timeout.as_secs_f64());
                continue;
            };
            if out.code != 0
                && network_error(&format!("{}{}", tail(&out.stdout, 2000), tail(&out.stderr, 2000)))
                && waits < NETWORK_TRIES
            {
                // a dropped network is not the model's failure: wait for it and try again without using up a try
                waits += 1;
                tries += 1;
                wait_for_network(Duration::from_secs(60));
                continue;
            }
            match self.parse(&out.stdout, schema) {
                Ok(data) => return Ok(data),
                Err(e) => last_error = format!("{e}; stderr: {}", tail(&out.stderr, 300)),
            }
        }
        Err(format!("claude call failed ({}): {last_error}", self.model))
    }

    fn usage(&self) -> Usage {
        self.usage.clone()
    }
}

const STOPWORDS: &str = "the and for that this with from have will they their them are was were been being into about \
than then there what when which while would could should also only more most some such very just over under because \
people";

static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[a-z][a-z'-]{3,}").expect("pattern"));

pub fn words(text: &str) -> std::collections::HashSet<String> {
    let stop: std::collections::HashSet<&str> = STOPWORDS.split_whitespace().collect();
    let lower = text.to_lowercase();
    WORD.find_iter(&lower).map(|m| m.as_str()).filter(|w| !stop.contains(w)).map(String::from).collect()
}

/// Offline stand-in for the judge in `--dry-run`: word overlap, no model. Its grades mean nothing; it only lets the
/// harness run end to end for free.
#[derive(Default)]
pub struct DryJudge {
    pub usage: Usage,
}

impl Judge for DryJudge {
    fn grade(&mut self, case: &Case, report: &Value, final_sentiment: Option<f64>) -> Result<Value, String> {
        self.usage.calls += 1;
        let text = words(&py_dumps_ascii(report));
        let mut objections = Vec::new();
        for o in case.r("objections").as_array().into_iter().flatten() {
            let want = words(o["text"].as_str().unwrap_or(""));
            let share = want.intersection(&text).count() as f64 / want.len().max(1) as f64;
            let verdict = if share >= 0.6 {
                "covered"
            } else if share >= 0.3 {
                "partial"
            } else {
                "missed"
            };
            objections.push(json!({"id": o["id"], "verdict": verdict, "evidence": ""}));
        }
        let groups: Vec<Value> = case
            .r("pushback_groups")
            .as_array()
            .into_iter()
            .flatten()
            .map(|g| {
                let named = words(g.as_str().unwrap_or("")).iter().any(|w| text.contains(w));
                json!({"group": g, "named": named})
            })
            .collect();
        Ok(json!({
            "predicted_direction": sentiment_direction(final_sentiment).unwrap_or("mixed"),
            "direction_evidence": "(dry run: read from the last round's sentiment)",
            "predicted_backdown": "unclear",
            "backdown_evidence": "",
            "objections": objections,
            "groups": groups,
            "extra_objections": [],
        }))
    }

    fn usage(&self) -> Usage {
        self.usage.clone()
    }
}

/// Offline stand-in for the zero-shot call in `--dry-run`: a fixed, schema-valid answer.
#[derive(Default)]
pub struct DryModel {
    pub usage: Usage,
}

impl Model for DryModel {
    fn call(&mut self, _system: &str, _user: &str, _schema: &Value) -> Result<Value, String> {
        self.usage.calls += 1;
        Ok(json!({
            "summary": "(offline test reply) Reactions will be mixed.",
            "prediction": "(offline test reply) Most people accept it after some grumbling.",
            "confidence": "low",
            "camps": [{"name": "Accept", "summary": "Fine with it."}, {"name": "Object", "summary": "Against it."}],
            "risks": ["(offline test reply) Some people object to the price."],
        }))
    }

    fn usage(&self) -> Usage {
        self.usage.clone()
    }
}

/// The real judge: one model call with `bench/judge_prompt.md`.
pub struct LlmJudge<M: Model> {
    pub model: M,
    system: String,
    template: String,
}

impl<M: Model> LlmJudge<M> {
    pub fn new(model: M, prompt: &Path) -> Result<Self, String> {
        let (system, template) = split_prompt(prompt)?;
        Ok(Self { model, system, template })
    }
}

impl<M: Model> Judge for LlmJudge<M> {
    fn grade(&mut self, case: &Case, report: &Value, _final_sentiment: Option<f64>) -> Result<Value, String> {
        let has_groups = truthy(case.r("pushback_groups"));
        let system = render_groups(&self.system, has_groups);
        let user = render_groups(&self.template, has_groups)
            .replace("{{CHECKLIST}}", &py_dumps_pretty(&judge_checklist(case)))
            .replace("{{REPORT}}", &py_dumps_pretty(&report_for_judge(report)));
        self.model.call(&system, &user, &judge_schema(&case.rubric))
    }

    fn usage(&self) -> Usage {
        self.model.usage()
    }
}

/// Keeps the prompt's `{{#groups}}...{{/groups}}` text for a case with pushback groups and cuts it otherwise.
pub fn render_groups(text: &str, has_groups: bool) -> String {
    GROUPS.replace_all(text, if has_groups { "$1" } else { "" }).into_owned()
}

/// What the judge may see of a case. Never the true direction or backdown: the judge classifies what the report
/// predicts blind, and `score` compares that with the rubric. A case without pushback groups has no groups key at
/// all, so the judge cannot read "nobody pushed back" from an empty list.
pub fn judge_checklist(case: &Case) -> Value {
    let objections: Vec<Value> = case
        .r("objections")
        .as_array()
        .into_iter()
        .flatten()
        .map(|o| json!({"id": o["id"], "text": o["text"]}))
        .collect();
    let mut shown = json!({
        "announcement": seed_headline(case),
        "question": case.r("question"),
        "objections": objections,
    });
    if truthy(case.r("pushback_groups")) {
        shown["pushback_groups"] = case.r("pushback_groups").clone();
    }
    shown
}

/// The judge's schema with this case's objection ids and group names as enums, so the reply can be matched exactly.
/// Without pushback groups the schema has no groups field, like the checklist and the prompt.
pub fn judge_schema(rubric: &Value) -> Value {
    let mut schema = judge_schema_base();
    let ids: Vec<Value> = rubric["objections"].as_array().into_iter().flatten().map(|o| o["id"].clone()).collect();
    let props = &mut schema["properties"];
    props["objections"]["items"]["properties"]["id"]["enum"] = Value::Array(ids);
    let groups = rubric.get("pushback_groups").cloned().unwrap_or(json!([]));
    if truthy(&groups) {
        props["groups"]["items"]["properties"]["group"]["enum"] = groups;
    } else {
        props.as_object_mut().expect("object").shift_remove("groups");
        let required = schema["required"].as_array_mut().expect("array");
        required.retain(|k| k != "groups");
    }
    schema
}

/// The parts of a report that carry its forecast; ids, citations audit and persona lists are left out.
pub fn report_for_judge(report: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for key in ["title", "summary", "prediction", "confidence", "reasons", "risks", "caveats"] {
        if let Some(v) = report.get(key) {
            out.insert(key.into(), v.clone());
        }
    }
    let list = |key: &str| report.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
    let field = |v: &Value, k: &str| v.get(k).cloned().unwrap_or(Value::Null);
    out.insert(
        "camps".into(),
        list("camps").iter().map(|c| json!({"name": field(c, "name"), "summary": field(c, "summary")})).collect(),
    );
    out.insert(
        "sections".into(),
        list("sections").iter().map(|s| json!({"title": field(s, "title"), "body": field(s, "body_markdown")})).collect(),
    );
    out.insert(
        "recommendations".into(),
        list("recommendations")
            .iter()
            .map(|r| {
                let kept = r.as_object().into_iter().flatten().filter(|(k, _)| {
                    matches!(k.as_str(), "action" | "title" | "why" | "expected_effect")
                });
                Value::Object(kept.map(|(k, v)| (k.clone(), v.clone())).collect())
            })
            .collect(),
    );
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_prompt_needs_both_headings_once() {
        let (system, user) = split_prompt_text("intro mentions ## System inline\n## System\n s \n## User\nu\n").unwrap();
        assert_eq!((system.as_str(), user.as_str()), ("s", "u"));
        assert!(split_prompt_text("## System\na\n## System\nb\n").is_none());
        assert!(split_prompt_text("## System\na\n").is_none());
    }
}
