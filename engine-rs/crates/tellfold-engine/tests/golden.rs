//! Parity with the Python engine: fixtures written by `engine-rs/golden/make.py`.

mod common;

use serde_json::json;
use tellfold_engine::rng::PyRandom;

#[test]
fn rng_matches_cpython() {
    for case in fixture("engine/rng.json").as_array().unwrap() {
        let seed = case["seed"].as_str().unwrap();
        let mut r = PyRandom::from_str_seed(seed);
        let floats: Vec<f64> = (0..12).map(|_| r.random()).collect();
        assert_eq!(json!(floats), case["random"], "{seed}");
        let bits: Vec<u64> = [1, 5, 31, 32, 1, 17].into_iter().map(|k| r.getrandbits(k)).collect();
        assert_eq!(json!(bits), case["bits"], "{seed}");
        let below: Vec<u64> = [1, 2, 3, 7, 100, 1_000_003].into_iter().map(|n| r.randbelow(n)).collect();
        assert_eq!(json!(below), case["below"], "{seed}");
        let mut items: Vec<i64> = (0..13).collect();
        r.shuffle(&mut items);
        assert_eq!(json!(items), case["shuffle"], "{seed}");
        let picks: Vec<usize> = (0..8).map(|_| r.choice_weighted(&[0.6, 0.01, 2.5, 0.3]).unwrap()).collect();
        assert_eq!(json!(picks), case["choices"], "{seed}");
    }
}

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use common::{examples_dir, fixture, fixture_path, golden_responder};
use serde_json::Value;
use tellfold_engine::engine::{self, AgentView, ProposedAction};
use tellfold_engine::graph::{self, Extraction};
use tellfold_engine::personas::{self, Slot};
use tellfold_engine::runner::{run_pipeline, PipelineError, ReportStage};
use tellfold_llm::canned::fake_responder;
use tellfold_llm::fake::{FakeLlm, FakeReply};
use tellfold_llm::ingest::{chunk_text_default, load_documents};
use tellfold_llm::{embedded_schema, Llm, Schema};
use tellfold_models::{AgentMemory, KnowledgeGraph, Persona, Report, SimConfig};
use tellfold_store::world::World;

fn pairs(helpers: &Value, key: &str) -> Vec<Vec<Value>> {
    helpers[key].as_array().unwrap().iter().map(|c| c.as_array().unwrap().clone()).collect()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}

#[test]
fn graph_text_helpers_match_python() {
    let helpers = fixture("engine/helpers.json");
    for case in pairs(&helpers, "slugify") {
        assert_eq!(graph::slugify(s(&case[0])), case[1], "slugify {:?}", case[0]);
    }
    for case in pairs(&helpers, "match_key") {
        assert_eq!(graph::match_key(s(&case[0])), case[1], "match_key {:?}", case[0]);
    }
    for case in pairs(&helpers, "normalize_kind") {
        assert_eq!(graph::normalize_kind(s(&case[0])), case[1], "normalize_kind {:?}", case[0]);
    }
    for case in pairs(&helpers, "cap") {
        assert_eq!(graph::cap(s(&case[0]), case[1].as_u64().unwrap() as usize), case[2], "cap {:?}", case[1]);
    }
    for case in pairs(&helpers, "clip") {
        assert_eq!(engine::clip(s(&case[0]), case[1].as_u64().unwrap() as usize), case[2]);
    }
    for case in pairs(&helpers, "agent_id") {
        let (i, t) = (case[0].as_u64().unwrap() as usize, case[1].as_u64().unwrap() as usize);
        assert_eq!(personas::agent_id(i, t), case[2]);
    }
    let taken: HashSet<String> = ["jane", "jane_2", "user", "acme_corporation_te"].map(String::from).into();
    for case in pairs(&helpers, "unique_handle") {
        assert_eq!(personas::unique_handle(s(&case[0]), s(&case[1]), &taken), case[2], "{case:?}");
    }
}

#[test]
fn merge_matches_python() {
    let merge = &fixture("engine/helpers.json")["merge"];
    let parts: Vec<Extraction> = serde_json::from_value(merge["parts"].clone()).unwrap();
    assert_eq!(serde_json::to_value(graph::merge_extractions(&parts)).unwrap(), merge["graph"]);
}

fn slot_json(slots: &[Slot]) -> Value {
    json!(slots
        .iter()
        .map(|s| json!([s.label, s.brief, s.entity_id, [s.activity_band.0, s.activity_band.1]]))
        .collect::<Vec<_>>())
}

fn ids(personas: &[Persona]) -> Value {
    json!(personas.iter().map(|p| p.id.clone()).collect::<Vec<_>>())
}

