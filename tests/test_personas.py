import re
from collections import Counter

import pytest

from shoal.llm import FakeLLM, LLMError
from shoal.models import Entity, KnowledgeGraph, Relation
from shoal.personas import PersonaBatch, generate_personas, plan_slots, unique_handle

GRAPH = KnowledgeGraph(
    entities=[
        Entity(id="harbor-town-council", name="Harbor Town Council", kind="organization",
               summary="Voted for the shuttle pilot.", source_chunk_ids=[0, 1, 2]),
        Entity(id="ines-duarte", name="Ines Duarte", kind="person", summary="Mayor.", source_chunk_ids=[0]),
        Entity(id="riders-union", name="Riders Union", kind="organization", summary="Opposes cuts.",
               source_chunk_ids=[1]),
        Entity(id="pier-street-shop-owners", name="Pier Street shop owners", kind="group",
               summary="Worried about footfall."),
        Entity(id="shuttle", name="On-demand shuttle", kind="product", summary="The pilot service."),
    ],
    relations=[Relation(source="riders-union", target="harbor-town-council", label="criticizes")],
)
QUESTION = "How will residents react to replacing bus lines with an on-demand shuttle?"


def _slots(prompt: str) -> list[str]:
    return re.findall(r"- slot (a\d+):", prompt)


def _draft(slot: str, handle: str = "same_handle", activity: float = 2.0, influence: float = -1.0) -> dict:
    return {"slot": slot, "handle": handle, "name": f"Person {slot}", "bio": "bio", "background": "bg",
            "stance": "st", "traits": ["calm", " ", "a", "b", "c", "d", "e"],
            "activity": activity, "influence": influence}


def test_plan_mixes_entities_and_a_varied_public():
    slots = plan_slots(GRAPH, question=QUESTION, n=20)
    assert [s.label for s in slots] == [f"a{i:02d}" for i in range(1, 21)]
    entity_ids = [s.entity_id for s in slots if s.entity_id]
    assert entity_ids[0] == "harbor-town-council"  # most connected/mentioned first
    assert set(entity_ids) == {"harbor-town-council", "ines-duarte", "riders-union"}  # groups not embodied
    public = [s.brief for s in slots if not s.entity_id]
    stances = Counter(b.split("starting stance: ")[1].split(";")[0] for b in public)
    assert len(stances) >= 5 and max(stances.values()) <= 4  # no single dominant stance
    assert sum("strongly" in k for k in stances.elements()) <= len(public) // 3  # few extremists
    assert sum("lurker" in b for b in public) >= len(public) // 3  # most are not very engaged
    assert any("Pier Street shop owners" in b for b in public)
    assert plan_slots(GRAPH, question=QUESTION, n=20) == slots  # deterministic


def test_plan_without_entities_and_tiny_casts():
    assert all(s.entity_id is None for s in plan_slots(KnowledgeGraph(), question="q", n=5))
    assert plan_slots(GRAPH, question="q", n=1)[0].entity_id is None
    assert [s.label for s in plan_slots(GRAPH, question="q", n=120)][-1] == "a120"


def test_unique_handle_sanitizes_and_dedupes():
    taken = {"maria_dev"}
    assert unique_handle("@Maria.Dev", "Maria", taken) == "maria_dev_2"
    assert unique_handle("", "Zoë Ångström", set()) == "zoe_angstrom"
    assert unique_handle("", "東京", set()) == "user"
    assert len(unique_handle("x" * 40, "", {"x" * 20})) <= 20


async def test_generate_personas_in_batches_with_unique_handles_and_clamped_values():
    def responder(purpose, prompt, schema):
        if purpose == "personas.cast":
            return LLMError("cast pick unavailable")  # falls back to ranking by graph prominence
        assert purpose == "personas.batch" and schema is PersonaBatch
        assert "Spanish" in prompt and QUESTION in prompt
        return {"personas": [_draft(s) for s in _slots(prompt)]}

    llm = FakeLLM(responder)
    personas = await generate_personas(llm, GRAPH, question=QUESTION, n=23, model="sonnet", language="Spanish")
    batches = [c for c in llm.calls if c[0] == "personas.batch"]
    assert len(batches) == 3  # batches of <= 10
    assert all(len(_slots(p)) <= 10 for _, _, p in batches)
    assert "@same_handle" in batches[1][2]  # later batches see the handles already taken
    assert [p.id for p in personas] == [f"a{i:02d}" for i in range(1, 24)]
    assert len({p.handle for p in personas}) == 23
    assert all(0 <= p.activity <= 1 and 0 <= p.influence <= 1 for p in personas)
    assert all(len(p.traits) <= 5 and " " not in p.traits for p in personas)
    assert personas[0].entity_id == "harbor-town-council"
    assert sum(p.entity_id is not None for p in personas) == 3
    public = [p for p in personas if p.entity_id is None]
    assert max(p.activity for p in public) <= 0.95  # clamped into each slot's engagement band
    assert any(p.activity <= 0.3 for p in public)


async def test_generate_personas_retries_missing_and_failed_batches():
    calls = {"n": 0}

    def responder(purpose, prompt, schema):
        if purpose == "personas.cast":
            return LLMError("cast pick unavailable")
        calls["n"] += 1
        if calls["n"] == 1:
            return LLMError("rate limited")
        slots = _slots(prompt)
        if calls["n"] == 2:  # returns only half, with wrong slot ids -> matched positionally
            return {"personas": [_draft("zz", handle=f"h{i}") for i in range(len(slots) // 2)]}
        return {"personas": [_draft(s, handle=f"x_{s}", activity=0.5, influence=0.4) for s in slots]}

    personas = await generate_personas(FakeLLM(responder), GRAPH, question=QUESTION, n=8, model="m",
                                       language="English")
    assert [p.id for p in personas] == [f"a{i:02d}" for i in range(1, 9)]
    assert {p.handle for p in personas[:4]} == {"h0", "h1", "h2", "h3"}


async def test_generate_personas_raises_when_nothing_comes_back():
    with pytest.raises(LLMError):
        await generate_personas(FakeLLM(lambda *a: LLMError("down")), GRAPH, question="q", n=4,
                                model="m", language="English")
    assert await generate_personas(FakeLLM(lambda *a: {}), GRAPH, question="q", n=0, model="m",
                                   language="English") == []


async def test_planner_picks_which_entities_get_a_voice():
    candidates = [e.id for e in GRAPH.entities if e.kind in ("organization", "person")]
    chosen = candidates[-1]

    def responder(purpose, prompt, schema):
        if purpose == "personas.cast":
            assert all(c in prompt for c in candidates)
            return {"entity_ids": [chosen, "not-a-real-id", chosen]}
        return {"personas": [_draft(s) for s in _slots(prompt)]}

    personas = await generate_personas(FakeLLM(responder), GRAPH, question=QUESTION, n=10, model="sonnet",
                                       language="English")
    assert [p.entity_id for p in personas if p.entity_id] == [chosen]
