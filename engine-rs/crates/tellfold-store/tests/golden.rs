//! Parity with the Python engine on real run folders: the example runs the Mac app ships, read through `World` and
//! `RunReader`, compared with what Python returns for them (fixtures from `engine-rs/golden/make.py`).

mod common;

use std::fs;

use common::*;
use serde_json::{json, Value};
use tellfold_models::{
    Action, ActionType, AgentMemory, GodEvent, KnowledgeGraph, Persona, Report, RoundStats, RunStatus, SimConfig,
};
use tellfold_store::estimate::estimate_call_split;
use tellfold_store::files::{json_list_text, json_text};
use tellfold_store::study::{default_variant, letter};
use tellfold_store::{RunReader, World};

fn to_value<T: serde::Serialize>(v: &T) -> Value {
    serde_json::from_str(&tellfold_models::json::to_string(v)).unwrap()
}

#[test]
fn example_files_are_rewritten_like_python() {
    for name in example_names() {
        let dir = examples_dir().join(&name);
        let want = fixture(&format!("examples/{name}/rewritten.json"));
        let read = |f: &str| fs::read_to_string(dir.join(f)).unwrap();
        let expect = |f: &str| want[f].as_str().unwrap().to_string();
        let config: SimConfig = serde_json::from_str(&read("config.json")).unwrap();
        assert_eq!(json_text(&config), expect("config.json"), "{name}/config.json");
        let graph: KnowledgeGraph = serde_json::from_str(&read("graph.json")).unwrap();
        assert_eq!(json_text(&graph), expect("graph.json"), "{name}/graph.json");
        let personas: Vec<Persona> = serde_json::from_str(&read("personas.json")).unwrap();
        assert_eq!(json_list_text(&personas), expect("personas.json"), "{name}/personas.json");
        let report: Report = serde_json::from_str(&read("report.json")).unwrap();
        assert_eq!(json_text(&report), expect("report.json"), "{name}/report.json");
        let status: RunStatus = serde_json::from_str(&read("status.json")).unwrap();
        assert_eq!(json_text(&status), expect("status.json"), "{name}/status.json");
    }
}

#[test]
fn reader_answers_like_python_for_the_examples() {
    for name in example_names() {
        let dir = examples_dir().join(&name);
        let reader = RunReader::new(&dir, None, true);
        let f = |file: &str| fixture(&format!("examples/{name}/{file}"));

        let mut summary = reader.summary(false).unwrap();
        summary.example = true;
        assert_eq!(to_value(&summary), f("summary.json"), "{name} summary");

        let mut detail = reader.detail(false).unwrap();
        detail.example = true;
        assert_eq!(to_value(&detail), f("detail.json"), "{name} detail");

        assert_eq!(to_value(&reader.posts(0)), f("posts.json"), "{name} posts");
        assert_eq!(to_value(&reader.actions(None)), f("actions.json"), "{name} actions");
        let last = reader.round_stats().last().unwrap().round;
        assert_eq!(
            to_value(&reader.actions(Some(last))),
            f("actions_last_round.json"),
            "{name} actions of round {last}"
        );
        assert_eq!(to_value(&reader.new_post_ids(5)), f("new_post_ids_after_5.json"), "{name} new post ids");

        let agents = f("agents.json");
        let agents = agents.as_object().unwrap();
        assert_eq!(agents.len(), reader.personas().len());
        for (id, want) in agents {
            assert_eq!(to_value(&reader.agent(id).unwrap()), *want, "{name} agent {id}");
        }
        assert!(reader.agent("nobody").is_none());
    }
}

#[test]
fn feeds_and_follows_match_python_for_the_examples() {
    for name in example_names() {
        let dir = examples_dir().join(&name);
        let want = fixture(&format!("examples/{name}/world.json"));
        let world = World::open_read_only(&dir.join("world.db"), want["seed"].as_i64().unwrap()).unwrap();
        let last = world.last_round().unwrap();
        assert_eq!(last, want["last_round"].as_i64().unwrap());
        // the examples predate memory_history, and a read-only world does not create it
        match world.memory_rounds() {
            Ok(rounds) => assert_eq!(json!(rounds), want["memory_rounds"]),
            Err(_) => assert!(want["memory_rounds"].is_null()),
        }
        let k = want["feed_size"].as_i64().unwrap();
        for (key, ids) in want["feeds"].as_object().unwrap() {
            let (agent, round) = key.split_once('@').unwrap();
            let feed: Vec<i64> =
                world.feed_for(agent, round.parse().unwrap(), k).unwrap().iter().map(|p| p.id).collect();
            assert_eq!(json!(feed), *ids, "{name} feed of {key}");
        }
        for (agent, engaged) in want["engagement"].as_object().unwrap() {
            let got: serde_json::Map<String, Value> =
                world.engagement_by(agent).unwrap().into_iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
            assert_eq!(Value::Object(got), *engaged, "{name} engagement of {agent}");
        }
        let edges: Vec<[String; 2]> = world.follow_edges().unwrap().into_iter().map(|(a, b)| [a, b]).collect();
        assert_eq!(json!(edges), want["follow_edges"]);
        for (agent, notes) in want["memories"].as_object().unwrap() {
            assert_eq!(json!(world.memory(agent).unwrap().notes), *notes, "{name} memory of {agent}");
        }
    }
}

