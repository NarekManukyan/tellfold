//! Port of tests/test_world.py and the world parts of tests/test_branch.py.

mod common;

use std::path::Path;

use common::table;
use serde_json::json;
use tellfold_models::{Action, ActionType, AgentMemory, GodEvent, Persona, RoundStats};
use tellfold_store::{World, WorldError};

fn persona(i: usize) -> Persona {
    Persona::new(&format!("a{i:02}"), &format!("user{i}"), &format!("User {i}"), "bio", "bg", "neutral")
}

fn world(dir: &Path) -> World {
    let w = World::open(&dir.join("world.db"), 3).unwrap();
    w.add_personas(&(1..=5).map(persona).collect::<Vec<_>>()).unwrap();
    w
}

fn act(agent: &str, kind: ActionType) -> Action {
    Action::new(agent, kind)
}

fn ids(posts: Vec<tellfold_models::Post>) -> Vec<i64> {
    posts.into_iter().map(|p| p.id).collect()
}

#[test]
fn personas_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let world = world(tmp.path());
    let got: Vec<String> = world.personas().unwrap().into_iter().map(|p| p.id).collect();
    assert_eq!(got, ["a01", "a02", "a03", "a04", "a05"]);
    assert_eq!(world.persona("a03").unwrap().handle, "user3");
    assert!(matches!(world.persona("zz"), Err(WorldError::UnknownAgent(_))));
}

#[test]
fn post_reply_repost_counters() {
    let tmp = tempfile::tempdir().unwrap();
    let world = world(tmp.path());
    let pid = world.apply(1, &act("a01", ActionType::Post).content("hello")).unwrap().unwrap();
    let rid = world.apply(1, &act("a02", ActionType::Reply).content("hi back").post(pid)).unwrap().unwrap();
    let rp = world.apply(1, &act("a03", ActionType::Repost).post(pid)).unwrap().unwrap();
    // a repost of a repost credits the original
    let rp2 = world.apply(2, &act("a04", ActionType::Repost).post(rp)).unwrap().unwrap();
    assert_eq!(world.post(rid).unwrap().parent_id, Some(pid));
    assert_eq!(world.post(rp2).unwrap().repost_of, Some(pid));
    let original = world.post(pid).unwrap();
    assert_eq!((original.replies, original.reposts), (1, 2));
    assert_eq!(ids(world.posts(Some(1)).unwrap()), [pid, rid, rp]);
}

