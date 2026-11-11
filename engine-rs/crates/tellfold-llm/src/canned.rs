//! The canned offline responder (port of `fake_responder` and `_FakeBuilder` in `tellfold/cli.py`): a
//! deterministic instance of any JSON schema, steered by what the prompt mentions. `TELLFOLD_FAKE_LLM=1` runs whole
//! simulations on it, and the pipeline tests use it.
//!
//! Agent ids (a01, a02...) and post ids (#12, post 12) found in the prompt are reused so fake decisions point at
//! real agents and posts; a field description like "one of: a, b, c" is honoured; relation endpoints reuse
//! ids/names generated earlier in the same response. Everything else is placeholder text.

use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Map, Number, Value};
use tellfold_models::json::py_round_int;
use tellfold_models::text::py_strip;

use crate::fake::FakeReply;
use crate::Schema;

const MAX_FAKE_ITEMS: usize = 12;

fn agent_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\ba\d{2,3}\b").expect("valid regex"))
}

fn post_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(?:#|\bpost[ _-]?(?:id)?\s*[:=#]?\s*)(\d+)").expect("valid regex"))
}

fn choices_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\bone of:\s*([\w\s,|/-]+)").expect("valid regex"))
}

/// Canned responder: a schema-valid value for `json()`, short text for `text()`.
pub fn fake_responder(purpose: &str, prompt: &str, schema: Option<&Schema>) -> FakeReply {
    match schema {
        None => FakeReply::Text(format!("(offline test reply for {purpose}) I would keep an eye on how people react.")),
        Some(schema) => FakeReply::Json(FakeBuilder::new(purpose, prompt, &schema.value).build()),
    }
}

fn dedupe<T: PartialEq>(items: impl IntoIterator<Item = T>) -> Vec<T> {
    let mut out: Vec<T> = Vec::new();
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

/// Builds a deterministic instance of a JSON schema.
pub struct FakeBuilder<'a> {
    purpose: String,
    root: &'a Value,
    defs: Map<String, Value>,
    agents: Vec<String>,
    post_ids: Vec<i64>,
    counter: usize,
    refs: Vec<String>,
    agent_cursor: usize,
}

impl<'a> FakeBuilder<'a> {
    pub fn new(purpose: &str, prompt: &str, schema: &'a Value) -> Self {
        let agents = dedupe(agent_re().find_iter(prompt).map(|m| m.as_str().to_string()));
        let post_refs = dedupe(post_re().captures_iter(prompt).map(|c| c[1].to_string()));
        FakeBuilder {
            purpose: purpose.into(),
            root: schema,
            defs: schema.get("$defs").and_then(Value::as_object).cloned().unwrap_or_default(),
            agents,
            post_ids: post_refs.iter().filter_map(|p| p.parse().ok()).collect(),
            counter: 0,
            refs: Vec::new(),
            agent_cursor: 0,
        }
    }

    pub fn build(mut self) -> Value {
        let root = self.root.clone();
        self.value(&root, "", None, false)
    }

    fn next(&mut self) -> usize {
        self.counter += 1;
        self.counter
    }

    fn resolve(&self, node: &Value) -> Map<String, Value> {
        let mut node = node.as_object().cloned().unwrap_or_default();
        while let Some(reference) = node.get("$ref").and_then(Value::as_str).map(str::to_string) {
            let name = reference.rsplit('/').next().unwrap_or("");
            let mut merged = self.defs.get(name).and_then(Value::as_object).cloned().unwrap_or_default();
            for (k, v) in node {
                if k != "$ref" {
                    merged.insert(k, v);
                }
            }
            node = merged;
        }
        if let Some(Value::Array(all)) = node.get("allOf") {
            if all.len() == 1 {
                return self.resolve(&all[0]);
            }
        }
        node
    }

    fn value(&mut self, node: &Value, name: &str, agent: Option<&str>, nullable: bool) -> Value {
        let node = self.resolve(node);
        if let Some(c) = node.get("const") {
            return c.clone();
        }
        if let Some(Value::Array(options)) = node.get("enum") {
            let n = self.next();
            return options[n % options.len()].clone();
        }
        for key in ["anyOf", "oneOf"] {
            if let Some(Value::Array(all)) = node.get(key) {
                let options: Vec<&Value> =
                    all.iter().filter(|o| self.resolve(o).get("type") != Some(&json!("null"))).collect();
                let Some(first) = options.first() else { return Value::Null };
                let mut merged = first.as_object().cloned().unwrap_or_default();
                if let Some(d) = node.get("description") {
                    merged.insert("description".into(), d.clone());
                }
                let nullable = options.len() < all.len();
                return self.value(&Value::Object(merged), name, agent, nullable);
            }
        }
        let mut nullable = nullable;
        let kind = match node.get("type") {
            Some(Value::Array(kinds)) => {
                nullable = nullable || kinds.contains(&json!("null"));
                kinds.iter().filter_map(Value::as_str).find(|k| *k != "null").unwrap_or("null").to_string()
            }
            Some(Value::String(k)) => k.clone(),
            _ => String::new(),
        };
        match kind.as_str() {
            "object" => self.object(&node, agent),
            "array" => self.array(&node, name, agent),
            "string" => self.string(&node, name, agent, nullable),
            "integer" | "number" => self.number(&node, name, kind == "integer", nullable),
            "boolean" => Value::Bool(true),
            _ => Value::Null,
        }
    }

