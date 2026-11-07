import re

import pytest

from tellfold.graph import (
    ExtractedEntity,
    ExtractedRelation,
    Extraction,
    extract_graph,
    match_key,
    merge_extractions,
    slugify,
)
from tellfold.llm import FakeLLM, LLMError
from tellfold.models import Chunk

CHUNKS = [Chunk(id=i, source="seed.md", text=f"chunk text {i}") for i in range(9)]


def _chunk_ids(prompt: str) -> list[int]:
    return [int(x) for x in re.findall(r'<chunk id="(\d+)"', prompt)]


def test_slug_and_match_key():
    assert slugify("Acme Corp.") == "acme-corp"
    assert slugify("Café Société & Co") == "cafe-societe-and-co"
    assert slugify("東京都") == "東京都"
    assert match_key("The Acme Corporation") == match_key("Acme Inc.") == "acme"
    assert match_key("The") == "the"


def test_merge_dedupes_by_name_and_alias_and_drops_dangling_relations():
    part1 = Extraction(
        entities=[
            ExtractedEntity(name="Acme Corp", kind="company", summary="Makes widgets.", chunk_ids=[0]),
            ExtractedEntity(name="Riders Union", kind="group", summary="Transit riders.",
                            aliases=["RU"], chunk_ids=[1]),
        ],
        relations=[
            ExtractedRelation(source="Acme Corp", target="RU", label="Lobbies", evidence="Met twice."),
            ExtractedRelation(source="Acme Corp", target="Nobody Known", label="sues"),
            ExtractedRelation(source="Acme Corp", target="Acme Inc", label="is"),
        ],
    )
    part2 = Extraction(
        entities=[
            ExtractedEntity(name="ACME Inc.", kind="organization", summary="Makes widgets.", chunk_ids=[4]),
            ExtractedEntity(name="RU", kind="org", summary="Union of bus riders.", chunk_ids=[5]),
        ],
        relations=[ExtractedRelation(source="acme corp", target="Riders Union", label="lobbies ")],
    )
    graph = merge_extractions([part1, part2])
    by_id = {e.id: e for e in graph.entities}
    assert set(by_id) == {"acme-corp", "riders-union"}
    acme = by_id["acme-corp"]
    assert acme.kind == "organization" and acme.source_chunk_ids == [0, 4]
    assert acme.summary == "Makes widgets."  # identical summaries are not repeated
    assert "Union of bus riders." in by_id["riders-union"].summary
    assert [(r.source, r.target, r.label, r.evidence) for r in graph.relations] == [
        ("acme-corp", "riders-union", "lobbies", "Met twice.")
    ]


def test_merge_caps_long_summaries_and_keeps_ids_unique():
    long = "A very detailed sentence about the org. " * 40
    graph = merge_extractions([Extraction(entities=[
        ExtractedEntity(name="Big Org", kind="organization", summary=long),
        ExtractedEntity(name="Big-Org!", kind="weird-kind", summary="x"),
        ExtractedEntity(name="   ", kind="person"),
    ])])
    assert len(graph.entities) == 1
    assert len(graph.entities[0].summary) <= 480
    assert graph.entities[0].summary.endswith(".")


async def test_extract_graph_runs_groups_and_merges():
    def responder(purpose, prompt, schema):
        assert purpose == "graph.extract" and schema is Extraction
        ids = _chunk_ids(prompt)
        return {
            "entities": [
                {"name": "Harbor Town Council", "kind": "organization", "summary": "City council.",
                 "aliases": ["the council"], "chunk_ids": ids[:1] + [999]},
                {"name": f"Group {ids[0]}", "kind": "group", "summary": "Locals."},
            ],
            "relations": [{"source": "The Council", "target": f"Group {ids[0]}", "label": "consults"}],
        }

    llm = FakeLLM(responder)
    progress: list[str] = []
    graph = await extract_graph(llm, CHUNKS, question="Will the shuttle succeed?", model="sonnet",
                                on_progress=progress.append)
    assert len(llm.calls) == 3  # 9 chunks, 4 per call
    assert all("Will the shuttle succeed?" in prompt for _, _, prompt in llm.calls)
    council = next(e for e in graph.entities if e.id == "harbor-town-council")
    assert council.source_chunk_ids == [0, 4, 8]  # hallucinated chunk id 999 removed
    group0 = next(e for e in graph.entities if e.id == "group-0")
    assert group0.source_chunk_ids == [0, 1, 2, 3]  # no citation -> whole group
    assert graph.entities[0].id == "harbor-town-council"  # most-mentioned first
    assert len(graph.relations) == 3
    assert len(progress) == 4


async def test_extract_graph_tolerates_a_failed_group_but_not_all():
    def flaky(purpose, prompt, schema):
        if _chunk_ids(prompt)[0] == 4:
            return LLMError("boom")
        return {"entities": [{"name": "Acme", "kind": "organization", "summary": "s"}], "relations": []}

    graph = await extract_graph(FakeLLM(flaky), CHUNKS, question="q", model="m")
    assert [e.id for e in graph.entities] == ["acme"]

    with pytest.raises(LLMError):
        await extract_graph(FakeLLM(lambda *a: LLMError("down")), CHUNKS, question="q", model="m")


async def test_extract_graph_with_no_chunks_is_empty():
    llm = FakeLLM(lambda *a: {})
    graph = await extract_graph(llm, [], question="q", model="m")
    assert graph.entities == [] and llm.calls == []