#[test]
fn invalid_actions_downgrade_to_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let world = world(tmp.path());
    let pid = world.apply(1, &act("a01", ActionType::Post).content("x")).unwrap().unwrap();
    assert_eq!(world.apply(1, &act("a02", ActionType::Reply).content("?").post(999)).unwrap(), None);
    assert_eq!(world.apply(1, &act("a02", ActionType::Like)).unwrap(), None);
    assert_eq!(world.apply(1, &act("a02", ActionType::Follow).agent("a02")).unwrap(), None); // self-follow
    assert_eq!(world.apply(1, &act("a02", ActionType::Follow).agent("nobody")).unwrap(), None);
    assert_eq!(world.apply(1, &act("ghost", ActionType::Post).content("boo")).unwrap(), None);
    assert_eq!(world.apply(1, &act("a01", ActionType::Like).post(pid)).unwrap(), None); // own post
    assert_eq!(world.apply(1, &act("a01", ActionType::Repost).post(pid)).unwrap(), None); // own post
    assert_eq!(world.apply(1, &act("a03", ActionType::Post).content("   ")).unwrap(), None);
    let actions = world.actions(Some(1)).unwrap();
    assert_eq!(actions[0].1.r#type, ActionType::Post);
    assert!(actions[1..].iter().all(|(_, a)| a.r#type == ActionType::Nothing));
    assert!(actions[1].1.reasoning.contains("unknown target post"));
    assert_eq!(world.posts(None).unwrap().len(), 1);
}

#[test]
fn one_reaction_per_agent_per_post() {
    let tmp = tempfile::tempdir().unwrap();
    let world = world(tmp.path());
    let pid = world.apply(1, &act("a01", ActionType::Post).content("x")).unwrap().unwrap();
    world.apply(1, &act("a02", ActionType::Like).post(pid)).unwrap();
    world.apply(1, &act("a02", ActionType::Like).post(pid)).unwrap();
    world.apply(1, &act("a02", ActionType::Dislike).post(pid)).unwrap();
    world.apply(1, &act("a03", ActionType::Dislike).post(pid)).unwrap();
    let post = world.post(pid).unwrap();
    assert_eq!((post.likes, post.dislikes), (1, 1));
}

#[test]
fn follow_graph() {
    let tmp = tempfile::tempdir().unwrap();
    let world = world(tmp.path());
    world.apply(1, &act("a01", ActionType::Follow).agent("a02")).unwrap();
    world.apply(1, &act("a01", ActionType::Follow).agent("a02")).unwrap(); // duplicate -> nothing
    assert!(world.add_follow("a03", "a02").unwrap());
    assert!(!world.add_follow("a03", "a03").unwrap());
    assert_eq!(world.following("a01").unwrap(), ["a02"]);
    assert_eq!(world.followers("a02").unwrap(), ["a01", "a03"]);
    let kinds: Vec<ActionType> = world.actions(None).unwrap().into_iter().map(|(_, a)| a.r#type).collect();
    assert_eq!(kinds, [ActionType::Follow, ActionType::Nothing]);
}

#[test]
fn feed_ranking() {
    let tmp = tempfile::tempdir().unwrap();
    let world = world(tmp.path());
    world.add_follow("a01", "a02").unwrap();
    let mine = world.apply(1, &act("a01", ActionType::Post).content("my post")).unwrap().unwrap();
    let popular = world.apply(1, &act("a03", ActionType::Post).content("popular")).unwrap().unwrap();
    let quiet = world.apply(1, &act("a04", ActionType::Post).content("quiet")).unwrap().unwrap();
    let followed = world.apply(1, &act("a02", ActionType::Post).content("from a friend")).unwrap().unwrap();
    for liker in ["a02", "a04", "a05"] {
        world.apply(1, &act(liker, ActionType::Like).post(popular)).unwrap();
    }
    let reply = world.apply(2, &act("a05", ActionType::Reply).content("re").post(mine)).unwrap().unwrap();

    let feed = ids(world.feed_for("a01", 2, 10).unwrap());
    assert!(!feed.contains(&mine));
    assert_eq!(feed[..2], [reply, followed]);
    let pos = |id| feed.iter().position(|p| *p == id).unwrap();
    assert!(pos(popular) < pos(quiet));
    assert_eq!(ids(world.feed_for("a01", 2, 2).unwrap()), [reply, followed]);
    assert_eq!(world.feed_for("a01", 2, 10).unwrap(), world.feed_for("a01", 2, 10).unwrap()); // deterministic
    assert!(!ids(world.feed_for("a01", 1, 10).unwrap()).contains(&reply)); // future posts hidden
                                                                           // a negative size slices like Python's [:k]: all but the last |k|
    assert_eq!(ids(world.feed_for("a01", 2, -2).unwrap()), feed[..feed.len() - 2]);
    assert!(world.feed_for("a01", 2, -100).unwrap().is_empty());
}

#[test]
fn feed_recency_and_dislikes() {
    let tmp = tempfile::tempdir().unwrap();
    let world = world(tmp.path());
    let old = world.apply(1, &act("a02", ActionType::Post).content("old")).unwrap().unwrap();
    let new = world.apply(3, &act("a03", ActionType::Post).content("new")).unwrap().unwrap();
    let hated = world.apply(3, &act("a04", ActionType::Post).content("hated")).unwrap().unwrap();
    for agent in ["a01", "a02", "a03"] {
        world.apply(3, &act(agent, ActionType::Dislike).post(hated)).unwrap();
    }
    let feed = ids(world.feed_for("a05", 3, 10).unwrap());
    let pos = |id| feed.iter().position(|p| *p == id).unwrap();
    assert!(pos(new) < pos(old));
    assert_eq!(*feed.last().unwrap(), hated);
}

#[test]
fn memory_stats_events_persist() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("w.db");
    let w = World::open(&path, 0).unwrap();
    w.add_personas(&[persona(1)]).unwrap();
    assert!(w.memory("a01").unwrap().notes.is_empty());
    w.save_memory(&AgentMemory { agent_id: "a01".into(), notes: vec!["saw a post".into(), "änderte Meinung".into()] })
        .unwrap();
    let mut stats = RoundStats { round: 1, new_posts: 2, sentiment: Some(0.25), ..Default::default() };
    stats.actions.insert("post".into(), 2);
    w.save_round_stats(&stats).unwrap();
    let mut stats = RoundStats { round: 1, new_posts: 3, ..Default::default() }; // upsert
    stats.actions.insert("post".into(), 3);
    w.save_round_stats(&stats).unwrap();
    w.add_event(&GodEvent::new(2, "Breaking")).unwrap();
    w.close().unwrap();

    let w2 = World::open(&path, 0).unwrap();
    assert_eq!(w2.memory("a01").unwrap().notes, ["saw a post", "änderte Meinung"]);
    assert_eq!(w2.round_stats().unwrap().iter().map(|s| s.new_posts).collect::<Vec<_>>(), [3]);
    assert_eq!(w2.events().unwrap(), [GodEvent::new(2, "Breaking")]);
    assert_eq!(w2.persona("a01").unwrap().name, "User 1");
}

#[test]
fn stale_followed_posts_do_not_bury_fresh_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let world = world(tmp.path());
    world.add_follow("a01", "a02").unwrap();
    let old_followed: Vec<i64> = (0..5)
        .map(|i| world.apply(1, &act("a02", ActionType::Post).content(&format!("old {i}"))).unwrap().unwrap())
        .collect();
    let fresh = world.apply(6, &act("a03", ActionType::Post).content("breaking")).unwrap().unwrap();
    assert_eq!(ids(world.feed_for("a01", 6, 3).unwrap())[0], fresh);
    // while still fresh, a followed post keeps its priority over a stranger's
    let recent = world.apply(6, &act("a02", ActionType::Post).content("new from a friend")).unwrap().unwrap();
    assert_eq!(ids(world.feed_for("a01", 6, 2).unwrap()), [recent, fresh]);
    let feed = ids(world.feed_for("a01", 6, 10).unwrap());
    assert!(old_followed.iter().any(|id| feed.contains(id)));
}

#[test]
fn engaged_posts_sink_and_are_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let world = world(tmp.path());
    let liked = world.apply(1, &act("a02", ActionType::Post).content("liked")).unwrap().unwrap();
    let replied = world.apply(1, &act("a03", ActionType::Post).content("replied")).unwrap().unwrap();
    let reposted = world.apply(1, &act("a04", ActionType::Post).content("reposted")).unwrap().unwrap();
    let untouched = world.apply(1, &act("a05", ActionType::Post).content("untouched")).unwrap().unwrap();
    world.apply(1, &act("a01", ActionType::Like).post(liked)).unwrap();
    world.apply(1, &act("a01", ActionType::Reply).content("hi").post(replied)).unwrap();
    world.apply(1, &act("a01", ActionType::Repost).post(reposted)).unwrap();
    world.apply(1, &act("a01", ActionType::Dislike).post(replied)).unwrap();
    let engaged: Vec<(i64, Vec<String>)> = world.engagement_by("a01").unwrap().into_iter().collect();
    let mut sorted = engaged.clone();
    sorted.sort();
    assert_eq!(
        json!(sorted),
        json!([[liked, ["liked"]], [replied, ["disliked", "replied"]], [reposted, ["reposted"]]])
    );
    let feed = ids(world.feed_for("a01", 1, 10).unwrap());
    assert_eq!(feed[0], untouched);
    let mut tail = feed[feed.len() - 3..].to_vec();
    tail.sort();
    assert_eq!(tail, [liked, replied, reposted]);
}

