#![allow(dead_code)]

use std::path::{Path, PathBuf};

use rusqlite::types::ValueRef;
use serde_json::{json, Value};

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap()
}

pub fn examples_dir() -> PathBuf {
    repo_root().join("mac/Tellfold/Resources/Examples")
}

pub fn example_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(examples_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no example runs found");
    names
}

pub fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../golden/fixtures").join(name);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap()
}

/// The same queries as `TABLES` in golden/make.py.
pub const TABLES: &[(&str, &str)] = &[
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

pub fn table(db: &Path, sql: &str) -> Value {
    let conn = rusqlite::Connection::open(db).unwrap();
    let mut stmt = conn.prepare(sql).unwrap();
    let n = stmt.column_count();
    let rows = stmt
        .query_map([], |r| {
            Ok(Value::Array(
                (0..n)
                    .map(|i| match r.get_ref(i).unwrap() {
                        ValueRef::Null => Value::Null,
                        ValueRef::Integer(v) => json!(v),
                        ValueRef::Real(v) => json!(v),
                        ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                        ValueRef::Blob(_) => panic!("no blobs in world.db"),
                    })
                    .collect(),
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    Value::Array(rows)
}

pub fn tables(db: &Path) -> Value {
    let mut out = serde_json::Map::new();
    for (name, sql) in TABLES {
        out.insert((*name).into(), table(db, sql));
    }
    Value::Object(out)
}