#[test]
fn truncating_an_example_matches_python() {
    for name in example_names() {
        let want = fixture(&format!("examples/{name}/truncated.json"));
        for (round, tables_want) in want.as_object().unwrap() {
            let tmp = tempfile::tempdir().unwrap();
            let copy = tmp.path().join("world.db");
            fs::copy(examples_dir().join(&name).join("world.db"), &copy).unwrap();
            let world = World::open(&copy, 0).unwrap();
            world.truncate_after(round.parse().unwrap()).unwrap();
            world.close().unwrap();
            let got = tables(&copy);
            for (table, rows) in tables_want.as_object().unwrap() {
                assert_eq!(got[table], *rows, "{name} truncated after {round}: {table}");
            }
        }
    }
}

#[test]
fn scripted_world_is_stored_like_python() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("world.db");
    let w = World::open(&db, 3).unwrap();
    let people: Vec<Persona> = (1..=5)
        .map(|i| {
            let mut p =
                Persona::new(&format!("a{i:02}"), &format!("user{i}"), &format!("User {i}"), "bio", "bg", "neutral");
            p.traits = vec!["calm".into(), "ünïcode".into()];
            p.activity = 0.25 * i as f64;
            p.influence = 0.1 * i as f64;
            p.entity_id = (i == 1).then(|| "acme".into());
            p
        })
        .collect();
    w.add_personas(&people).unwrap();
    w.add_follow("a01", "a02").unwrap();
    let p1 = w
        .apply(0, &Action::new("a01", ActionType::Post).content("  Hello \"world\"\n line\t2  ").reasoning("opening"))
        .unwrap()
        .unwrap();
    w.apply(1, &Action::new("a02", ActionType::Reply).content("re").post(p1)).unwrap();
    w.apply(1, &Action::new("a03", ActionType::Like).post(p1)).unwrap();
    w.apply(1, &Action::new("a03", ActionType::Like).post(p1).reasoning("again")).unwrap();
    w.apply(1, &Action::new("a04", ActionType::Repost).post(p1).content(&"x".repeat(700))).unwrap();
    w.apply(1, &Action::new("a05", ActionType::Follow).agent("a01")).unwrap();
    w.apply(1, &Action::new("a05", ActionType::Follow).agent("zz")).unwrap();
    w.save_memory(&AgentMemory {
        agent_id: "a01".into(),
        notes: vec!["saw a post".into(), "änderte Meinung".into(), "quote \" and \\".into()],
    })
    .unwrap();
    w.snapshot_memories(1).unwrap();
    let mut stats = RoundStats {
        round: 1,
        new_posts: 2,
        active_agents: 3,
        sentiment: Some(0.25),
        llm_calls: 2,
        llm_seconds: 1.5,
        ..Default::default()
    };
    stats.actions.insert("post".into(), 2);
    stats.actions.insert("like".into(), 1);
    w.save_round_stats(&stats).unwrap();
    w.save_post_sentiment(&[(p1, 0.4), (2, -3.0)]).unwrap();
    w.add_event(&GodEvent::new(2, "Breaking")).unwrap();
    w.close().unwrap();
    assert_eq!(tables(&db), fixture("scripted_world.json"));
}

#[test]
fn list_files_are_laid_out_like_write_json() {
    let want = fixture("write_json.json");
    assert_eq!(json_list_text::<GodEvent>(&[]), want["empty.json"].as_str().unwrap());
    assert_eq!(json_list_text(&[GodEvent::new(1, "x")]), want["one.json"].as_str().unwrap());
}

#[test]
fn call_estimates_match_python() {
    for row in fixture("estimates.json").as_array().unwrap() {
        let mut config = SimConfig::new("q", vec![]);
        config.num_agents = row["agents"].as_i64().unwrap();
        config.rounds = row["rounds"].as_i64().unwrap();
        config.batch_size = row["batch"].as_i64().unwrap();
        config.cast_from = row["cast_from"].as_str().map(str::to_string);
        config.branched_from = row["branched_from"].as_str().map(str::to_string);
        config.branch_round = row["branch_round"].as_i64();
        let (agent, planner) = estimate_call_split(&config, row["chunks"].as_i64().unwrap());
        assert_eq!(json!([agent, planner]), row["split"], "{row}");
    }
}

#[test]
fn study_letters_match_python() {
    let want = fixture("letters.json");
    let letters: Vec<String> = (0..30).map(letter).collect();
    let variants: Vec<String> = (0..30).map(default_variant).collect();
    assert_eq!(json!(letters), want["letters"]);
    assert_eq!(json!(variants), want["variants"]);
}