#[test]
fn post_sentiment_roundtrip_and_clamp() {
    let tmp = tempfile::tempdir().unwrap();
    let world = world(tmp.path());
    let a = world.apply(0, &act("a01", ActionType::Post).content("one")).unwrap().unwrap();
    let b = world.apply(1, &act("a02", ActionType::Post).content("two")).unwrap().unwrap();
    assert!(world.post_sentiments().unwrap().is_empty());
    world.save_post_sentiment(&[(a, 0.4), (b, -3.0)]).unwrap();
    world.save_post_sentiment(&[(a, 0.1)]).unwrap(); // re-scoring overwrites
    world.save_post_sentiment(&[]).unwrap();
    let got: Vec<(i64, f64)> = world.post_sentiments().unwrap().into_iter().collect();
    assert_eq!(got, [(a, 0.1), (b, -1.0)]);
}

#[test]
fn follow_edges() {
    let tmp = tempfile::tempdir().unwrap();
    let world = world(tmp.path());
    world.add_follow("a02", "a01").unwrap();
    world.add_follow("a01", "a03").unwrap();
    world.apply(1, &act("a03", ActionType::Follow).agent("a01")).unwrap();
    let edges: Vec<(String, String)> = world.follow_edges().unwrap();
    let want: Vec<(String, String)> =
        [("a01", "a03"), ("a02", "a01"), ("a03", "a01")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
    assert_eq!(edges, want);
}

#[test]
fn old_database_gains_new_tables() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("old.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TABLE posts (id INTEGER PRIMARY KEY AUTOINCREMENT, round INTEGER NOT NULL, \
         author_id TEXT NOT NULL, content TEXT NOT NULL, parent_id INTEGER, repost_of INTEGER, \
         likes INTEGER NOT NULL DEFAULT 0, dislikes INTEGER NOT NULL DEFAULT 0, \
         reposts INTEGER NOT NULL DEFAULT 0, replies INTEGER NOT NULL DEFAULT 0); \
         INSERT INTO posts (round, author_id, content) VALUES (0, 'a01', 'old post');",
    )
    .unwrap();
    drop(db);
    let w = World::open(&path, 0).unwrap();
    assert!(w.post_sentiments().unwrap().is_empty());
    assert_eq!(w.posts(None).unwrap()[0].content, "old post");
    assert!(w.memory_rounds().unwrap().is_empty());
    w.save_post_sentiment(&[(1, 0.2)]).unwrap();
    assert_eq!(w.post_sentiments().unwrap().get(&1), Some(&0.2));
}

