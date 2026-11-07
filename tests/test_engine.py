import re
from pathlib import Path

import pytest

from tellfold.engine import Engine, build_follow_graph, pick_opening_authors, sample_active
from tellfold.llm import FakeLLM, LLMError
from tellfold.models import Action, ActionType, Entity, GodEvent, KnowledgeGraph, Persona, Relation, SimConfig
from tellfold.world import World

AGENT_RE = re.compile(r"^=== (a\d+) @", re.M)


def make_personas(n: int = 8) -> list[Persona]:
    out = []
    for i in range(1, n + 1):
        out.append(
            Persona(
                id=f"a{i:02d}",
                handle=f"user{i}",
                name=f"User {i}",
                bio="bio",
                background="bg",
                stance="supportive" if i % 2 else "critical",
                traits=["curious"] if i % 3 else ["skeptical"],
                activity=0.9,
                influence=0.9 if i <= 2 else 0.2,
                entity_id="acme" if i == 1 else ("gov" if i == 2 else None),
            )
        )
    return out


GRAPH = KnowledgeGraph(
    entities=[
        Entity(id="acme", name="Acme", kind="organization", summary="Maker of widgets"),
        Entity(id="gov", name="Regulator", kind="organization", summary="Oversees widgets"),
    ],
    relations=[Relation(source="gov", target="acme", label="regulates")],
)


def agent_blocks(prompt: str) -> dict[str, str]:
    """Split a round prompt into per-agent sections."""
    parts = AGENT_RE.split(prompt)
    return {parts[i]: parts[i + 1] for i in range(1, len(parts), 2)}


def responder_factory(*, fail_agent: str | None = None, bad_ids: bool = False):
    def respond(purpose: str, prompt: str, schema):
        if purpose == "engine.seed":
            return {
                "posts": [
                    {"agent_id": aid, "content": f"opening from {aid}", "memory_note": "I spoke first"}
                    for aid in AGENT_RE.findall(prompt)
                ]
            }
        if purpose == "engine.sentiment":
            ids = [int(x) for x in re.findall(r"^#(\d+):", prompt, re.M)]
            return {"scores": [{"post_id": i, "score": 0.5 if i % 2 else -0.1} for i in ids] + [{"post_id": 99999, "score": 5}]}
        assert purpose == "engine.round"
        blocks = agent_blocks(prompt)
        if fail_agent and fail_agent in blocks:
            return LLMError("simulated failure")
        decisions = []
        for aid, block in blocks.items():
            feed_ids = [int(x) for x in re.findall(r"^\[#(\d+)\]", block, re.M)]
            actions = []
            if bad_ids:
                actions.append({"type": "like", "target_post_id": 424242})
            if feed_ids:
                actions.append({"type": "like", "target_post_id": f"#{feed_ids[0]}", "reasoning": "agree"})
                actions.append({"type": "reply", "target_post_id": feed_ids[0], "content": f"reply by {aid}"})
            else:
                actions.append({"type": "post", "content": f"post by {aid}"})
            actions.append({"type": "dance"})
            decisions.append({"agent_id": aid, "actions": actions, "memory_note": f"note from {aid}"})
        decisions.append({"agent_id": "a99", "actions": [{"type": "post", "content": "intruder"}]})
        return {"decisions": decisions}

    return respond


@pytest.fixture
def world(tmp_path: Path) -> World:
    w = World(tmp_path / "world.db", seed=7)
    w.add_personas(make_personas())
    return w


def config(**kw) -> SimConfig:
    base = dict(question="Will widgets be banned?", seed_files=[], rounds=2, batch_size=3, memory_size=2)
    return SimConfig(**(base | kw))


async def test_seed_world_builds_graph_and_opening_posts(world: World):
    events = []
    llm = FakeLLM(responder_factory())
    engine = Engine(llm, world, config(events=[GodEvent(round=0, content="Launch day")]), on_event=lambda n, p: events.append((n, p)))
    await engine.seed_world(GRAPH)

    followers = {p.id: len(world.followers(p.id)) for p in world.personas()}
    assert sum(followers.values()) > 0
    assert followers["a01"] + followers["a02"] > followers["a07"] + followers["a08"]
    openers = world.posts(0)
    assert openers and all(p.content.startswith("opening from") for p in openers)
    assert len(openers) == round(8 * 0.3)
    assert world.round_stats()[0].round == 0
    assert world.round_stats()[0].sentiment is not None
    assert world.memory(openers[0].author_id).notes == ["R0: I spoke first"]
    assert world.actions(0) and all(a.type is ActionType.POST for _, a in world.actions(0))
    seed_prompt = next(p for purpose, _, p in llm.calls if purpose == "engine.seed")
    assert "Launch day" in seed_prompt and "Acme" in seed_prompt
    assert ("stage", {"stage": "follow_graph"}) in events


