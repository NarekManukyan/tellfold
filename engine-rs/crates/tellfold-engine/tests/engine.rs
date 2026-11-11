//! Port of `tests/test_engine.py`.

use std::cell::RefCell;
use std::collections::HashSet;

use regex::Regex;
use serde_json::{json, Value};
use tellfold_engine::engine::{build_follow_graph, pick_opening_authors, sample_active, Engine, EventCallback};
use tellfold_llm::fake::{FakeLlm, FakeReply};
use tellfold_llm::{LlmError, Schema};
use tellfold_models::json::py_round;
use tellfold_models::{Action, ActionType, Entity, GodEvent, KnowledgeGraph, Persona, Relation, SimConfig};
use tellfold_store::world::World;

fn make_personas(n: usize) -> Vec<Persona> {
    (1..=n)
        .map(|i| Persona {
            id: format!("a{i:02}"),
            handle: format!("user{i}"),
            name: format!("User {i}"),
            bio: "bio".into(),
            background: "bg".into(),
            stance: if i % 2 == 1 { "supportive" } else { "critical" }.into(),
            traits: vec![if i % 3 != 0 { "curious" } else { "skeptical" }.into()],
            activity: 0.9,
            influence: if i <= 2 { 0.9 } else { 0.2 },
            entity_id: match i {
                1 => Some("acme".into()),
                2 => Some("gov".into()),
                _ => None,
            },
            camp: None,
            source: None,
            grounding: None,
        })
        .collect()
}

fn graph() -> KnowledgeGraph {
    let entity = |id: &str, name: &str, summary: &str| Entity {
        id: id.into(),
        name: name.into(),
        kind: "organization".into(),
        summary: summary.into(),
        source_chunk_ids: vec![],
    };
    KnowledgeGraph {
        entities: vec![entity("acme", "Acme", "Maker of widgets"), entity("gov", "Regulator", "Oversees widgets")],
        relations: vec![Relation {
            source: "gov".into(),
            target: "acme".into(),
            label: "regulates".into(),
            evidence: String::new(),
        }],
    }
}

/// Splits a round prompt into per-agent sections.
fn agent_blocks(prompt: &str) -> Vec<(String, String)> {
    let re = Regex::new(r"(?m)^=== (a\d+) @").unwrap();
    let found: Vec<_> = re.captures_iter(prompt).collect();
    found
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let start = c.get(0).unwrap().end();
            let end = found.get(i + 1).map_or(prompt.len(), |n| n.get(0).unwrap().start());
            (c[1].to_string(), prompt[start..end].to_string())
        })
        .collect()
}

fn feed_ids(block: &str) -> Vec<i64> {
    Regex::new(r"(?m)^\[#(\d+)\]").unwrap().captures_iter(block).map(|c| c[1].parse().unwrap()).collect()
}

