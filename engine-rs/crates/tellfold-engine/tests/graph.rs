//! Port of `tests/test_graph.py`.

mod common;

use std::collections::HashSet;
use std::sync::Mutex;

use serde_json::json;
use tellfold_engine::graph::{
    extract_graph, match_key, merge_extractions, select_chunks, slugify, ExtractedEntity, ExtractedRelation,
    Extraction, MAX_EXTRACT_CHUNKS,
};
use tellfold_llm::fake::{FakeLlm, FakeReply};
use tellfold_llm::LlmError;
use tellfold_models::Chunk;

fn chunks() -> Vec<Chunk> {
    (0..9).map(|i| Chunk { id: i, source: "seed.md".into(), text: format!("chunk text {i}") }).collect()
}

fn chunk_ids(prompt: &str) -> Vec<i64> {
    prompt.split("<chunk id=\"").skip(1).map(|rest| rest.split('"').next().unwrap().parse().unwrap()).collect()
}

fn entity(name: &str, kind: &str, summary: &str, aliases: &[&str], chunk_ids: &[i64]) -> ExtractedEntity {
    ExtractedEntity {
        name: name.into(),
        kind: kind.into(),
        summary: summary.into(),
        aliases: aliases.iter().map(|a| a.to_string()).collect(),
        chunk_ids: chunk_ids.to_vec(),
    }
}

fn relation(source: &str, target: &str, label: &str, evidence: &str) -> ExtractedRelation {
    ExtractedRelation { source: source.into(), target: target.into(), label: label.into(), evidence: evidence.into() }
}

#[test]
fn slug_and_match_key() {
    assert_eq!(slugify("Acme Corp."), "acme-corp");
    assert_eq!(slugify("Café Société & Co"), "cafe-societe-and-co");
    assert_eq!(slugify("東京都"), "東京都");
    assert_eq!(match_key("The Acme Corporation"), "acme");
    assert_eq!(match_key("Acme Inc."), "acme");
    assert_eq!(match_key("The"), "the");
}

#[test]
fn merge_dedupes_by_name_and_alias_and_drops_dangling_relations() {
    let part1 = Extraction {
        entities: vec![
            entity("Acme Corp", "company", "Makes widgets.", &[], &[0]),
            entity("Riders Union", "group", "Transit riders.", &["RU"], &[1]),
        ],
        relations: vec![
            relation("Acme Corp", "RU", "Lobbies", "Met twice."),
            relation("Acme Corp", "Nobody Known", "sues", ""),
            relation("Acme Corp", "Acme Inc", "is", ""),
        ],
    };
    let part2 = Extraction {
        entities: vec![
            entity("ACME Inc.", "organization", "Makes widgets.", &[], &[4]),
            entity("RU", "org", "Union of bus riders.", &[], &[5]),
        ],
        relations: vec![relation("acme corp", "Riders Union", "lobbies ", "")],
    };
    let graph = merge_extractions(&[part1, part2]);
    let ids: HashSet<&str> = graph.entities.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, HashSet::from(["acme-corp", "riders-union"]));
    let acme = graph.entities.iter().find(|e| e.id == "acme-corp").unwrap();
    assert_eq!(acme.kind, "organization");
    assert_eq!(acme.source_chunk_ids, [0, 4]);
    assert_eq!(acme.summary, "Makes widgets."); // identical summaries are not repeated
    let riders = graph.entities.iter().find(|e| e.id == "riders-union").unwrap();
    assert!(riders.summary.contains("Union of bus riders."));
    let rels: Vec<_> = graph.relations.iter().map(|r| (&*r.source, &*r.target, &*r.label, &*r.evidence)).collect();
    assert_eq!(rels, [("acme-corp", "riders-union", "lobbies", "Met twice.")]);
}

#[test]
fn merge_caps_long_summaries_and_keeps_ids_unique() {
    let long = "A very detailed sentence about the org. ".repeat(40);
    let graph = merge_extractions(&[Extraction {
        entities: vec![
            entity("Big Org", "organization", &long, &[], &[]),
            entity("Big-Org!", "weird-kind", "x", &[], &[]),
            entity("   ", "person", "", &[], &[]),
        ],
        relations: vec![],
    }]);
    assert_eq!(graph.entities.len(), 1);
    assert!(graph.entities[0].summary.chars().count() <= 480);
    assert!(graph.entities[0].summary.ends_with('.'));
}

