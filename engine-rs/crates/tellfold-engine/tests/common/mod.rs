#![allow(dead_code)]

use std::path::PathBuf;

use serde_json::{json, Value};
use tellfold_llm::canned::fake_responder;
use tellfold_llm::fake::FakeReply;
use tellfold_llm::{LlmError, Schema};

pub fn fixture(name: &str) -> Value {
    let path = fixture_path(name);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap()
}

pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../golden/fixtures").join(name)
}

pub fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../mac/Tellfold/Resources/Examples")
}

/// `golden_responder` in `engine-rs/golden/make.py`: the canned responder, a hand-made graph and one failing
/// round batch.
pub fn golden_responder(purpose: &str, prompt: &str, schema: Option<&Schema>) -> FakeReply {
    if purpose == "graph.extract" {
        let start = prompt.find("<chunk id=\"").expect("chunk in prompt") + 11;
        let first: i64 = prompt[start..].split('"').next().unwrap().parse().unwrap();
        return FakeReply::Json(json!({
            "entities": [
                {"name": "Acme Corporation", "kind": "company", "summary": "Acme makes widgets.",
                 "aliases": ["Acme"], "chunk_ids": [first]},
                {"name": "Jane Doe", "kind": "person", "summary": "Chief executive of Acme.", "aliases": [],
                 "chunk_ids": [first, 999]},
                {"name": format!("Topic {first}"), "kind": "concept", "summary": "", "aliases": [], "chunk_ids": []},
                {"name": "the City Council", "kind": "government", "summary": "Local regulator.",
                 "aliases": ["Council"], "chunk_ids": [first]},
                {"name": "Small landlords", "kind": "community", "summary": "Owners of a few flats.",
                 "aliases": []},
            ],
            "relations": [
                {"source": "Acme", "target": "Jane Doe", "label": "Employs ", "evidence": "Jane runs it."},
                {"source": "Council", "target": "Acme Corporation", "label": "regulates", "evidence": ""},
                {"source": "Jane Doe", "target": "nobody", "label": "x"},
            ],
        }));
    }
    if purpose == "personas.cast" {
        let mut ids: Vec<String> = prompt
            .lines()
            .filter_map(|l| l.strip_prefix("- "))
            .filter_map(|rest| {
                rest.split_once(' ').filter(|(_, tail)| tail.starts_with('[')).map(|(id, _)| id.to_string())
            })
            .collect();
        ids.reverse();
        ids.push("nope".into());
        return FakeReply::Json(json!({"entity_ids": ids}));
    }
    if purpose == "engine.round" && prompt.contains("Round 2 of") && prompt.contains("=== a04 ") {
        return FakeReply::Error(LlmError("rate limited (fake)".into()));
    }
    fake_responder(purpose, prompt, schema)
}