fn responder(fail_agent: Option<&'static str>, bad_ids: bool) -> impl Fn(&str, &str, Option<&Schema>) -> FakeReply {
    move |purpose, prompt, _| {
        if purpose == "engine.seed" {
            let posts: Vec<Value> = agent_blocks(prompt)
                .iter()
                .map(|(aid, _)| json!({"agent_id": aid, "content": format!("opening from {aid}"), "memory_note": "I spoke first"}))
                .collect();
            return json!({"posts": posts}).into();
        }
        if purpose == "engine.sentiment" {
            let ids: Vec<i64> =
                Regex::new(r"(?m)^#(\d+):").unwrap().captures_iter(prompt).map(|c| c[1].parse().unwrap()).collect();
            let mut scores: Vec<Value> =
                ids.iter().map(|i| json!({"post_id": i, "score": if i % 2 == 1 { 0.5 } else { -0.1 }})).collect();
            scores.push(json!({"post_id": 99999, "score": 5}));
            return json!({"scores": scores}).into();
        }
        assert_eq!(purpose, "engine.round");
        let blocks = agent_blocks(prompt);
        if let Some(fail) = fail_agent {
            if blocks.iter().any(|(aid, _)| aid == fail) {
                return LlmError("simulated failure".into()).into();
            }
        }
        let mut decisions = Vec::new();
        for (aid, block) in &blocks {
            let feed = feed_ids(block);
            let mut actions = Vec::new();
            if bad_ids {
                actions.push(json!({"type": "like", "target_post_id": 424242}));
            }
            if let Some(first) = feed.first() {
                actions.push(json!({"type": "like", "target_post_id": format!("#{first}"), "reasoning": "agree"}));
                actions.push(json!({"type": "reply", "target_post_id": first, "content": format!("reply by {aid}")}));
            } else {
                actions.push(json!({"type": "post", "content": format!("post by {aid}")}));
            }
            actions.push(json!({"type": "dance"}));
            decisions.push(json!({"agent_id": aid, "actions": actions, "memory_note": format!("note from {aid}")}));
        }
        decisions.push(json!({"agent_id": "a99", "actions": [{"type": "post", "content": "intruder"}]}));
        json!({"decisions": decisions}).into()
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    world: World,
}

fn world() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let world = World::open(&dir.path().join("world.db"), 7).unwrap();
    world.add_personas(&make_personas(8)).unwrap();
    Fixture { _dir: dir, world }
}

fn config() -> SimConfig {
    SimConfig { rounds: 2, batch_size: 3, memory_size: 2, ..SimConfig::new("Will widgets be banned?", vec![]) }
}

type Events = RefCell<Vec<(String, Value)>>;

fn recorder(events: &Events) -> Option<EventCallback<'_>> {
    Some(Box::new(move |n: &str, p: Value| events.borrow_mut().push((n.to_string(), p))))
}

fn named<'e>(events: &'e [(String, Value)], name: &str) -> Vec<&'e Value> {
    events.iter().filter(|(n, _)| n == name).map(|(_, p)| p).collect()
}

