//! Port of `tests/test_personas.py`.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};
use tellfold_engine::personas::{generate_personas, plan_slots, unique_handle};
use tellfold_llm::fake::{FakeLlm, FakeReply};
use tellfold_llm::{LlmError, Schema};
use tellfold_models::{Entity, KnowledgeGraph, Relation};

const QUESTION: &str = "How will residents react to replacing bus lines with an on-demand shuttle?";

fn entity(id: &str, name: &str, kind: &str, summary: &str, chunks: &[i64]) -> Entity {
    Entity {
        id: id.into(),
        name: name.into(),
        kind: kind.into(),
        summary: summary.into(),
        source_chunk_ids: chunks.to_vec(),
    }
}

fn graph() -> KnowledgeGraph {
    KnowledgeGraph {
        entities: vec![
            entity(
                "harbor-town-council",
                "Harbor Town Council",
                "organization",
                "Voted for the shuttle pilot.",
                &[0, 1, 2],
            ),
            entity("ines-duarte", "Ines Duarte", "person", "Mayor.", &[0]),
            entity("riders-union", "Riders Union", "organization", "Opposes cuts.", &[1]),
            entity("pier-street-shop-owners", "Pier Street shop owners", "group", "Worried about footfall.", &[]),
            entity("shuttle", "On-demand shuttle", "product", "The pilot service.", &[]),
        ],
        relations: vec![Relation {
            source: "riders-union".into(),
            target: "harbor-town-council".into(),
            label: "criticizes".into(),
            evidence: String::new(),
        }],
    }
}

fn slots_in(prompt: &str) -> Vec<String> {
    prompt
        .lines()
        .filter_map(|l| l.strip_prefix("- slot "))
        .filter_map(|rest| rest.split_once(':').map(|(id, _)| id.to_string()))
        .filter(|id| id.starts_with('a') && id[1..].chars().all(|c| c.is_ascii_digit()))
        .collect()
}

fn draft(slot: &str, handle: &str, activity: f64, influence: f64) -> Value {
    json!({"slot": slot, "handle": handle, "name": format!("Person {slot}"), "bio": "bio", "background": "bg",
           "stance": "st", "traits": ["calm", " ", "a", "b", "c", "d", "e"],
           "activity": activity, "influence": influence})
}

#[test]
fn plan_mixes_entities_and_a_varied_public() {
    let g = graph();
    let slots = plan_slots(&g, QUESTION, 20, None);
    let labels: Vec<&str> = slots.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, (1..=20).map(|i| format!("a{i:02}")).collect::<Vec<_>>());
    let entity_ids: Vec<&str> = slots.iter().filter_map(|s| s.entity_id.as_deref()).collect();
    assert_eq!(entity_ids[0], "harbor-town-council"); // most connected/mentioned first
    assert_eq!(
        entity_ids.iter().copied().collect::<HashSet<_>>(),
        HashSet::from(["harbor-town-council", "ines-duarte", "riders-union"])
    ); // groups not embodied
    let public: Vec<&str> = slots.iter().filter(|s| s.entity_id.is_none()).map(|s| s.brief.as_str()).collect();
    let mut stances: HashMap<&str, usize> = HashMap::new();
    for b in &public {
        let stance = b.split("starting stance: ").nth(1).unwrap().split(';').next().unwrap();
        *stances.entry(stance).or_default() += 1;
    }
    assert!(stances.len() >= 5 && *stances.values().max().unwrap() <= 4); // no single dominant stance
    let strong: usize = stances.iter().filter(|(k, _)| k.contains("strongly")).map(|(_, n)| n).sum();
    assert!(strong <= public.len() / 3); // few extremists
    assert!(public.iter().filter(|b| b.contains("lurker")).count() >= public.len() / 3); // most are not very engaged
    assert!(public.iter().any(|b| b.contains("Pier Street shop owners")));
    assert_eq!(plan_slots(&g, QUESTION, 20, None), slots); // deterministic
}

#[test]
fn plan_without_entities_and_tiny_casts() {
    assert!(plan_slots(&KnowledgeGraph::default(), "q", 5, None).iter().all(|s| s.entity_id.is_none()));
    assert!(plan_slots(&graph(), "q", 1, None)[0].entity_id.is_none());
    assert_eq!(plan_slots(&graph(), "q", 120, None).last().unwrap().label, "a120");
}

#[test]
fn unique_handle_sanitizes_and_dedupes() {
    assert_eq!(unique_handle("@Maria.Dev", "Maria", &HashSet::from(["maria_dev".to_string()])), "maria_dev_2");
    assert_eq!(unique_handle("", "Zoë Ångström", &HashSet::new()), "zoe_angstrom");
    assert_eq!(unique_handle("", "東京", &HashSet::new()), "user");
    assert!(unique_handle(&"x".repeat(40), "", &HashSet::from(["x".repeat(20)])).len() <= 20);
}

