//! The crowd rules of a new run (`SimConfig::crowd_rules`), from the first accuracy pass (docs/ACCURACY.md):
//! organisation accounts announced concessions and data the announcement never had, runs drifted to other stories,
//! and about eight of twenty people never posted. Under the rules every round shows the situation and the fact rules,
//! and everyone posts by round [`FIRST_TURN_ROUNDS`].
//!
//! The fake model behaves like a real one on these prompts: a user posts when their activity is high or when the
//! prompt gives them the floor, and otherwise scrolls past.

use std::collections::HashSet;

use serde_json::{json, Value};
use tellfold_engine::engine::{first_turns, Engine, FIRST_TURN_ROUNDS, TURN_FIRST};
use tellfold_engine::personas::tidy_cast;
use tellfold_llm::canned::fake_responder;
use tellfold_llm::fake::{FakeLlm, FakeReply};
use tellfold_llm::Schema;
use tellfold_models::{
    Entity, KnowledgeGraph, Persona, SimConfig, CROWD_RULES, QUESTION_RULES, REACTION_RULES, STAKE_RULES,
};
use tellfold_store::world::World;

/// (handle, activity): a council raising bus fares. Most of these people mostly read.
const TOWN: [(&str, f64); 8] = [
    ("council_news", 0.9),
    ("commuter_ana", 0.8),
    ("student_sam", 0.1),
    ("night_nurse", 0.08),
    ("gran_on_hill", 0.05),
    ("taxi_tom", 0.12),
    ("shop_owner", 0.1),
    ("retired_bill", 0.06),
];

fn town_cast() -> Vec<Persona> {
    let people = TOWN
        .iter()
        .map(|(handle, activity)| Persona {
            activity: *activity,
            entity_id: (*handle == "council_news").then(|| "city-council".to_string()),
            ..Persona::new("x", handle, handle, "bio", "background", "has a view on the fares")
        })
        .collect();
    tidy_cast(people)
}

fn graph() -> KnowledgeGraph {
    KnowledgeGraph {
        entities: vec![Entity {
            id: "city-council".into(),
            name: "City Council".into(),
            kind: "government".into(),
            summary: "Raises adult bus fares by 15% from January.".into(),
            source_chunk_ids: vec![],
        }],
        ..Default::default()
    }
}

/// What the council's account invents from round 3 on, as Barclays' did in the second accuracy pass.
const INVENTED: &str = "We will freeze fares for students.";

fn responder(purpose: &str, prompt: &str, schema: Option<&Schema>) -> FakeReply {
    if purpose == "engine.factcheck" {
        // the posts are numbered "[n] @handle (name): text"
        let invented: Vec<i64> = prompt
            .lines()
            .filter(|l| l.starts_with('[') && l.contains(INVENTED))
            .filter_map(|l| l[1..l.find(']')?].parse().ok())
            .collect();
        return FakeReply::Json(json!({"invented": invented}));
    }
    if purpose != "engine.round" {
        return fake_responder(purpose, prompt, schema);
    }
    let late = !(prompt.contains("Round 1 of") || prompt.contains("Round 2 of"));
    let mut decisions: Vec<Value> = Vec::new();
    for block in prompt.split("=== ").skip(1) {
        let agent = block.split_whitespace().next().unwrap_or_default();
        let speaks = block.contains("Activity level: high") || block.contains(TURN_FIRST);
        // the block's header line is "a01 @council_news ==="; its feed may show the council to others
        let council = block.lines().next().is_some_and(|l| l.contains("@council_news"));
        let action = if speaks && late && council {
            json!({"type": "post", "content": format!("{agent} on the fares. {INVENTED}")})
        } else if speaks {
            json!({"type": "post", "content": format!("{agent} on the fares.")})
        } else {
            json!({"type": "nothing"})
        };
        decisions.push(json!({"agent_id": agent, "actions": [action], "memory_note": ""}));
    }
    FakeReply::Json(json!({"decisions": decisions}))
}