    fn object(&mut self, node: &Map<String, Value>, agent: Option<&str>) -> Value {
        let props = node.get("properties").and_then(Value::as_object).cloned().unwrap_or_default();
        let taken;
        let agent = if props.contains_key("agent_id") && agent.is_none() {
            taken = self.take_agent();
            Some(taken.as_str())
        } else {
            agent
        };
        let mut out = Map::new();
        for (key, sub) in &props {
            let v = self.value(sub, key, agent, false);
            out.insert(key.clone(), v);
        }
        Value::Object(out)
    }

    fn array(&mut self, node: &Map<String, Value>, name: &str, agent: Option<&str>) -> Value {
        let items = self.resolve(node.get("items").unwrap_or(&json!({})));
        let top_level_objects = items.get("type") == Some(&json!("object")) && agent.is_none();
        let want = if top_level_objects {
            if self.agents.is_empty() {
                3
            } else {
                self.agents.len()
            }
        } else {
            2
        };
        let min = node.get("minItems").and_then(Value::as_u64).unwrap_or(0) as usize;
        let max = node.get("maxItems").and_then(Value::as_u64).map_or(MAX_FAKE_ITEMS, |m| m as usize);
        let size = min.max(want).min(max);
        let items = Value::Object(items);
        (0..size).map(|_| self.value(&items, name, agent, false)).collect()
    }

    fn take_agent(&mut self) -> String {
        if self.agents.is_empty() {
            return "a01".into();
        }
        let agent = self.agents[self.agent_cursor % self.agents.len()].clone();
        self.agent_cursor += 1;
        agent
    }

    fn string(&mut self, node: &Map<String, Value>, name: &str, agent: Option<&str>, nullable: bool) -> Value {
        let n = self.next();
        let key = name.to_lowercase();
        let description = node.get("description").and_then(Value::as_str).unwrap_or("");
        if let Some(found) = choices_re().captures(description) {
            let options: Vec<&str> = found[1].split([',', '|', '/']).map(py_strip).filter(|c| !c.is_empty()).collect();
            return json!(options[n % options.len()]);
        }
        if key == "agent_id" {
            return json!(agent.map_or_else(|| self.take_agent(), str::to_string));
        }
        if key.starts_with("target_agent") {
            let others: Vec<&String> = self.agents.iter().filter(|a| Some(a.as_str()) != agent).collect();
            return if !others.is_empty() {
                json!(others[n % others.len()])
            } else if nullable {
                Value::Null
            } else {
                json!("a01")
            };
        }
        if (key == "source" || key == "target") && !self.refs.is_empty() {
            return json!(self.refs[n % self.refs.len()]);
        }
        let text = if key.contains("handle") {
            format!("fake_user_{n}")
        } else if key == "id" || key == "slug" || key.ends_with("_id") {
            if nullable {
                return Value::Null;
            }
            let text = format!("item-{n}");
            self.refs.push(text.clone());
            text
        } else if key.contains("name") || key == "title" {
            let text = format!("Placeholder {} {n}", if key.is_empty() { "name" } else { &key });
            self.refs.push(text.clone());
            text
        } else {
            format!(
                "Canned {} #{n} from the offline test model ({}).",
                if key.is_empty() { "text" } else { &key },
                self.purpose
            )
        };
        json!(fit(text, node))
    }

    fn number(&mut self, node: &Map<String, Value>, name: &str, integer: bool, nullable: bool) -> Value {
        let n = self.next();
        if integer && name.to_lowercase().contains("post") {
            if !self.post_ids.is_empty() {
                return json!(self.post_ids[n % self.post_ids.len()]);
            }
            if nullable {
                return Value::Null;
            }
        }
        let bound = |a: &str, b: &str| node.get(a).or_else(|| node.get(b)).and_then(Value::as_number).cloned();
        let low = bound("minimum", "exclusiveMinimum");
        let high = bound("maximum", "exclusiveMaximum");
        let f = |x: &Number| x.as_f64().unwrap_or(0.0);
        // Python keeps an int where the arithmetic stays in ints (`low + 1`, `high - 1`)
        let value: Number = match (&low, &high) {
            (Some(lo), Some(hi)) => Number::from_f64(f(lo) + (f(hi) - f(lo)) * 0.6).unwrap_or(0.into()),
            (Some(lo), None) => match lo.as_i64() {
                Some(i) => (i + 1).into(),
                None => Number::from_f64(f(lo) + 1.0).unwrap_or(0.into()),
            },
            (None, Some(hi)) => {
                let below = match hi.as_i64() {
                    Some(i) => Number::from(i - 1),
                    None => Number::from_f64(f(hi) - 1.0).unwrap_or(0.into()),
                };
                if 0.5 < f(&below) {
                    Number::from_f64(0.5).expect("finite")
                } else {
                    below
                }
            }
            (None, None) => Number::from_f64(0.5).expect("finite"),
        };
        if integer {
            json!(value.as_i64().unwrap_or_else(|| py_round_int(f(&value))))
        } else {
            Value::Number(value)
        }
    }
}

fn fit(text: String, node: &Map<String, Value>) -> String {
    let mut text = text;
    let min = node.get("minLength").and_then(Value::as_u64).unwrap_or(0) as usize;
    let len = text.chars().count();
    if len < min {
        text.extend(std::iter::repeat_n('x', min - len));
    }
    match node.get("maxLength").and_then(Value::as_u64) {
        Some(max) => text.chars().take(max as usize).collect(),
        None => text,
    }
}