#[tokio::test]
async fn generate_personas_in_batches_with_unique_handles_and_clamped_values() {
    let llm = FakeLlm::new(|purpose: &str, prompt: &str, schema: Option<&Schema>| -> FakeReply {
        if purpose == "personas.cast" {
            return LlmError("cast pick unavailable".into()).into(); // falls back to ranking by graph prominence
        }
        assert_eq!(purpose, "personas.batch");
        assert_eq!(schema.unwrap().name, "PersonaBatch");
        assert!(prompt.contains("Spanish") && prompt.contains(QUESTION));
        json!({"personas": slots_in(prompt).iter().map(|s| draft(s, "same_handle", 2.0, -1.0)).collect::<Vec<_>>()})
            .into()
    });
    let personas = generate_personas(&llm, &graph(), QUESTION, 23, "sonnet", "Spanish").await.unwrap();
    let batches: Vec<String> =
        llm.calls.lock().unwrap().iter().filter(|c| c.0 == "personas.batch").map(|c| c.2.clone()).collect();
    assert_eq!(batches.len(), 3); // batches of <= 10
    assert!(batches.iter().all(|p| slots_in(p).len() <= 10));
    assert!(batches[1].contains("@same_handle")); // later batches see the handles already taken
    let ids: Vec<&str> = personas.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, (1..=23).map(|i| format!("a{i:02}")).collect::<Vec<_>>());
    assert_eq!(personas.iter().map(|p| &p.handle).collect::<HashSet<_>>().len(), 23);
    assert!(personas.iter().all(|p| (0.0..=1.0).contains(&p.activity) && (0.0..=1.0).contains(&p.influence)));
    assert!(personas.iter().all(|p| p.traits.len() <= 5 && !p.traits.contains(&" ".to_string())));
    assert_eq!(personas[0].entity_id.as_deref(), Some("harbor-town-council"));
    assert_eq!(personas.iter().filter(|p| p.entity_id.is_some()).count(), 3);
    let public: Vec<_> = personas.iter().filter(|p| p.entity_id.is_none()).collect();
    assert!(public.iter().map(|p| p.activity).fold(0.0, f64::max) <= 0.95); // clamped into each slot's band
    assert!(public.iter().any(|p| p.activity <= 0.3));
}

#[tokio::test]
async fn generate_personas_retries_missing_and_failed_batches() {
    let calls = AtomicUsize::new(0);
    let llm = FakeLlm::new(move |purpose: &str, prompt: &str, _: Option<&Schema>| -> FakeReply {
        if purpose == "personas.cast" {
            return LlmError("cast pick unavailable".into()).into();
        }
        let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
        if n == 1 {
            return LlmError("rate limited".into()).into();
        }
        let slots = slots_in(prompt);
        if n == 2 {
            // returns only half, with wrong slot ids -> matched positionally
            let drafts: Vec<Value> = (0..slots.len() / 2).map(|i| draft("zz", &format!("h{i}"), 2.0, -1.0)).collect();
            return json!({"personas": drafts}).into();
        }
        json!({"personas": slots.iter().map(|s| draft(s, &format!("x_{s}"), 0.5, 0.4)).collect::<Vec<_>>()}).into()
    });
    let personas = generate_personas(&llm, &graph(), QUESTION, 8, "m", "English").await.unwrap();
    let ids: Vec<&str> = personas.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, (1..=8).map(|i| format!("a{i:02}")).collect::<Vec<_>>());
    let first: HashSet<&str> = personas[..4].iter().map(|p| p.handle.as_str()).collect();
    assert_eq!(first, HashSet::from(["h0", "h1", "h2", "h3"]));
}

#[tokio::test]
async fn generate_personas_raises_when_nothing_comes_back() {
    let down = FakeLlm::new(|_: &str, _: &str, _: Option<&Schema>| LlmError("down".into()));
    let err = generate_personas(&down, &graph(), "q", 4, "m", "English").await.unwrap_err();
    assert_eq!(err.0, "persona generation produced no personas: down");
    let empty = FakeLlm::new(|_: &str, _: &str, _: Option<&Schema>| json!({}));
    assert!(generate_personas(&empty, &graph(), "q", 0, "m", "English").await.unwrap().is_empty());
}

#[tokio::test]
async fn planner_picks_which_entities_get_a_voice() {
    let g = graph();
    let candidates: Vec<String> = g
        .entities
        .iter()
        .filter(|e| ["organization", "person"].contains(&e.kind.as_str()))
        .map(|e| e.id.clone())
        .collect();
    let chosen = candidates.last().unwrap().clone();
    let (want, all) = (chosen.clone(), candidates.clone());
    let llm = FakeLlm::new(move |purpose: &str, prompt: &str, _: Option<&Schema>| -> Value {
        if purpose == "personas.cast" {
            assert!(all.iter().all(|c| prompt.contains(c.as_str())));
            return json!({"entity_ids": [want, "not-a-real-id", want]});
        }
        json!({"personas": slots_in(prompt).iter().map(|s| draft(s, "same_handle", 2.0, -1.0)).collect::<Vec<_>>()})
    });
    let personas = generate_personas(&llm, &g, QUESTION, 10, "sonnet", "English").await.unwrap();
    let voices: Vec<&str> = personas.iter().filter_map(|p| p.entity_id.as_deref()).collect();
    assert_eq!(voices, [chosen.as_str()]);
}

