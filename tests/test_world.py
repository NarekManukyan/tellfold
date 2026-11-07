from pathlib import Path

import pytest

from tellfold.models import Action, ActionType, AgentMemory, GodEvent, Persona, RoundStats
from tellfold.world import World


def persona(i: int, **kw) -> Persona:
    base = dict(
        id=f"a{i:02d}",
        handle=f"user{i}",
        name=f"User {i}",
        bio="bio",
        background="bg",
        stance="neutral",
    )
    return Persona(**(base | kw))


@pytest.fixture
def world(tmp_path: Path) -> World:
    w = World(tmp_path / "world.db", seed=3)
    w.add_personas([persona(i) for i in range(1, 6)])
    return w


def act(agent: str, kind: ActionType, **kw) -> Action:
    return Action(agent_id=agent, type=kind, **kw)


def test_personas_roundtrip(world: World):
    assert [p.id for p in world.personas()] == ["a01", "a02", "a03", "a04", "a05"]
    assert world.persona("a03").handle == "user3"
    with pytest.raises(KeyError):
        world.persona("zz")


def test_post_reply_repost_counters(world: World):
    pid = world.apply(1, act("a01", ActionType.POST, content="hello"))
    rid = world.apply(1, act("a02", ActionType.REPLY, content="hi back", target_post_id=pid))
    rp = world.apply(1, act("a03", ActionType.REPOST, target_post_id=pid))
    rp2 = world.apply(2, act("a04", ActionType.REPOST, target_post_id=rp))  # repost of a repost credits original
    assert world.post(rid).parent_id == pid
    assert world.post(rp2).repost_of == pid
    original = world.post(pid)
    assert (original.replies, original.reposts) == (1, 2)
    assert [p.id for p in world.posts(1)] == [pid, rid, rp]


def test_invalid_actions_downgrade_to_nothing(world: World):
    pid = world.apply(1, act("a01", ActionType.POST, content="x"))
    assert world.apply(1, act("a02", ActionType.REPLY, content="?", target_post_id=999)) is None
    assert world.apply(1, act("a02", ActionType.LIKE, target_post_id=None)) is None
    assert world.apply(1, act("a02", ActionType.FOLLOW, target_agent_id="a02")) is None  # self-follow
    assert world.apply(1, act("a02", ActionType.FOLLOW, target_agent_id="nobody")) is None
    assert world.apply(1, act("ghost", ActionType.POST, content="boo")) is None
    assert world.apply(1, act("a01", ActionType.LIKE, target_post_id=pid)) is None  # own post
    assert world.apply(1, act("a01", ActionType.REPOST, target_post_id=pid)) is None  # own post
    assert world.apply(1, act("a03", ActionType.POST, content="   ")) is None
    kinds = [a.type for _, a in world.actions(1)]
    assert kinds[0] is ActionType.POST
    assert all(k is ActionType.NOTHING for k in kinds[1:])
    assert "unknown target post" in world.actions(1)[1][1].reasoning
    assert len(world.posts()) == 1


def test_one_reaction_per_agent_per_post(world: World):
    pid = world.apply(1, act("a01", ActionType.POST, content="x"))
    world.apply(1, act("a02", ActionType.LIKE, target_post_id=pid))
    world.apply(1, act("a02", ActionType.LIKE, target_post_id=pid))
    world.apply(1, act("a02", ActionType.DISLIKE, target_post_id=pid))
    world.apply(1, act("a03", ActionType.DISLIKE, target_post_id=pid))
    post = world.post(pid)
    assert (post.likes, post.dislikes) == (1, 1)


def test_follow_graph(world: World):
    world.apply(1, act("a01", ActionType.FOLLOW, target_agent_id="a02"))
    world.apply(1, act("a01", ActionType.FOLLOW, target_agent_id="a02"))  # duplicate -> nothing
    assert world.add_follow("a03", "a02")
    assert not world.add_follow("a03", "a03")
    assert world.following("a01") == ["a02"]
    assert world.followers("a02") == ["a01", "a03"]
    assert [a.type for _, a in world.actions()] == [ActionType.FOLLOW, ActionType.NOTHING]


def test_feed_ranking(world: World):
    world.add_follow("a01", "a02")
    mine = world.apply(1, act("a01", ActionType.POST, content="my post"))
    popular = world.apply(1, act("a03", ActionType.POST, content="popular"))
    quiet = world.apply(1, act("a04", ActionType.POST, content="quiet"))
    followed = world.apply(1, act("a02", ActionType.POST, content="from a friend"))
    for liker in ("a02", "a04", "a05"):
        world.apply(1, act(liker, ActionType.LIKE, target_post_id=popular))
    reply = world.apply(2, act("a05", ActionType.REPLY, content="re", target_post_id=mine))

    feed = [p.id for p in world.feed_for("a01", 2, 10)]
    assert mine not in feed
    assert feed[:2] == [reply, followed]
    assert feed.index(popular) < feed.index(quiet)
    assert [p.id for p in world.feed_for("a01", 2, 2)] == [reply, followed]
    assert world.feed_for("a01", 2, 10) == world.feed_for("a01", 2, 10)  # deterministic
    assert reply not in [p.id for p in world.feed_for("a01", 1, 10)]  # future posts hidden