#[test]
fn seeded_helpers_match_python_on_the_examples() {
    let helpers = fixture("engine/helpers.json");
    for (name, expected) in helpers["examples"].as_object().unwrap() {
        let run = examples_dir().join(name);
        let graph: KnowledgeGraph =
            serde_json::from_str(&std::fs::read_to_string(run.join("graph.json")).unwrap()).unwrap();
        let cast: Vec<Persona> =
            serde_json::from_str(&std::fs::read_to_string(run.join("personas.json")).unwrap()).unwrap();
        let question = s(&expected["question"]);
        let mut seeds: Vec<PathBuf> =
            std::fs::read_dir(run.join("seeds")).unwrap().map(|e| e.unwrap().path()).collect();
        seeds.sort();
        let chunks = chunk_text_default(&load_documents(&seeds).unwrap());
        assert_eq!(json!(chunks.len()), expected["chunk_count"], "{name}");
        for (limit, want) in expected["select_chunks"].as_object().unwrap() {
            let got: Vec<i64> =
                graph::select_chunks(&chunks, question, limit.parse().unwrap()).iter().map(|c| c.id).collect();
            assert_eq!(json!(got), *want, "{name} select_chunks {limit}");
        }
        for (n, want) in expected["plan_slots"].as_object().unwrap() {
            assert_eq!(
                slot_json(&personas::plan_slots(&graph, question, n.parse().unwrap(), None)),
                *want,
                "{name} {n}"
            );
        }
        assert_eq!(slot_json(&personas::plan_slots(&graph, question, 6, Some(&[]))), expected["plan_slots_no_voices"]);
        assert_eq!(personas::graph_digest(&graph), expected["persona_digest"], "{name}");
        assert_eq!(engine::graph_digest(&graph, 25), expected["engine_digest"], "{name}");
        for (seed, want) in expected["follows"].as_object().unwrap() {
            let edges = engine::build_follow_graph(&cast, &graph, seed.parse().unwrap());
            assert_eq!(json!(edges), *want, "{name} follows {seed}");
        }
        for (seed, want) in expected["opening"].as_object().unwrap() {
            assert_eq!(ids(&engine::pick_opening_authors(&cast, seed.parse().unwrap())), *want, "{name} {seed}");
        }
        for (key, want) in expected["active"].as_object().unwrap() {
            let parts: Vec<&str> = key.split(':').collect();
            let active =
                engine::sample_active(&cast, parts[1].parse().unwrap(), parts[0].parse().unwrap(), parts[2] == "True")
                    .unwrap();
            assert_eq!(ids(&active), *want, "{name} active {key}");
        }
        let quiet: Vec<Persona> = cast.iter().map(|p| Persona { activity: 0.0, ..p.clone() }).collect();
        assert_eq!(ids(&engine::sample_active(&quiet, 1, 7, false).unwrap()), expected["active_quiet"]);
        for (p, want) in cast.iter().zip(expected["persona_blocks"].as_array().unwrap()) {
            let memory = AgentMemory { agent_id: p.id.clone(), notes: vec!["R0: hi".into(), "R1: yo".into()] };
            assert_eq!(engine::persona_block(p, &memory), *want);
        }
    }
}