#[test]
fn event_added_from_second_connection_is_visible() {
    let tmp = tempfile::tempdir().unwrap();
    let world = world(tmp.path());
    let other = World::open(&tmp.path().join("world.db"), 0).unwrap();
    other.add_event(&GodEvent::new(3, "from the UI")).unwrap();
    other.close().unwrap();
    assert_eq!(world.events().unwrap(), [GodEvent::new(3, "from the UI")]);
}

#[test]
fn read_only_world_refuses_writes() {
    let tmp = tempfile::tempdir().unwrap();
    world(tmp.path()).close().unwrap();
    let ro = World::open_read_only(&tmp.path().join("world.db"), 0).unwrap();
    assert_eq!(ro.personas().unwrap().len(), 5);
    assert!(ro.add_event(&GodEvent::new(1, "x")).is_err());
}

/// A three-round world with memories kept per round, built through the public API.
fn three_rounds(path: &Path) {
    let w = World::open(path, 7).unwrap();
    w.add_personas(&(1..=4).map(persona).collect::<Vec<_>>()).unwrap();
    w.add_follow("a01", "a02").unwrap();
    let opening = w.apply(0, &act("a01", ActionType::Post).content("opening")).unwrap().unwrap();
    w.save_round_stats(&RoundStats { round: 0, ..Default::default() }).unwrap();
    for i in 1..=4 {
        w.save_memory(&AgentMemory { agent_id: format!("a{i:02}"), notes: vec![] }).unwrap();
    }
    w.snapshot_memories(0).unwrap();
    for r in 1..=3 {
        let p = w
            .apply(r, &act(&format!("a0{}", r + 1), ActionType::Post).content(&format!("round {r}")))
            .unwrap()
            .unwrap();
        w.apply(r, &act("a01", ActionType::Like).post(p)).unwrap();
        w.apply(r, &act(&format!("a0{}", r + 1), ActionType::Reply).content("re").post(opening)).unwrap();
        w.apply(r, &act(&format!("a0{}", r + 1), ActionType::Follow).agent("a01")).unwrap();
        w.save_post_sentiment(&[(p, 0.1 * r as f64)]).unwrap();
        w.add_event(&GodEvent::new(r, &format!("event {r}"))).unwrap();
        for i in 1..=4 {
            let notes = (1..=r).map(|n| format!("note {n}")).collect();
            w.save_memory(&AgentMemory { agent_id: format!("a{i:02}"), notes }).unwrap();
        }
        w.save_round_stats(&RoundStats { round: r, new_posts: 2, ..Default::default() }).unwrap();
        w.snapshot_memories(r).unwrap();
    }
    w.close().unwrap();
}

#[test]
fn truncating_restores_the_world_of_an_earlier_round() {
    let tmp = tempfile::tempdir().unwrap();
    let long = tmp.path().join("long.db");
    three_rounds(&long);
    let w = World::open(&long, 7).unwrap();
    assert_eq!(w.memory_rounds().unwrap(), [0, 1, 2, 3]);
    assert_eq!(w.last_round().unwrap(), 3);
    w.truncate_after(1).unwrap();
    let opening = w.post(1).unwrap();
    assert_eq!((opening.replies, opening.likes), (1, 0));
    assert_eq!(w.posts(None).unwrap().iter().map(|p| p.round).max(), Some(1));
    assert_eq!(w.followers("a01").unwrap(), ["a02"]);
    assert_eq!(w.memory("a03").unwrap().notes, ["note 1"]);
    assert_eq!(w.events().unwrap(), [GodEvent::new(1, "event 1")]);
    assert_eq!(w.post_sentiments().unwrap().len(), 1);
    // the counters pick up again where the kept rows end
    let next = w.apply(2, &act("a04", ActionType::Post).content("after the branch")).unwrap().unwrap();
    assert_eq!(next, 4);
    w.close().unwrap();
    assert_eq!(table(&long, "SELECT MAX(round) FROM memory_history"), json!([[1]]));
}

#[test]
fn older_runs_branch_only_from_round_zero_or_their_last_round() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("old.db");
    three_rounds(&path);
    rusqlite::Connection::open(&path).unwrap().execute_batch("DROP TABLE memory_history").unwrap();
    let w = World::open(&path, 7).unwrap(); // recreates the empty table
    assert!(w.memory_rounds().unwrap().is_empty());
    assert!(matches!(w.truncate_after(1), Err(WorldError::Branch(_))));
    w.truncate_after(0).unwrap();
    assert!(w.memory("a01").unwrap().notes.is_empty());
    assert_eq!(w.posts(None).unwrap().iter().map(|p| p.round).max(), Some(0));
}
