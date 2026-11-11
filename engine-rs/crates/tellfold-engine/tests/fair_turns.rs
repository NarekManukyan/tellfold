//! Participation: in a test run of the street-closure example the two keenest supporters never posted. Their
//! slots had drawn a lurker's activity, so they were rarely in a round, and when they were the prompt called them
//! "low (mostly lurks)". `fair_turns` makes sure every camp posts and strongly held views get a turn.
//!
//! The fake model here behaves like a real one on that prompt: a user posts when their activity is high or when the
//! prompt gives them the floor, and otherwise scrolls past.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};
use tellfold_engine::engine::{fair_turns, pick_opening_authors, sample_active, Engine, TURN_STRONG, TURN_UNHEARD};
use tellfold_engine::personas::{plan_cast, plan_slots, tidy_cast, CAMPS, ENGAGEMENT_MIX};
use tellfold_llm::canned::fake_responder;
use tellfold_llm::fake::{FakeLlm, FakeReply};
use tellfold_llm::Schema;
use tellfold_models::{KnowledgeGraph, Persona, SimConfig};
use tellfold_store::world::World;

/// (handle, camp, activity, influence): a town closing its high street to cars. The two strong supporters are
/// quiet people, as their slots once planned them.
const STREET: [(&str, &str, f64, f64); 12] = [
    ("cafe_owner_jo", "strong_support", 0.12, 0.2),
    ("cyclist_priya", "strong_support", 0.1, 0.15),
    ("mum_of_two", "lean_support", 0.4, 0.2),
    ("retired_teacher", "lean_support", 0.35, 0.1),
    ("student_sam", "undecided", 0.3, 0.1),
    ("night_nurse", "undecided", 0.25, 0.1),
    ("bus_driver_ray", "indifferent", 0.1, 0.1),
    ("gran_on_hill", "indifferent", 0.08, 0.05),
    ("taxi_tom", "lean_critical", 0.45, 0.2),
    ("market_trader", "lean_critical", 0.5, 0.3),
    ("garage_owner", "strong_critical", 0.85, 0.4),
    ("motorists_assoc", "strong_critical", 0.9, 0.6),
];

fn street_cast() -> Vec<Persona> {
    let people = STREET
        .iter()
        .map(|(handle, camp, activity, influence)| Persona {
            activity: *activity,
            influence: *influence,
            camp: Some((*camp).into()),
            ..Persona::new("x", handle, handle, "bio", "background", &format!("{camp} on closing the street"))
        })
        .collect();
    tidy_cast(people)
}

/// Round decisions like a model that honours activity labels: post when the activity is high or the user is
/// given the floor, else nothing. Everything else is the canned responder.
fn responder(purpose: &str, prompt: &str, schema: Option<&Schema>) -> FakeReply {
    if purpose != "engine.round" {
        return fake_responder(purpose, prompt, schema);
    }
    let mut decisions: Vec<Value> = Vec::new();
    for block in prompt.split("=== ").skip(1) {
        let agent = block.split_whitespace().next().unwrap_or_default();
        let speaks =
            block.contains("Activity level: high") || block.contains(TURN_UNHEARD) || block.contains(TURN_STRONG);
        let action = if speaks {
            json!({"type": "post", "content": format!("{agent} says what they think about the street.")})
        } else {
            json!({"type": "nothing"})
        };
        decisions.push(json!({"agent_id": agent, "actions": [action], "memory_note": ""}));
    }
    FakeReply::Json(json!({"decisions": decisions}))
}

/// Each camp's post count, each persona's active rounds, and the cast.
type Outcome = (HashMap<String, usize>, HashMap<String, Vec<i64>>, Vec<Persona>);

/// Runs the street cast for six rounds.
fn simulate(fair: bool) -> Outcome {
    let tmp = tempfile::tempdir().unwrap();
    let world = World::open(&tmp.path().join("world.db"), 7).unwrap();
    let cast = street_cast();
    world.add_personas(&cast).unwrap();
    let config = SimConfig {
        num_agents: cast.len() as i64,
        rounds: 6,
        batch_size: 4,
        fair_turns: fair,
        ..SimConfig::new("Will the town accept closing the high street to cars?", vec![])
    };
    let llm = FakeLlm::new(responder);
    let engine = Engine::new(&llm, &world, &config, None);
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        engine.seed_world(&KnowledgeGraph::default()).await.unwrap();
        engine.run().await.unwrap();
    });
    let camp_of: HashMap<String, String> = cast.iter().map(|p| (p.id.clone(), p.camp.clone().unwrap())).collect();
    let mut posts: HashMap<String, usize> = HashMap::new();
    for post in world.posts(None).unwrap() {
        *posts.entry(camp_of[&post.author_id].clone()).or_default() += 1;
    }
    let mut active: HashMap<String, Vec<i64>> = HashMap::new();
    for (round, action) in world.actions(None).unwrap() {
        let rounds = active.entry(action.agent_id.clone()).or_default();
        if round > 0 && !rounds.contains(&round) {
            rounds.push(round);
        }
    }
    (posts, active, cast)
}

#[test]
fn legacy_turns_leave_the_keenest_supporters_silent() {
    // the bug this guards against, reproduced on the same seed
    let (posts, _, _) = simulate(false);
    assert_eq!(posts.get("strong_support"), None, "{posts:?}");
    assert!(CAMPS.iter().any(|c| !posts.contains_key(*c)));
}

