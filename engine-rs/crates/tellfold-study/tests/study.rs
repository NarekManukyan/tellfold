//! Parity with `tellfold/compare.py`, `agree.py`, `branch.py` and `examples.py`: `golden/fixtures/study/study.json`
//! was written by the Python engine (`engine-rs/golden/make.py`, `study_fixtures`) on the same canned runs.

#[path = "../../tellfold-engine/tests/common/mod.rs"]
mod common;

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};

use common::fixture;
use indexmap::IndexMap;
use serde_json::{json, Value};
use tellfold_engine::runner::run_pipeline;
use tellfold_llm::canned::fake_responder;
use tellfold_llm::fake::{FakeLlm, FakeReply};
use tellfold_llm::Schema;
use tellfold_models::{CampMatches, GodEvent, Report, SimConfig};
use tellfold_report::ReportWriter;
use tellfold_store::files::copy_seeds;
use tellfold_study::agree::{bands, build_agreement, match_camps};
use tellfold_study::branch::{branch_rounds, prepare_branch, BranchOptions};
use tellfold_study::compare::{build_compare, digest_run, Member};
use tellfold_study::examples::pack_example;
use tellfold_study::Error;

fn block_on<F: Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("not a string: {v}"))
}

fn calls(llm: &FakeLlm) -> Value {
    json!(llm.calls.lock().unwrap().iter().map(|(p, m, t)| json!([p, m, t])).collect::<Vec<_>>())
}

fn seed_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../examples/mossbook_pricing.md")
}

fn replying(reply: Value) -> FakeLlm {
    FakeLlm::new(move |_: &str, _: &str, _: Option<&Schema>| -> FakeReply { FakeReply::Json(reply.clone()) })
}