struct Run {
    /// the authors of every post up to each round
    posted: Vec<HashSet<String>>,
    /// the text of every post
    texts: Vec<String>,
    /// (purpose, prompt, system) of every round, opening, fact check and sentiment call
    calls: Vec<(String, String, String)>,
    cast: Vec<Persona>,
}

/// Runs the town for `rounds` rounds under the given crowd rules.
fn simulate(rules: i64, rounds: i64) -> Run {
    let tmp = tempfile::tempdir().unwrap();
    let world = World::open(&tmp.path().join("world.db"), 7).unwrap();
    let cast = town_cast();
    world.add_personas(&cast).unwrap();
    let config = SimConfig {
        num_agents: cast.len() as i64,
        rounds,
        batch_size: 4,
        crowd_rules: rules,
        ..SimConfig::new("How will riders react to the fare rise?", vec![])
    };
    let llm = FakeLlm::new(responder);
    let graph = graph();
    let engine = Engine::new(&llm, &world, &config, None)
        .with_graph(&graph)
        .with_source("The City Council raises adult bus fares by 15% from January.");
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        engine.seed_world(&graph).await.unwrap();
        engine.run().await.unwrap();
    });
    let posts = world.posts(None).unwrap();
    let posted =
        (0..=rounds).map(|r| posts.iter().filter(|p| p.round <= r).map(|p| p.author_id.clone()).collect()).collect();
    let prompts = llm.calls.lock().unwrap().clone();
    let systems = llm.systems.lock().unwrap().clone();
    let calls = prompts
        .into_iter()
        .zip(systems)
        .filter(|(c, _)| {
            ["engine.round", "engine.seed", "engine.factcheck", "engine.sentiment"].contains(&c.0.as_str())
        })
        .map(|(c, s)| (c.0, c.2, s.1))
        .collect();
    Run { posted, texts: posts.iter().map(|p| p.content.clone()).collect(), calls, cast }
}

#[test]
fn everyone_posts_by_the_first_turn_rounds() {
    let run = simulate(CROWD_RULES, 4);
    let everyone: HashSet<String> = run.cast.iter().map(|p| p.id.clone()).collect();
    assert_eq!(run.posted[FIRST_TURN_ROUNDS as usize], everyone);
}

#[test]
fn the_original_rules_leave_lurkers_silent() {
    // what the first accuracy pass saw, on the same seed
    let run = simulate(0, 4);
    assert!(run.posted[4].len() < run.cast.len(), "{:?}", run.posted[4]);
}

#[test]
fn round_prompts_show_the_situation_only_under_the_rules() {
    for (rules, want) in [(CROWD_RULES, true), (0, false)] {
        let run = simulate(rules, 2);
        let rounds: Vec<&String> = run.calls.iter().filter(|c| c.0 == "engine.round").map(|c| &c.1).collect();
        assert!(!rounds.is_empty());
        for prompt in rounds {
            assert_eq!(prompt.contains("Situation (from the source material):"), want, "{prompt}");
            assert_eq!(prompt.contains("Raises adult bus fares by 15% from January."), want, "{prompt}");
        }
    }
}

#[test]
fn the_fact_rules_are_in_the_system_prompts_only_under_the_rules() {
    for (rules, want) in [(CROWD_RULES, true), (0, false)] {
        let run = simulate(rules, 1);
        assert!(run.calls.iter().any(|c| c.0 == "engine.seed") && run.calls.iter().any(|c| c.0 == "engine.round"));
        for (_, _, system) in run.calls.iter().filter(|c| c.0 == "engine.round" || c.0 == "engine.seed") {
            assert_eq!(system.contains("never announces anything it doesn't"), want, "{system}");
            assert_eq!(system.contains("Stay on the announcement"), want, "{system}");
        }
    }
}