async def test_run_round_loop(world: World):
    events = []
    llm = FakeLLM(responder_factory(bad_ids=True))
    cfg = config(events=[GodEvent(round=2, content="Regulator announces probe")])
    engine = Engine(llm, world, cfg, on_event=lambda n, p: events.append((n, p)))
    await engine.seed_world(GRAPH)
    stats = await engine.run()

    assert [s.round for s in stats] == [1, 2]
    assert all(s.active_agents >= 1 for s in stats)
    assert stats[0].actions.get("nothing", 0) > 0  # bad ids and unknown types downgraded
    assert stats[0].actions.get("like", 0) > 0
    assert stats[0].new_posts == len(world.posts(1))
    assert all("a99" != a.agent_id for _, a in world.actions())
    assert -1 <= stats[0].sentiment <= 1
    # every action targets a post that existed and was in the agent's feed: no orphan counters
    assert all(p.likes <= 8 for p in world.posts())
    # memory trimmed to memory_size
    assert all(len(world.memory(p.id).notes) <= 2 for p in world.personas())
    round2 = [p for purpose, _, p in llm.calls if purpose == "engine.round"][-1]
    assert "BREAKING NEWS" in round2 and "Regulator announces probe" in round2
    assert world.events() == [GodEvent(round=2, content="Regulator announces probe")]
    names = [n for n, _ in events]
    assert names.count("round_start") == 2 and names.count("round_done") == 3  # round 0 (opening posts) too
    assert "event_injected" not in names  # planned events are not announced as injections
    done = [p for n, p in events if n == "round_done"]
    assert [d["round"] for d in done] == [0, 1, 2] and done[1]["total_rounds"] == 2
    starts = [p for n, p in events if n == "round_start"]
    assert all(s["active"] == len(s["active_agents"]) >= 1 for s in starts)


async def test_failed_batch_is_tolerated(world: World):
    events = []
    llm = FakeLLM(responder_factory(fail_agent="a03"))
    engine = Engine(llm, world, config(), on_event=lambda n, p: events.append((n, p)))
    await engine.seed_world(GRAPH)
    stats = await engine.run()

    failed = [p for n, p in events if n == "batch_failed"]
    assert failed and all("a03" in p["agents"] for p in failed)
    assert len(stats) == 2
    assert not [a for r, a in world.actions() if r > 0 and a.agent_id == "a03"]
    assert any(r > 0 for r, _ in world.actions())  # other batches still acted


async def test_only_own_feed_ids_accepted(world: World):
    """A post id that exists but is not in the agent's feed must be rejected."""
    hidden = world.apply(0, Action(agent_id="a01", type=ActionType.POST, content="hidden"))

    def respond(purpose, prompt, schema):
        if purpose == "engine.sentiment":
            return {"scores": []}
        return {"decisions": [{"agent_id": aid, "actions": [{"type": "like", "target_post_id": hidden}]}
                              for aid in agent_blocks(prompt)]}

    engine = Engine(FakeLLM(respond), world, config(feed_size=0, rounds=1))
    await engine.run()
    assert world.post(hidden).likes == 0
    assert all(a.type is ActionType.NOTHING for _, a in world.actions(1))


def test_sampling_is_seeded_and_never_empty():
    personas = make_personas()
    a = [p.id for p in sample_active(personas, 3, seed=1)]
    b = [p.id for p in sample_active(personas, 3, seed=1)]
    assert a == b
    lurkers = [p.model_copy(update={"activity": 0.0}) for p in personas]
    assert len(sample_active(lurkers, 1, seed=1)) == 1
    mixed = [p.model_copy(update={"activity": 0.1 if i % 2 else 1.0}) for i, p in enumerate(personas)]
    counts = sum(len(sample_active(mixed, r, seed=5)) for r in range(50)) / 50
    assert 4 <= counts <= 5.5