#[tokio::test]
async fn seed_world_builds_graph_and_opening_posts() {
    let f = world();
    let events = Events::default();
    let llm = FakeLlm::new(responder(None, false));
    let cfg = SimConfig { events: vec![GodEvent::new(0, "Launch day")], ..config() };
    Engine::new(&llm, &f.world, &cfg, recorder(&events)).seed_world(&graph()).await.unwrap();

    let followers = |id: &str| f.world.followers(id).unwrap().len();
    let total: usize = f.world.personas().unwrap().iter().map(|p| followers(&p.id)).sum();
    assert!(total > 0);
    assert!(followers("a01") + followers("a02") > followers("a07") + followers("a08"));
    let openers = f.world.posts(Some(0)).unwrap();
    assert!(!openers.is_empty() && openers.iter().all(|p| p.content.starts_with("opening from")));
    assert_eq!(openers.len(), 2); // round(8 * 0.3)
    let stats = f.world.round_stats().unwrap();
    assert_eq!(stats[0].round, 0);
    assert!(stats[0].sentiment.is_some());
    assert_eq!(f.world.memory(&openers[0].author_id).unwrap().notes, ["R0: I spoke first"]);
    let actions = f.world.actions(Some(0)).unwrap();
    assert!(!actions.is_empty() && actions.iter().all(|(_, a)| a.r#type == ActionType::Post));
    let calls = llm.calls.lock().unwrap().clone();
    let seed_prompt = &calls.iter().find(|c| c.0 == "engine.seed").unwrap().2;
    assert!(seed_prompt.contains("Launch day") && seed_prompt.contains("Acme"));
    assert!(events.borrow().contains(&("stage".to_string(), json!({"stage": "follow_graph"}))));
}

#[tokio::test]
async fn run_round_loop() {
    let f = world();
    let events = Events::default();
    let llm = FakeLlm::new(responder(None, true));
    let cfg = SimConfig { events: vec![GodEvent::new(2, "Regulator announces probe")], ..config() };
    let engine = Engine::new(&llm, &f.world, &cfg, recorder(&events));
    engine.seed_world(&graph()).await.unwrap();
    let stats = engine.run().await.unwrap();
    drop(engine);

    assert_eq!(stats.iter().map(|s| s.round).collect::<Vec<_>>(), [1, 2]);
    assert!(stats.iter().all(|s| s.active_agents >= 1));
    assert!(stats[0].actions.get("nothing").copied().unwrap_or(0) > 0); // bad ids and unknown types downgraded
    assert!(stats[0].actions.get("like").copied().unwrap_or(0) > 0);
    assert_eq!(stats[0].new_posts as usize, f.world.posts(Some(1)).unwrap().len());
    assert!(f.world.actions(None).unwrap().iter().all(|(_, a)| a.agent_id != "a99"));
    let sentiment = stats[0].sentiment.unwrap();
    assert!((-1.0..=1.0).contains(&sentiment));
    // every action targets a post that existed and was in the agent's feed: no orphan counters
    assert!(f.world.posts(None).unwrap().iter().all(|p| p.likes <= 8));
    // memory trimmed to memory_size
    assert!(f.world.personas().unwrap().iter().all(|p| f.world.memory(&p.id).unwrap().notes.len() <= 2));
    let calls = llm.calls.lock().unwrap().clone();
    let round2 = &calls.iter().rfind(|c| c.0 == "engine.round").unwrap().2;
    assert!(round2.contains("BREAKING NEWS") && round2.contains("Regulator announces probe"));
    assert_eq!(f.world.events().unwrap(), [GodEvent::new(2, "Regulator announces probe")]);
    let events = events.into_inner();
    assert_eq!(named(&events, "round_start").len(), 2);
    assert_eq!(named(&events, "round_done").len(), 3); // round 0 (opening posts) too
    assert!(named(&events, "event_injected").is_empty()); // planned events are not announced as injections
    let done = named(&events, "round_done");
    assert_eq!(done.iter().map(|d| d["round"].clone()).collect::<Vec<_>>(), [json!(0), json!(1), json!(2)]);
    assert_eq!(done[1]["total_rounds"], 2);
    assert!(named(&events, "round_start")
        .iter()
        .all(|s| s["active"].as_u64().unwrap() as usize == s["active_agents"].as_array().unwrap().len()
            && s["active"].as_u64().unwrap() >= 1));
}

#[tokio::test]
async fn failed_batch_is_tolerated() {
    let f = world();
    let events = Events::default();
    let llm = FakeLlm::new(responder(Some("a03"), false));
    let cfg = config();
    let engine = Engine::new(&llm, &f.world, &cfg, recorder(&events));
    engine.seed_world(&graph()).await.unwrap();
    let stats = engine.run().await.unwrap();
    drop(engine);

    let events = events.into_inner();
    let failed = named(&events, "batch_failed");
    assert!(!failed.is_empty() && failed.iter().all(|p| p["agents"].as_array().unwrap().contains(&json!("a03"))));
    assert_eq!(stats.len(), 2);
    let actions = f.world.actions(None).unwrap();
    assert!(!actions.iter().any(|(r, a)| *r > 0 && a.agent_id == "a03"));
    assert!(actions.iter().any(|(r, _)| *r > 0)); // other batches still acted
}

#[tokio::test]
async fn only_own_feed_ids_accepted() {
    // A post id that exists but is not in the agent's feed must be rejected.
    let f = world();
    let hidden = f.world.apply(0, &Action::new("a01", ActionType::Post).content("hidden")).unwrap().unwrap();
    let llm = FakeLlm::new(move |purpose: &str, prompt: &str, _: Option<&Schema>| {
        if purpose == "engine.sentiment" {
            return json!({"scores": []});
        }
        let decisions: Vec<Value> = agent_blocks(prompt)
            .iter()
            .map(|(aid, _)| json!({"agent_id": aid, "actions": [{"type": "like", "target_post_id": hidden}]}))
            .collect();
        json!({"decisions": decisions})
    });
    let cfg = SimConfig { feed_size: 0, rounds: 1, ..config() };
    Engine::new(&llm, &f.world, &cfg, None).run().await.unwrap();
    assert_eq!(f.world.post(hidden).unwrap().likes, 0);
    assert!(f.world.actions(Some(1)).unwrap().iter().all(|(_, a)| a.r#type == ActionType::Nothing));
}

#[test]
fn sampling_is_seeded_and_never_empty() {
    let personas = make_personas(8);
    let ids = |ps: Vec<Persona>| ps.into_iter().map(|p| p.id).collect::<Vec<_>>();
    assert_eq!(
        ids(sample_active(&personas, 3, 1, false).unwrap()),
        ids(sample_active(&personas, 3, 1, false).unwrap())
    );
    let lurkers: Vec<Persona> = personas.iter().map(|p| Persona { activity: 0.0, ..p.clone() }).collect();
    assert_eq!(sample_active(&lurkers, 1, 1, false).unwrap().len(), 1);
    let mixed: Vec<Persona> = personas
        .iter()
        .enumerate()
        .map(|(i, p)| Persona { activity: if i % 2 == 1 { 0.1 } else { 1.0 }, ..p.clone() })
        .collect();
    let total: usize = (0..50).map(|r| sample_active(&mixed, r, 5, false).unwrap().len()).sum();
    let mean = total as f64 / 50.0;
    assert!((4.0..=5.5).contains(&mean));
}

#[test]
fn follow_graph_and_openers_deterministic() {
    let personas = make_personas(12);
    let g1 = build_follow_graph(&personas, &graph(), 4);
    assert_eq!(g1, build_follow_graph(&personas, &graph(), 4));
    assert!(g1.iter().all(|(f, t)| f != t));
    assert_eq!(g1.iter().collect::<HashSet<_>>().len(), g1.len());
    assert!(g1.contains(&("a02".into(), "a01".into()))); // related entity accounts follow each other with high weight
    assert_eq!(pick_opening_authors(&personas, 4), pick_opening_authors(&personas, 4));
}

#[tokio::test]
async fn feed_surfaces_unengaged_posts_and_marks_reacted_ones() {
    // An agent that always likes the top of its feed must never hit 'already reacted'; old likes are marked.
    let f = world();
    for p in f.world.personas().unwrap() {
        for j in 0..2 {
            f.world.apply(0, &Action::new(&p.id, ActionType::Post).content(&format!("post {j} by {}", p.id))).unwrap();
        }
    }
    let prompts = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen = prompts.clone();
    let llm = FakeLlm::new(move |purpose: &str, prompt: &str, _: Option<&Schema>| {
        if purpose == "engine.sentiment" {
            return json!({"scores": []});
        }
        seen.lock().unwrap().push(prompt.to_string());
        let decisions: Vec<Value> = agent_blocks(prompt)
            .iter()
            .map(|(aid, block)| json!({"agent_id": aid, "actions": [{"type": "like", "target_post_id": feed_ids(block)[0]}]}))
            .collect();
        json!({"decisions": decisions})
    });
    let cfg = SimConfig { feed_size: 50, rounds: 4, batch_size: 8, ..config() };
    Engine::new(&llm, &f.world, &cfg, None).run().await.unwrap();

    let actions = f.world.actions(None).unwrap();
    let likes = actions.iter().filter(|(r, a)| *r > 0 && a.r#type == ActionType::Like).count();
    let downgraded = actions.iter().filter(|(r, a)| *r > 0 && a.reasoning.contains("already reacted")).count();
    assert!(likes > 0 && downgraded == 0);
    assert!(prompts.lock().unwrap()[1..].iter().any(|p| p.contains("[you already liked this]")));
}

#[tokio::test]
async fn events_are_json_safe_and_posts_stream_per_batch() {
    let f = world();
    let events = Events::default();
    let llm = FakeLlm::new(responder(None, false));
    let cfg = SimConfig { batch_size: 2, ..config() };
    let engine = Engine::new(&llm, &f.world, &cfg, recorder(&events));
    engine.seed_world(&graph()).await.unwrap();
    engine.run().await.unwrap();
    drop(engine);

    let events = events.into_inner();
    let added = named(&events, "posts_added");
    assert_eq!(added[0]["round"], 0);
    let mut streamed: Vec<i64> =
        added.iter().flat_map(|p| p["post_ids"].as_array().unwrap().iter().map(|i| i.as_i64().unwrap())).collect();
    streamed.sort_unstable();
    assert_eq!(streamed, f.world.posts(None).unwrap().iter().map(|p| p.id).collect::<Vec<_>>());
    assert!(added.iter().filter(|p| p["round"] == 1).count() >= 2); // several batches each streamed their posts
}

#[tokio::test]
async fn per_post_sentiment_is_stored() {
    let f = world();
    let llm = FakeLlm::new(responder(None, false));
    let cfg = config();
    let engine = Engine::new(&llm, &f.world, &cfg, None);
    engine.seed_world(&graph()).await.unwrap();
    engine.run().await.unwrap();

    let scores = f.world.post_sentiments().unwrap();
    let posts: HashSet<i64> = f.world.posts(None).unwrap().iter().map(|p| p.id).collect();
    assert_eq!(scores.keys().copied().collect::<HashSet<_>>(), posts); // round 0 included; unknown id 99999 dropped
    assert!(scores.iter().all(|(id, v)| *v == if id % 2 == 1 { 0.5 } else { -0.1 }));
    for stats in f.world.round_stats().unwrap() {
        let ids: Vec<i64> = f.world.posts(Some(stats.round)).unwrap().iter().map(|p| p.id).collect();
        let mean = tellfold_models::json::py_sum(ids.iter().map(|i| scores[i])) / ids.len() as f64;
        assert_eq!(stats.sentiment, Some(py_round(mean, 3)));
    }
}

#[tokio::test]
async fn event_added_mid_run_is_picked_up_and_announced() {
    let f = world();
    let events = Events::default();
    let llm = FakeLlm::new(responder(None, false));
    let cfg = SimConfig { rounds: 3, events: vec![GodEvent::new(2, "Planned news")], ..config() };
    let engine = Engine::new(&llm, &f.world, &cfg, recorder(&events));
    engine.seed_world(&graph()).await.unwrap();
    engine.run_round(1).await.unwrap();
    f.world.add_event(&GodEvent::new(2, "Injected news")).unwrap();
    f.world.add_event(&GodEvent::new(3, "Later injected news")).unwrap();
    engine.run_round(2).await.unwrap();
    engine.run_round(3).await.unwrap();
    drop(engine);

    let events = events.into_inner();
    let injected: Vec<Value> = named(&events, "event_injected").into_iter().cloned().collect();
    assert_eq!(
        injected,
        [json!({"round": 2, "content": "Injected news"}), json!({"round": 3, "content": "Later injected news"})]
    );
    let calls = llm.calls.lock().unwrap().clone();
    let rounds: Vec<&String> = calls.iter().filter(|c| c.0 == "engine.round").map(|c| &c.2).collect();
    let round2 = rounds[rounds.len() - 2];
    assert!(round2.contains("Injected news") && round2.contains("Planned news"));
    let start2 = named(&events, "round_start").into_iter().find(|s| s["round"] == 2).unwrap();
    let mut news: Vec<&str> = start2["events"].as_array().unwrap().iter().map(|e| e.as_str().unwrap()).collect();
    news.sort_unstable();
    assert_eq!(news, ["Injected news", "Planned news"]);
}