#[tokio::test]
async fn extract_graph_runs_groups_and_merges() {
    let llm = FakeLlm::new(|purpose, prompt, schema| {
        assert_eq!(purpose, "graph.extract");
        assert_eq!(schema.unwrap().name, "Extraction");
        let ids = chunk_ids(prompt);
        json!({
            "entities": [
                {"name": "Harbor Town Council", "kind": "organization", "summary": "City council.",
                 "aliases": ["the council"], "chunk_ids": [ids[0], 999]},
                {"name": format!("Group {}", ids[0]), "kind": "group", "summary": "Locals."},
            ],
            "relations": [{"source": "The Council", "target": format!("Group {}", ids[0]), "label": "consults"}],
        })
    });
    let progress = Mutex::new(Vec::<String>::new());
    let on_progress = |m: &str| progress.lock().unwrap().push(m.to_string());
    let graph =
        extract_graph(&llm, &chunks(), "Will the shuttle succeed?", "sonnet", Some(&on_progress)).await.unwrap();
    let calls = llm.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 3); // 9 chunks, 4 per call
    assert!(calls.iter().all(|(_, _, prompt)| prompt.contains("Will the shuttle succeed?")));
    let council = graph.entities.iter().find(|e| e.id == "harbor-town-council").unwrap();
    assert_eq!(council.source_chunk_ids, [0, 4, 8]); // hallucinated chunk id 999 removed
    let group0 = graph.entities.iter().find(|e| e.id == "group-0").unwrap();
    assert_eq!(group0.source_chunk_ids, [0, 1, 2, 3]); // no citation -> whole group
    assert_eq!(graph.entities[0].id, "harbor-town-council"); // most-mentioned first
    assert_eq!(graph.relations.len(), 3);
    assert_eq!(progress.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn extract_graph_tolerates_a_failed_group_but_not_all() {
    let flaky = FakeLlm::new(|_: &str, prompt: &str, _: Option<&_>| -> FakeReply {
        if chunk_ids(prompt)[0] == 4 {
            return LlmError("boom".into()).into();
        }
        json!({"entities": [{"name": "Acme", "kind": "organization", "summary": "s"}], "relations": []}).into()
    });
    let graph = extract_graph(&flaky, &chunks(), "q", "m", None).await.unwrap();
    let ids: Vec<&str> = graph.entities.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["acme"]);

    let down = FakeLlm::new(|_: &str, _: &str, _: Option<&_>| LlmError("down".into()));
    let err = extract_graph(&down, &chunks(), "q", "m", None).await.unwrap_err();
    assert_eq!(err.0, "graph extraction failed for every chunk group: down");
}

#[tokio::test]
async fn extract_graph_with_no_chunks_is_empty() {
    let llm = FakeLlm::new(|_: &str, _: &str, _: Option<&_>| json!({}));
    let graph = extract_graph(&llm, &[], "q", "m", None).await.unwrap();
    assert!(graph.entities.is_empty());
    assert!(llm.calls.lock().unwrap().is_empty());
}

#[test]
fn select_chunks_keeps_small_seed_sets_whole() {
    let chunks: Vec<Chunk> =
        (0..10).map(|i| Chunk { id: i, source: "a.md".into(), text: format!("text {i}") }).collect();
    assert_eq!(select_chunks(&chunks, "anything", MAX_EXTRACT_CHUNKS), chunks);
}

#[test]
fn select_chunks_keeps_each_documents_opening_and_the_most_relevant_chunks() {
    let mut chunks = Vec::new();
    for doc in 0..20 {
        for part in 0..30 {
            let text = if (doc, part) == (7, 12) {
                "Stripe card required before the free trial".to_string()
            } else {
                format!("filler {doc} {part}")
            };
            chunks.push(Chunk { id: chunks.len() as i64, source: format!("doc{doc}.md"), text });
        }
    }
    let picked = select_chunks(
        &chunks,
        "How will customers react if a bank card is required for the free trial?",
        MAX_EXTRACT_CHUNKS,
    );
    assert_eq!(picked.len(), MAX_EXTRACT_CHUNKS);
    let ids: Vec<i64> = picked.iter().map(|c| c.id).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted); // document order
    let openings: HashSet<String> = picked.iter().filter(|c| c.id % 30 == 0).map(|c| c.source.clone()).collect();
    assert_eq!(openings, (0..20).map(|d| format!("doc{d}.md")).collect());
    assert!(picked.iter().any(|c| c.text.contains("Stripe card")));
}