#[test]
fn first_turns_give_the_floor_to_whoever_has_not_posted() {
    let cast = town_cast();
    let posted: HashSet<String> = [cast[0].id.clone()].into();
    let drawn = vec![cast[0].clone(), cast[3].clone()];
    // round 1: nobody joins; the drawn person who has not posted is given the floor
    let (extra, turns) = first_turns(&cast, &drawn, &posted, 1);
    assert!(extra.is_empty());
    assert_eq!(turns.keys().cloned().collect::<HashSet<_>>(), [cast[3].id.clone()].into());
    // the last first-turn round: everyone who has not posted joins, in id order, and all of them get the floor
    let (extra, turns) = first_turns(&cast, &drawn, &posted, FIRST_TURN_ROUNDS);
    let want: Vec<String> = cast.iter().skip(1).filter(|p| p.id != cast[3].id).map(|p| p.id.clone()).collect();
    assert_eq!(extra.iter().map(|p| p.id.clone()).collect::<Vec<_>>(), want);
    assert_eq!(turns.len(), cast.len() - 1);
    assert!(turns.values().all(|n| *n == TURN_FIRST));
}

#[test]
fn organisation_posts_that_invent_something_are_dropped_under_the_stake_rules() {
    for (rules, kept) in [(STAKE_RULES, false), (STAKE_RULES - 1, true)] {
        let run = simulate(rules, 4);
        assert_eq!(run.texts.iter().any(|t| t.contains(INVENTED)), kept, "rules {rules}");
        let checks = run.calls.iter().filter(|c| c.0 == "engine.factcheck").count();
        assert_eq!(checks > 0, rules >= STAKE_RULES, "rules {rules}");
        // the council's honest posts stay
        assert!(run.posted[4].contains(&run.cast[0].id));
    }
}

#[test]
fn the_middle_of_the_crowd_and_the_sentiment_rules_start_with_the_reaction_rules() {
    // from the balanced accuracy cases: undecided people opened with complaints, and supporters asking what comes
    // next were scored as opposition
    for (rules, want) in [(REACTION_RULES, true), (STAKE_RULES, false), (0, false)] {
        let run = simulate(rules, 2);
        let talk: Vec<&String> =
            run.calls.iter().filter(|c| c.0 == "engine.round" || c.0 == "engine.seed").map(|c| &c.2).collect();
        let scoring: Vec<&String> = run.calls.iter().filter(|c| c.0 == "engine.sentiment").map(|c| &c.2).collect();
        assert!(!talk.is_empty() && !scoring.is_empty(), "rules {rules}");
        for system in talk {
            assert_eq!(system.contains("The middle of the crowd:"), want, "rules {rules}: {system}");
            assert_eq!(system.contains("Once a user has made their point, they mostly read."), want, "rules {rules}");
        }
        for system in scoring {
            assert!(system.starts_with("You are a careful annotator"), "{system}");
            assert_eq!(system.contains("attitude to the announcement or decision itself"), want, "rules {rules}");
        }
    }
}

#[test]
fn the_crowd_sees_the_situation_instead_of_the_question_from_the_question_rules() {
    // every accuracy question asks who will push back; under version 3 the crowd still called almost no positive
    // reaction positive
    for (rules, shown) in [(QUESTION_RULES, false), (REACTION_RULES, true), (0, true)] {
        let run = simulate(rules, 2);
        let talk: Vec<&String> =
            run.calls.iter().filter(|c| c.0 == "engine.round" || c.0 == "engine.seed").map(|c| &c.1).collect();
        assert!(!talk.is_empty());
        for prompt in talk {
            assert_eq!(prompt.contains("How will riders react to the fare rise?"), shown, "rules {rules}: {prompt}");
            assert!(!prompt.starts_with('\n'), "rules {rules}: {prompt}");
        }
        // the sentiment annotator still needs the subject
        let scoring = run.calls.iter().filter(|c| c.0 == "engine.sentiment");
        assert!(scoring.into_iter().all(|c| c.1.contains("How will riders react to the fare rise?")));
    }
}