def test_follow_graph_and_openers_deterministic():
    personas = make_personas(12)
    g1 = build_follow_graph(personas, GRAPH, seed=4)
    assert g1 == build_follow_graph(personas, GRAPH, seed=4)
    assert all(f != t for f, t in g1)
    assert len(set(g1)) == len(g1)
    assert ("a02", "a01") in g1  # related entity accounts follow each other with high weight
    assert pick_opening_authors(personas, seed=4) == pick_opening_authors(personas, seed=4)


async def test_feed_surfaces_unengaged_posts_and_marks_reacted_ones(world: World):
    """An agent that always likes the top of its feed must never hit 'already reacted'; old likes are marked."""
    for p in world.personas():
        for j in range(2):
            world.apply(0, Action(agent_id=p.id, type=ActionType.POST, content=f"post {j} by {p.id}"))

    prompts: list[str] = []

    def respond(purpose, prompt, schema):
        if purpose == "engine.sentiment":
            return {"scores": []}
        prompts.append(prompt)
        decisions = []
        for aid, block in agent_blocks(prompt).items():
            top = int(re.search(r"^\[#(\d+)\]", block, re.M).group(1))
            decisions.append({"agent_id": aid, "actions": [{"type": "like", "target_post_id": top}]})
        return {"decisions": decisions}

    engine = Engine(FakeLLM(respond), world, config(feed_size=50, rounds=4, batch_size=8))
    await engine.run()

    likes = [a for r, a in world.actions() if r > 0 and a.type is ActionType.LIKE]
    downgraded = [a for r, a in world.actions() if r > 0 and "already reacted" in a.reasoning]
    assert likes and not downgraded
    assert any("[you already liked this]" in p for p in prompts[1:])


async def test_events_are_json_safe_and_posts_stream_per_batch(world: World):
    import json

    events = []
    engine = Engine(FakeLLM(responder_factory()), world, config(batch_size=2), on_event=lambda n, p: events.append((n, p)))
    await engine.seed_world(GRAPH)
    await engine.run()

    for _, payload in events:
        json.dumps(payload)  # SSE forwards payloads verbatim
    added = [p for n, p in events if n == "posts_added"]
    assert added[0]["round"] == 0
    streamed = sorted(i for p in added for i in p["post_ids"])
    assert streamed == [p.id for p in world.posts()]
    assert len([p for p in added if p["round"] == 1]) >= 2  # several batches each streamed their posts


async def test_per_post_sentiment_is_stored(world: World):
    engine = Engine(FakeLLM(responder_factory()), world, config())
    await engine.seed_world(GRAPH)
    await engine.run()

    scores = world.post_sentiments()
    assert set(scores) == {p.id for p in world.posts()}  # round 0 included; unknown id 99999 dropped
    assert all(v == (0.5 if pid % 2 else -0.1) for pid, v in scores.items())
    for stats in world.round_stats():
        ids = [p.id for p in world.posts(stats.round)]
        assert stats.sentiment == round(sum(scores[i] for i in ids) / len(ids), 3)


async def test_event_added_mid_run_is_picked_up_and_announced(world: World):
    events = []
    cfg = config(rounds=3, events=[GodEvent(round=2, content="Planned news")])
    engine = Engine(FakeLLM(responder_factory()), world, cfg, on_event=lambda n, p: events.append((n, p)))
    await engine.seed_world(GRAPH)
    await engine.run_round(1)
    world.add_event(GodEvent(round=2, content="Injected news"))
    world.add_event(GodEvent(round=3, content="Later injected news"))
    await engine.run_round(2)
    await engine.run_round(3)

    injected = [p for n, p in events if n == "event_injected"]
    assert injected == [{"round": 2, "content": "Injected news"}, {"round": 3, "content": "Later injected news"}]
    round2 = [p for purpose, _, p in engine.llm.calls if purpose == "engine.round"][-2]
    assert "Injected news" in round2 and "Planned news" in round2
    starts = {p["round"]: p for n, p in events if n == "round_start"}
    assert sorted(starts[2]["events"]) == ["Injected news", "Planned news"]