def test_feed_recency_and_dislikes(world: World):
    old = world.apply(1, act("a02", ActionType.POST, content="old"))
    new = world.apply(3, act("a03", ActionType.POST, content="new"))
    hated = world.apply(3, act("a04", ActionType.POST, content="hated"))
    for agent in ("a01", "a02", "a03"):
        world.apply(3, act(agent, ActionType.DISLIKE, target_post_id=hated))
    feed = [p.id for p in world.feed_for("a05", 3, 10)]
    assert feed.index(new) < feed.index(old)
    assert feed[-1] == hated


def test_memory_stats_events_persist(tmp_path: Path):
    path = tmp_path / "w.db"
    w = World(path)
    w.add_personas([persona(1)])
    assert w.memory("a01").notes == []
    w.save_memory(AgentMemory(agent_id="a01", notes=["saw a post", "änderte Meinung"]))
    w.save_round_stats(RoundStats(round=1, actions={"post": 2}, new_posts=2, sentiment=0.25))
    w.save_round_stats(RoundStats(round=1, actions={"post": 3}, new_posts=3))  # upsert
    w.add_event(GodEvent(round=2, content="Breaking"))
    w.close()

    w2 = World(path)
    assert w2.memory("a01").notes == ["saw a post", "änderte Meinung"]
    assert [s.new_posts for s in w2.round_stats()] == [3]
    assert w2.events() == [GodEvent(round=2, content="Breaking")]
    assert w2.persona("a01").name == "User 1"


def test_stale_followed_posts_do_not_bury_fresh_ones(world: World):
    world.add_follow("a01", "a02")
    old_followed = [world.apply(1, act("a02", ActionType.POST, content=f"old {i}")) for i in range(5)]
    fresh = world.apply(6, act("a03", ActionType.POST, content="breaking"))
    feed = [p.id for p in world.feed_for("a01", 6, 3)]
    assert feed[0] == fresh
    # while still fresh, a followed post keeps its priority over a stranger's
    recent_followed = world.apply(6, act("a02", ActionType.POST, content="new from a friend"))
    assert [p.id for p in world.feed_for("a01", 6, 2)] == [recent_followed, fresh]
    assert set(old_followed) & {p.id for p in world.feed_for("a01", 6, 10)}


def test_engaged_posts_sink_and_are_reported(world: World):
    liked = world.apply(1, act("a02", ActionType.POST, content="liked"))
    replied = world.apply(1, act("a03", ActionType.POST, content="replied"))
    reposted = world.apply(1, act("a04", ActionType.POST, content="reposted"))
    untouched = world.apply(1, act("a05", ActionType.POST, content="untouched"))
    world.apply(1, act("a01", ActionType.LIKE, target_post_id=liked))
    world.apply(1, act("a01", ActionType.REPLY, content="hi", target_post_id=replied))
    world.apply(1, act("a01", ActionType.REPOST, target_post_id=reposted))
    world.apply(1, act("a01", ActionType.DISLIKE, target_post_id=replied))
    assert world.engagement_by("a01") == {liked: ["liked"], replied: ["disliked", "replied"], reposted: ["reposted"]}
    feed = [p.id for p in world.feed_for("a01", 1, 10)]
    assert feed[0] == untouched
    assert set(feed[-3:]) == {liked, replied, reposted}


def test_post_sentiment_roundtrip_and_clamp(world: World):
    a = world.apply(0, act("a01", ActionType.POST, content="one"))
    b = world.apply(1, act("a02", ActionType.POST, content="two"))
    assert world.post_sentiments() == {}
    world.save_post_sentiment({a: 0.4, b: -3.0})
    world.save_post_sentiment({a: 0.1})  # re-scoring overwrites
    world.save_post_sentiment({})
    assert world.post_sentiments() == {a: 0.1, b: -1.0}


def test_follow_edges(world: World):
    world.add_follow("a02", "a01")
    world.add_follow("a01", "a03")
    world.apply(1, act("a03", ActionType.FOLLOW, target_agent_id="a01"))
    assert world.follow_edges() == [("a01", "a03"), ("a02", "a01"), ("a03", "a01")]


def test_old_database_gains_post_sentiment_table(tmp_path: Path):
    import sqlite3

    path = tmp_path / "old.db"
    db = sqlite3.connect(path)
    db.execute("CREATE TABLE posts (id INTEGER PRIMARY KEY AUTOINCREMENT, round INTEGER NOT NULL, "
               "author_id TEXT NOT NULL, content TEXT NOT NULL, parent_id INTEGER, repost_of INTEGER, "
               "likes INTEGER NOT NULL DEFAULT 0, dislikes INTEGER NOT NULL DEFAULT 0, "
               "reposts INTEGER NOT NULL DEFAULT 0, replies INTEGER NOT NULL DEFAULT 0)")
    db.execute("INSERT INTO posts (round, author_id, content) VALUES (0, 'a01', 'old post')")
    db.commit()
    db.close()
    w = World(path)
    assert w.post_sentiments() == {} and w.posts()[0].content == "old post"
    w.save_post_sentiment({1: 0.2})
    assert w.post_sentiments() == {1: 0.2}


def test_event_added_from_second_connection_is_visible(world: World, tmp_path: Path):
    other = World(tmp_path / "world.db")
    other.add_event(GodEvent(round=3, content="from the UI"))
    other.close()
    assert world.events() == [GodEvent(round=3, content="from the UI")]