/// `study_run` in make.py: a finished canned run of the mossbook seed.
fn study_run(runs: &Path, name: &str, change: impl FnOnce(&mut SimConfig)) -> (PathBuf, SimConfig) {
    let run_dir = runs.join(name);
    std::fs::create_dir_all(&run_dir).unwrap();
    let seeds = copy_seeds(&run_dir, &[seed_path()]).unwrap();
    let mut config = SimConfig::new(
        "Will buyers accept the subscription?",
        seeds.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
    );
    config.num_agents = 6;
    config.rounds = 2;
    config.batch_size = 3;
    change(&mut config);
    let llm = FakeLlm::new(fake_responder);
    block_on(run_pipeline(&llm, &run_dir, &config, &seeds, None, None, None, &ReportWriter)).unwrap().unwrap();
    (run_dir, config)
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

/// The temporary folder written as `<TMP>`, as make.py does.
fn norm(value: Value, root: &Path) -> Value {
    let text = serde_json::to_string(&value).unwrap();
    serde_json::from_str(&text.replace(&root.to_string_lossy().to_string(), "<TMP>")).unwrap()
}

fn normalized_event(name: &str, payload: &Value) -> Value {
    let mut payload = payload.clone();
    if name == "status" {
        payload["pid"] = json!(0);
        payload["started_at"] = json!("T");
        payload["updated_at"] = json!("T");
    }
    json!([name, payload])
}

fn files_under(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                out.push(path.strip_prefix(base).unwrap().to_string_lossy().into_owned());
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

fn without_created_at(mut v: Value) -> Value {
    assert!(!v["created_at"].as_str().unwrap().is_empty());
    v["created_at"] = json!("");
    v
}

#[test]
fn studies_match_python() {
    let want = fixture("study/study.json");
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let runs = root.join("runs");
    let (a, _) = study_run(&runs, "a", |_| {});
    let (b, _) = study_run(&runs, "b", |c| c.seed = 8);
    let (c, c_config) = study_run(&runs, "c", |c| {
        c.rounds = 3;
        c.events = vec![GodEvent::new(3, "The paper runs the story")];
    });

    // digests
    let digest = digest_run(&Member::new("A", "a", "Draft A", &a), "done").unwrap();
    assert_eq!(serde_json::to_value(&digest).unwrap(), want["digest"]["a"]);
    std::fs::create_dir(root.join("empty")).unwrap();
    let digest = digest_run(&Member::new("A", "e", "Draft A", &root.join("empty")), "").unwrap();
    assert_eq!(serde_json::to_value(&digest).unwrap(), want["digest"]["empty"]);

    // comparisons
    let members = [Member::new("A", "a", "Draft A", &a), Member::new("B", "b", "Draft B", &b)];
    let states: HashMap<String, String> = [("a".to_string(), "done".to_string())].into();
    for (name, case) in want["compare"].as_object().unwrap() {
        let llm = replying(case["reply"].clone());
        let result =
            block_on(build_compare(&llm, "s1", &members, s(&want["question"]), "sonnet", "German", Some(&states)))
                .unwrap();
        assert_eq!(calls(&llm), case["calls"], "{name}: calls");
        assert_eq!(without_created_at(serde_json::to_value(&result).unwrap()), case["result"], "{name}");
    }
    let one = block_on(build_compare(&replying(json!({})), "s1", &members[..1], "q", "sonnet", "English", None));
    assert!(matches!(one, Err(Error::Invalid(m)) if m == "a comparison needs at least two runs"));

    // a branch, continued, then compared with its source
    let want_branch = &want["branch"];
    assert_eq!(json!(branch_rounds(&c, false).unwrap()), want_branch["rounds"]);
    let target = runs.join("branch");
    std::fs::create_dir(&target).unwrap();
    let options = BranchOptions { study_id: Some("s1"), variant: Some("Branch at round 1"), ..Default::default() };
    let new = prepare_branch(&c, &target, &c_config, 1, &[GodEvent::new(2, "The owner apologises")], options).unwrap();
    let prepared = json!({
        "config_json": std::fs::read_to_string(target.join("config.json")).unwrap(),
        "tables": tables(&target.join("world.db")),
        "files": files_under(&target),
    });
    assert_eq!(norm(prepared, &root), want_branch["prepared"]);
    let llm = FakeLlm::new(fake_responder);
    let events: RefCell<Vec<Value>> = RefCell::new(Vec::new());
    let on_event = |name: &str, payload: &Value| events.borrow_mut().push(normalized_event(name, payload));
    let seeds: Vec<PathBuf> = new.seed_files.iter().map(PathBuf::from).collect();
    block_on(run_pipeline(&llm, &target, &new, &seeds, Some(&on_event), None, None, &ReportWriter)).unwrap();
    assert_eq!(norm(calls(&llm), &root), want_branch["calls"]);
    assert_eq!(norm(json!(events.into_inner()), &root), want_branch["events"]);
    assert_eq!(norm(tables(&target.join("world.db")), &root), want_branch["tables"]);
    assert_eq!(std::fs::read_to_string(target.join("report.json")).unwrap(), s(&want_branch["report_json"]));
    let branch_members =
        [Member::new("A", "c", "Original", &c), Member::new("B", "branch", "Branch at round 1", &target)];
    let llm = replying(want["compare"]["clean"]["reply"].clone());
    block_on(build_compare(&llm, "s1", &branch_members, s(&want["question"]), "sonnet", "English", None)).unwrap();
    assert_eq!(norm(json!(llm.calls.lock().unwrap()[0].2), &root), want_branch["compare_prompt"]);

    // branch errors
    std::fs::create_dir(runs.join("nothing")).unwrap();
    let old = runs.join("old");
    std::fs::create_dir(&old).unwrap();
    for file in ["config.json", "graph.json", "personas.json", "world.db"] {
        std::fs::copy(c.join(file), old.join(file)).unwrap();
    }
    for side in ["world.db-wal", "world.db-shm"] {
        if c.join(side).exists() {
            std::fs::copy(c.join(side), old.join(side)).unwrap();
        }
    }
    rusqlite::Connection::open(old.join("world.db")).unwrap().execute_batch("DROP TABLE memory_history").unwrap();
    assert_eq!(json!(branch_rounds(&old, false).unwrap()), want["branch_old_rounds"]);
    let cases: [(&str, &Path, i64, Vec<GodEvent>); 5] = [
        ("no world", &runs.join("nothing"), 1, vec![]),
        ("last round", &c, 3, vec![]),
        ("event outside", &c, 1, vec![GodEvent::new(4, "x")]),
        ("event at round", &c, 1, vec![GodEvent::new(1, "x")]),
        ("old run", &old, 1, vec![]),
    ];
    for (label, source, round, events) in cases {
        let dir = runs.join(format!("x-{label}"));
        std::fs::create_dir(&dir).unwrap();
        match prepare_branch(source, &dir, &c_config, round, &events, BranchOptions::default()) {
            Err(Error::Run(e)) => assert_eq!(e.0, s(&want["branch_errors"][label]), "{label}"),
            other => panic!("{label}: {other:?}"),
        }
    }

    // Check it
    let mut repeats = Vec::new();
    for case in want["repeats"].as_array().unwrap() {
        let seed = case[0].as_i64().unwrap();
        let (run_dir, _) = study_run(&runs, &format!("r{seed}"), |c| {
            c.seed = seed;
            c.num_agents = 4;
            c.batch_size = 2;
        });
        let mut report: Report =
            serde_json::from_str(&std::fs::read_to_string(run_dir.join("report.json")).unwrap()).unwrap();
        report.camps = case[1]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| {
                serde_json::from_value(
                    json!({"name": n, "members": ["a01"], "sentiment": 0.1, "summary": format!("{}!", s(n))}),
                )
                .unwrap()
            })
            .collect();
        std::fs::write(run_dir.join("report.json"), serde_json::to_string(&report).unwrap()).unwrap();
        let letter = ["A", "B", "C"][(seed - 7) as usize];
        repeats.push(Member::new(letter, &format!("r{seed}"), &format!("Run {}", seed - 6), &run_dir));
    }
    let replies = want["agreement"]["replies"].clone();
    let llm = FakeLlm::new(move |purpose: &str, _: &str, _: Option<&Schema>| FakeReply::Json(replies[purpose].clone()));
    let agreement = block_on(build_agreement(&llm, "s1", &repeats, s(&want["question"]), "sonnet", "English")).unwrap();
    assert_eq!(calls(&llm), want["agreement"]["calls"]);
    assert_eq!(without_created_at(serde_json::to_value(&agreement).unwrap()), want["agreement"]["result"]);
    let plain = [Member::new("A", "a", "Run 1", &a), Member::new("B", "b", "Run 2", &b)];
    let llm = FakeLlm::new(fake_responder);
    let agreement = block_on(build_agreement(&llm, "s2", &plain, s(&want["question"]), "haiku", "French")).unwrap();
    assert_eq!(calls(&llm), want["agreement_canned"]["calls"]);
    assert_eq!(without_created_at(serde_json::to_value(&agreement).unwrap()), want["agreement_canned"]["result"]);

    // packing an example
    let packed = root.join("examples").join("cafe");
    pack_example(&a, &packed).unwrap();
    let mut status: Value =
        serde_json::from_str(&std::fs::read_to_string(packed.join("status.json")).unwrap()).unwrap();
    status["started_at"] = json!("T");
    status["updated_at"] = json!("T");
    let conn = rusqlite::Connection::open_with_flags(
        format!("file:{}?mode=ro&immutable=1", packed.join("world.db").display()),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap();
    let journal: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
    drop(conn);
    let got = json!({
        "files": files_under(&packed),
        "config_json": std::fs::read_to_string(packed.join("config.json")).unwrap(),
        "status": status,
        "report_html": std::fs::read_to_string(packed.join("report.html")).unwrap(),
        "tables": tables(&packed.join("world.db")),
        "journal_mode": journal,
    });
    let want_pack = &want["pack"];
    for key in ["files", "config_json", "status", "report_html", "tables", "journal_mode"] {
        assert_eq!(norm(got[key].clone(), &root), want_pack[key], "pack {key}");
    }
    let mut pack_errors = serde_json::Map::new();
    let mut expect_example = |label: &str, result: Result<PathBuf, Error>| match result {
        Err(Error::Example(e)) => {
            pack_errors.insert(label.into(), json!(e));
        }
        other => panic!("{label}: {other:?}"),
    };
    expect_example("exists", pack_example(&a, &packed));
    let mut log = std::fs::OpenOptions::new().create(true).append(true).open(b.join("calls.jsonl")).unwrap();
    std::io::Write::write_all(&mut log, b"{\"purpose\": \"x\", \"prompt\": \"secret\"}\n").unwrap();
    expect_example("prompt", pack_example(&b, &root.join("examples").join("b")));
    std::fs::remove_file(b.join("report.json")).unwrap();
    expect_example("unfinished", pack_example(&b, &root.join("examples").join("b2")));
    assert_eq!(norm(Value::Object(pack_errors), &root), want["pack_errors"]);
}

#[test]
fn match_camps_and_bands_match_python() {
    let want = fixture("study/study.json");
    let matched: CampMatches = serde_json::from_value(want["match_camps"]["matched"].clone()).unwrap();
    let loaded: IndexMap<String, Vec<String>> = serde_json::from_value(want["match_camps"]["loaded"].clone()).unwrap();
    assert_eq!(serde_json::to_value(match_camps(&matched, &loaded)).unwrap(), want["match_camps"]["out"]);
    for case in want["bands"].as_array().unwrap() {
        let input: Vec<Vec<(i64, Option<f64>)>> = serde_json::from_value(case[0].clone()).unwrap();
        assert_eq!(serde_json::to_value(bands(&input)).unwrap(), case[1], "{input:?}");
    }
}