#[test]
fn proposals_convert_like_python() {
    let helpers = fixture("engine/helpers.json");
    let mut view = AgentView::new(Persona::new("a02", "jane", "Jane", "", "", ""), Vec::new());
    view.allowed_posts.extend([3, 5]);
    view.visible_authors.extend(["a01".to_string(), "a03".to_string()]);
    let handles: HashMap<String, String> =
        [("a01", "a01"), ("a03", "a03"), ("bob", "a01"), ("carol", "a03"), ("dan", "a04")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
    for case in pairs(&helpers, "to_action") {
        let proposal: ProposedAction = serde_json::from_value(case[0].clone()).unwrap();
        let action = engine::to_action(&view, &proposal, &handles);
        assert_eq!(serde_json::to_value(action).unwrap(), case[1], "{:?}", case[0]);
    }
}

#[test]
fn canned_responder_matches_python() {
    let canned = fixture("engine/canned.json");
    let probe = s(&canned["probe"]);
    for (name, want) in canned["replies"].as_object().unwrap() {
        if name == "text" {
            assert_eq!(fake_responder("chat.interview", "x", None), FakeReply::Text(s(want).into()));
            continue;
        }
        let schema = Schema::from_value::<Value>(serde_json::from_str(embedded_schema(name).unwrap()).unwrap());
        assert_eq!(
            fake_responder("engine.round", probe, Some(&schema)),
            FakeReply::Json(want["agents"].clone()),
            "{name}"
        );
        assert_eq!(
            fake_responder("report.write", "nothing here", Some(&schema)),
            FakeReply::Json(want["plain"].clone()),
            "{name}"
        );
    }
}

// ---------- a whole run ----------

struct NoReport;

#[async_trait(?Send)]
impl ReportStage for NoReport {
    async fn write_report(
        &self,
        _: &dyn Llm,
        _: &Path,
        _: &World,
        _: &KnowledgeGraph,
        _: &SimConfig,
    ) -> Result<Report, PipelineError> {
        Ok(serde_json::from_value(json!({"title": "t", "summary": "s", "prediction": "p", "confidence": "low"}))
            .unwrap())
    }
}

fn normalized(name: &str, payload: &Value) -> Value {
    let mut payload = payload.clone();
    if name == "status" {
        payload["pid"] = json!(0);
        payload["started_at"] = json!("T");
        payload["updated_at"] = json!("T");
    }
    json!([name, payload])
}

const TABLES: &[(&str, &str)] = &[
    ("posts", "SELECT id, round, author_id, content, parent_id, repost_of, likes, dislikes, reposts, replies FROM posts ORDER BY id"),
    ("reactions", "SELECT agent_id, post_id, kind FROM reactions ORDER BY agent_id, post_id"),
    ("follows", "SELECT follower, followee FROM follows ORDER BY follower, followee"),
    ("actions", "SELECT seq, round, data FROM actions ORDER BY seq"),
    ("round_stats", "SELECT round, data FROM round_stats ORDER BY round"),
    ("events", "SELECT seq, round, content FROM events ORDER BY seq"),
    ("memories", "SELECT agent_id, notes FROM memories ORDER BY agent_id"),
    ("memory_history", "SELECT agent_id, round, notes FROM memory_history ORDER BY round, agent_id"),
    ("post_sentiment", "SELECT post_id, score FROM post_sentiment ORDER BY post_id"),
    ("sqlite_sequence", "SELECT name, seq FROM sqlite_sequence ORDER BY name"),
];

fn tables(db: &Path) -> Value {
    use rusqlite::types::ValueRef;
    let conn = rusqlite::Connection::open(db).unwrap();
    let mut out = serde_json::Map::new();
    for (name, sql) in TABLES {
        let mut stmt = conn.prepare(sql).unwrap();
        let width = stmt.column_count();
        let rows: Vec<Value> = stmt
            .query_map([], |row| {
                Ok(json!((0..width)
                    .map(|i| match row.get_ref(i).unwrap() {
                        ValueRef::Null => Value::Null,
                        ValueRef::Integer(n) => json!(n),
                        ValueRef::Real(x) => json!(x),
                        ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                        ValueRef::Blob(_) => panic!("blob"),
                    })
                    .collect::<Vec<_>>()))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        out.insert((*name).into(), json!(rows));
    }
    Value::Object(out)
}

#[test]
fn a_whole_run_matches_python() {
    let expected = fixture("engine/run/expected.json");
    let config: SimConfig = serde_json::from_value(expected["config"].clone()).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let run_dir = tmp.path().join("run");
    let llm = FakeLlm::new(golden_responder);
    let events: RefCell<Vec<Value>> = RefCell::new(Vec::new());
    let on_event = |name: &str, payload: &Value| events.borrow_mut().push(normalized(name, payload));
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let seed = fixture_path("engine/run/seed.md");
    let report = runtime
        .block_on(run_pipeline(&llm, &run_dir, &config, &[seed], Some(&on_event), None, None, &NoReport))
        .unwrap();
    assert!(report.is_some());

    let calls: Vec<Value> = llm.calls.lock().unwrap().iter().map(|(p, m, t)| json!([p, m, t])).collect();
    let want_calls = expected["calls"].as_array().unwrap();
    assert_eq!(calls.len(), want_calls.len(), "calls");
    for (i, (got, want)) in calls.iter().zip(want_calls).enumerate() {
        assert_eq!(got, want, "call {i}");
    }
    let read = |name: &str| std::fs::read_to_string(run_dir.join(name)).unwrap();
    assert_eq!(read("config.json"), s(&expected["config_json"]));
    assert_eq!(read("graph.json"), s(&expected["graph_json"]));
    assert_eq!(read("personas.json"), s(&expected["personas_json"]));
    let got_tables = tables(&run_dir.join("world.db"));
    for (name, want) in expected["tables"].as_object().unwrap() {
        assert_eq!(got_tables[name], *want, "table {name}");
    }
    let events = events.into_inner();
    let want_events = expected["events"].as_array().unwrap();
    for (i, (got, want)) in events.iter().zip(want_events).enumerate() {
        assert_eq!(got, want, "event {i}");
    }
    assert_eq!(events.len(), want_events.len(), "events");
    let mut status: Value = serde_json::from_str(&read("status.json")).unwrap();
    status["pid"] = json!(0);
    status["started_at"] = json!("T");
    status["updated_at"] = json!("T");
    assert_eq!(status, expected["status"]);
}