#[test]
fn fair_turns_let_every_camp_post() {
    let (posts, active, cast) = simulate(true);
    for camp in CAMPS {
        assert!(posts.get(camp).copied().unwrap_or(0) >= 1, "{camp} never posted: {posts:?}");
    }
    // strongly held views, for and against, are never idle for long
    for p in cast.iter().filter(|p| p.camp.as_deref().is_some_and(|c| c.starts_with("strong_"))) {
        let rounds = active.get(&p.id).cloned().unwrap_or_default();
        assert!(rounds.len() >= 2, "{} ({:?}) acted only in {rounds:?}", p.handle, p.camp);
        let mut last = 0;
        for r in 1..=6 {
            if rounds.contains(&r) {
                last = r;
            }
            assert!(r - last <= 3, "{} was idle from round {last} to {r}", p.handle);
        }
    }
}

#[test]
fn fair_turns_only_add_to_the_draw() {
    let cast = street_cast();
    for r in 1..=6 {
        let drawn = sample_active(&cast, r, 7, false).unwrap();
        let (extra, turns) = fair_turns(&cast, &drawn, &HashSet::new(), &HashMap::new(), r);
        let drawn_ids: HashSet<&str> = drawn.iter().map(|p| p.id.as_str()).collect();
        assert!(extra.iter().all(|p| !drawn_ids.contains(p.id.as_str())), "no one twice");
        // nobody has posted, so every camp is unheard and each has someone in the round
        let mut in_round: Vec<&Persona> = drawn.iter().chain(&extra).collect();
        in_round.sort_by(|a, b| a.id.cmp(&b.id));
        for camp in CAMPS {
            assert!(in_round.iter().any(|p| p.camp.as_deref() == Some(camp)), "round {r}: {camp}");
        }
        assert!(turns.values().all(|t| *t == TURN_UNHEARD));
        assert_eq!(turns.len(), in_round.len());
    }
}

#[test]
fn heard_camps_only_bring_back_idle_strong_voices() {
    let cast = street_cast();
    let posted: HashSet<String> = cast.iter().filter(|p| p.handle != "cyclist_priya").map(|p| p.id.clone()).collect();
    let priya = cast.iter().find(|p| p.handle == "cyclist_priya").unwrap();
    let jo = cast.iter().find(|p| p.handle == "cafe_owner_jo").unwrap();
    let last: HashMap<String, i64> = cast.iter().map(|p| (p.id.clone(), 4)).collect();
    // everyone acted in round 4: nobody is due in round 5 or 6
    let (extra, _) = fair_turns(&cast, &[], &posted, &last, 6);
    assert!(extra.is_empty());
    // in round 7 the strong voices (and only they) are due, and Priya, who has not posted, gets the floor
    let (extra, turns) = fair_turns(&cast, &[], &posted, &last, 7);
    let mut camps: Vec<&str> = extra.iter().map(|p| p.camp.as_deref().unwrap()).collect();
    camps.dedup();
    assert_eq!(camps.len(), 2, "{camps:?}");
    assert!(camps.iter().all(|c| c.starts_with("strong_")));
    assert_eq!(turns.get(&priya.id), Some(&TURN_STRONG));
    assert_eq!(turns.get(&jo.id), None, "Jo has posted already");
}

#[test]
fn opening_posts_come_from_every_camp_when_fair() {
    let cast = street_cast();
    let legacy = pick_opening_authors(&cast, 7);
    let fair = tellfold_engine::engine::pick_opening_authors_fair(&cast, 7);
    assert!(legacy.iter().all(|p| fair.contains(p)), "the draw stays");
    for camp in CAMPS {
        assert!(fair.iter().any(|p| p.camp.as_deref() == Some(camp)), "{camp}");
    }
}

#[test]
fn fair_casts_never_plan_a_strong_view_as_a_lurker() {
    let graph = KnowledgeGraph::default();
    let lurker = ENGAGEMENT_MIX[0].0 .1;
    for n in [6, 12, 20, 40] {
        let question = "Will the town accept closing the high street to cars?";
        let legacy = plan_slots(&graph, question, n, Some(&[]));
        let fair = plan_cast(&graph, question, n, Some(&[]), true);
        // the same stances in the same slots
        let camps = |slots: &[tellfold_engine::personas::Slot]| -> Vec<Option<String>> {
            slots.iter().map(|s| s.camp.clone()).collect()
        };
        assert_eq!(camps(&legacy), camps(&fair));
        assert!(fair.iter().all(|s| s.camp.is_some()));
        let strong_lurkers = fair
            .iter()
            .filter(|s| s.camp.as_deref().is_some_and(|c| c.starts_with("strong_")) && s.activity_band == lurker)
            .count();
        assert_eq!(strong_lurkers, 0, "n={n}");
        // the engagement mix is the same, only reassigned
        let mut a: Vec<String> = legacy.iter().map(|s| format!("{:?}", s.activity_band)).collect();
        let mut b: Vec<String> = fair.iter().map(|s| format!("{:?}", s.activity_band)).collect();
        a.sort();
        b.sort();
        assert_eq!(a, b);
    }
}