fn run<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(future)
}

#[test]
fn fair_cast_records_camps_and_source() {
    use tellfold_engine::personas::{generate_cast, CastPlan, CAMPS, SOURCE_CAST};
    let llm = FakeLlm::new(tellfold_llm::canned::fake_responder);
    let fair = run(generate_cast(
        &llm,
        &graph(),
        QUESTION,
        10,
        "sonnet",
        "English",
        CastPlan { fair: true, ..Default::default() },
    ))
    .unwrap();
    assert_eq!(fair.len(), 10);
    for p in &fair {
        assert_eq!(p.source.as_deref(), Some(SOURCE_CAST));
        // entity voices have no planned camp; everyone else does
        assert_eq!(p.camp.is_some(), p.entity_id.is_none(), "{p:?}");
        assert!(p.camp.as_deref().is_none_or(|c| CAMPS.contains(&c)));
    }
    let legacy = run(generate_personas(&llm, &graph(), QUESTION, 10, "sonnet", "English")).unwrap();
    assert!(legacy.iter().all(|p| p.camp.is_none() && p.source.is_none()));
    let text = tellfold_models::json::to_string(&legacy);
    assert!(!text.contains("\"camp\"") && !text.contains("\"source\""));
}

#[test]
fn a_described_group_becomes_that_many_personas() {
    use tellfold_engine::personas::{generate_group, tidy_cast, CAMPS, SOURCE_GROUP};
    let llm = FakeLlm::new(tellfold_llm::canned::fake_responder);
    let existing = run(generate_personas(&llm, &graph(), QUESTION, 4, "sonnet", "English")).unwrap();
    let description = "three parents of Year 7 children who take the bus";
    let config = tellfold_models::SimConfig::new(QUESTION, vec![]);
    let group = run(generate_group(&llm, &graph(), &config, description, 3, &existing)).unwrap();
    assert_eq!(group.len(), 3);
    let taken: HashSet<&str> = existing.iter().map(|p| p.handle.as_str()).collect();
    for p in &group {
        assert_eq!(p.source.as_deref(), Some(SOURCE_GROUP));
        assert!(CAMPS.contains(&p.camp.as_deref().unwrap()), "{:?}", p.camp);
        assert!(!taken.contains(p.handle.as_str()));
        assert!((0.05..=0.95).contains(&p.activity));
    }
    let calls = llm.calls.lock().unwrap();
    let (purpose, model, prompt) = calls.last().unwrap();
    assert_eq!((purpose.as_str(), model.as_str()), ("personas.group", "sonnet"));
    assert!(prompt.contains(description) && prompt.contains("\"camp\": str"));
    drop(calls);
    let mut cast = existing.clone();
    cast.extend(group);
    let ids: Vec<String> = tidy_cast(cast).into_iter().map(|p| p.id).collect();
    assert_eq!(ids, ["a01", "a02", "a03", "a04", "a05", "a06", "a07"]);
    assert!(run(generate_group(&llm, &graph(), &config, "  ", 3, &existing)).is_err());
}

#[test]
fn tidy_cast_cleans_what_a_client_sends() {
    use tellfold_engine::personas::tidy_cast;
    use tellfold_models::Persona;
    let mut a = Persona::new("zz", "@Same Handle", "  Ana  ", " bio ", "bg", " for it ");
    a.activity = 7.0;
    a.influence = f64::NAN;
    a.camp = Some("strong_support".into());
    a.source = Some("pasted from a forum".into());
    a.traits = vec![" kind ".into(), "".into(), "a".into(), "b".into(), "c".into(), "d".into(), "e".into()];
    let mut b = Persona::new("a01", "same_handle", "", "", "", "");
    b.camp = Some("made_up".into());
    b.source = Some(" edited ".into());
    let tidy = tidy_cast(vec![a, b]);
    assert_eq!(tidy.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["a01", "a02"]);
    assert_eq!((tidy[0].handle.as_str(), tidy[1].handle.as_str()), ("same_handle", "same_handle_2"));
    assert_eq!((tidy[0].name.as_str(), tidy[0].bio.as_str(), tidy[0].stance.as_str()), ("Ana", "bio", "for it"));
    assert_eq!((tidy[0].activity, tidy[0].influence), (1.0, 0.5));
    assert_eq!(tidy[0].traits, ["kind", "a", "b", "c", "d"]);
    assert_eq!(tidy[1].name, "same_handle_2", "a nameless persona is named after its handle");
    assert_eq!((tidy[1].camp.as_deref(), tidy[1].source.as_deref()), (None, Some("edited")));
    assert_eq!(tidy[0].source, None, "only the known sources are kept");
    assert_eq!(tidy_cast(vec![Persona::new("a", "b", "c", "d", "e", "f"); 250]).len(), 200);
}
