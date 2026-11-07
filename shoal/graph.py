"""Knowledge-graph extraction: concurrent LLM passes over chunk groups, then a pure-python merge."""

from __future__ import annotations

import re
import unicodedata
from collections import Counter
from collections.abc import Callable

import anyio
from pydantic import BaseModel, Field

from shoal.llm import LLM, LLMError
from shoal.models import Chunk, Entity, KnowledgeGraph, Relation

CHUNKS_PER_CALL = 4
SUMMARY_CAP = 480
EVIDENCE_CAP = 220
KINDS = ("person", "organization", "group", "product", "place", "concept", "event")
_KIND_SYNONYMS = {
    "company": "organization", "org": "organization", "institution": "organization",
    "agency": "organization", "media": "organization", "outlet": "organization",
    "government": "organization", "party": "organization", "union": "organization",
    "individual": "person", "people": "group", "community": "group", "demographic": "group",
    "location": "place", "country": "place", "city": "place", "region": "place",
    "policy": "concept", "idea": "concept", "law": "concept", "technology": "product",
    "service": "product", "brand": "product", "incident": "event",
}
_LEGAL_SUFFIXES = {"inc", "incorporated", "corp", "corporation", "co", "company", "ltd", "limited",
                   "llc", "plc", "gmbh", "ag", "sa", "nv", "bv"}
_ARTICLES = {"the", "a", "an"}


# ---------- LLM output schema (names, not ids: ids are assigned during the merge) ----------

class ExtractedEntity(BaseModel):
    name: str
    kind: str = "concept"
    summary: str = ""
    aliases: list[str] = Field(default_factory=list)
    chunk_ids: list[int] = Field(default_factory=list)


class ExtractedRelation(BaseModel):
    source: str
    target: str
    label: str
    evidence: str = ""


class Extraction(BaseModel):
    entities: list[ExtractedEntity] = Field(default_factory=list)
    relations: list[ExtractedRelation] = Field(default_factory=list)


SYSTEM_PROMPT = """\
You are an analyst preparing the cast for a social-reaction simulation. You read source documents and \
map the actors who would plausibly respond in public to a specific question: people, organizations, \
interest groups, communities, media outlets, regulators, competitors, customers. You also capture the \
products, places, events and concepts those actors argue about, but actors come first.

Rules:
- Only extract what the text supports. Never invent people, numbers or quotes.
- Use the most complete proper name as `name` (e.g. "European Commission", not "the Commission"); put \
short forms, acronyms and nicknames that the text uses in `aliases`.
- `kind` is exactly one of: person, organization, group, product, place, concept, event. Media outlets \
and government bodies are organizations; loose collectives (e.g. "small landlords", "students") are groups.
- `summary`: one or two factual sentences on who/what it is and, where the text shows it, its interest \
or likely position regarding the question.
- `chunk_ids`: the ids of the chunks that mention it.
- Relations connect two extracted entities by their `name`, with a short present-tense verb phrase as \
`label` ("regulates", "competes with", "funds", "criticizes") and a brief paraphrase of the supporting \
text as `evidence`.
- Skip generic nouns that are not identifiable actors or topics ("people", "the market", "experts").
- Write summaries and evidence in the language of the source text. Reply with JSON only."""


async def extract_graph(
    llm: LLM,
    chunks: list[Chunk],
    *,
    question: str,
    model: str,
    on_progress: Callable[[str], None] | None = None,
) -> KnowledgeGraph:
    groups = [chunks[i:i + CHUNKS_PER_CALL] for i in range(0, len(chunks), CHUNKS_PER_CALL)]
    results: list[Extraction | None] = [None] * len(groups)
    done = 0

    def report(message: str) -> None:
        if on_progress:
            on_progress(message)

    errors: list[str] = []

    async def run(index: int, group: list[Chunk]) -> None:
        nonlocal done
        try:
            result = await llm.json(
                _extraction_prompt(group, question),
                system=SYSTEM_PROMPT, model=model, purpose="graph.extract", schema=Extraction,
            )
            results[index] = _scope_chunk_ids(result, group)
            outcome = f"{len(result.entities)} entities"
        except LLMError as e:
            errors.append(str(e))
            outcome = f"failed: {e}"
        done += 1
        report(f"extract {done}/{len(groups)}: chunks {group[0].id}-{group[-1].id} {outcome}")

    async with anyio.create_task_group() as tg:
        for index, group in enumerate(groups):
            tg.start_soon(run, index, group)

    succeeded = [r for r in results if r is not None]
    if groups and not succeeded:
        last = f": {errors[-1]}" if errors else ""
        raise LLMError(f"graph extraction failed for every chunk group{last}")
    graph = merge_extractions(succeeded)
    report(f"graph: {len(graph.entities)} entities, {len(graph.relations)} relations")
    return graph


def _extraction_prompt(group: list[Chunk], question: str) -> str:
    body = "\n\n".join(f'<chunk id="{c.id}" source="{c.source}">\n{c.text}\n</chunk>' for c in group)
    return f"""\
Question the simulation will try to answer:
{question}

Extract the entities and relations from the chunks below. Favour the actors whose reaction to the \
question matters, and the things they would react to. A typical chunk yields 3-12 entities.

{body}

Return a JSON object:
{{"entities": [{{"name": str, "kind": str, "summary": str, "aliases": [str], "chunk_ids": [int]}}],
  "relations": [{{"source": entity name, "target": entity name, "label": str, "evidence": str}}]}}"""


def _scope_chunk_ids(result: Extraction, group: list[Chunk]) -> Extraction:
    """Keep only chunk ids from this group; fall back to the whole group when none are cited."""
    allowed = [c.id for c in group]
    for entity in result.entities:
        cited = [i for i in entity.chunk_ids if i in allowed]
        entity.chunk_ids = cited or allowed
    return result


# ---------- merge ----------

def slugify(name: str) -> str:
    """Lowercase, accent-free, dash-separated id; keeps non-latin letters so any language works."""
    text = unicodedata.normalize("NFKD", name)
    text = "".join(ch for ch in text if not unicodedata.combining(ch)).lower()
    text = text.replace("&", " and ").replace("'", "").replace("’", "")
    return re.sub(r"[\W_]+", "-", text).strip("-")


def match_key(name: str) -> str:
    """Looser key used to detect duplicates: drops leading articles and legal-form suffixes."""
    words = slugify(name).split("-")
    while len(words) > 1 and words[0] in _ARTICLES:
        words.pop(0)
    while len(words) > 1 and words[-1] in _LEGAL_SUFFIXES:
        words.pop()
    return "-".join(w for w in words if w)


def normalize_kind(kind: str) -> str:
    kind = kind.strip().lower()
    if kind in KINDS:
        return kind
    return _KIND_SYNONYMS.get(kind, _KIND_SYNONYMS.get(kind.rstrip("s"), "concept"))


def merge_extractions(parts: list[Extraction]) -> KnowledgeGraph:
    """Dedupe entities across extraction passes (by name or alias), then resolve and dedupe relations."""
    mentions = [e for part in parts for e in part.entities if match_key(e.name)]
    clusters = _cluster(mentions)
    ranked: list[tuple[int, Entity]] = []
    key_to_id: dict[str, str] = {}
    used_ids: set[str] = set()
    for members in clusters:
        entity = _merge_cluster(members, used_ids)
        used_ids.add(entity.id)
        ranked.append((len(members), entity))
        for m in members:
            for key in _keys(m):
                key_to_id.setdefault(key, entity.id)
    # most-mentioned first, then widest coverage; stable sort keeps first-seen order for ties
    ranked.sort(key=lambda pair: (-pair[0], -len(pair[1].source_chunk_ids)))
    entities = [entity for _, entity in ranked]
    relations = _merge_relations([r for part in parts for r in part.relations], key_to_id)
    return KnowledgeGraph(entities=entities, relations=relations)


def _keys(entity: ExtractedEntity) -> list[str]:
    keys = [match_key(entity.name)]
    keys += [k for k in (match_key(a) for a in entity.aliases) if len(k) >= 2]
    return [k for k in keys if k]


def _cluster(mentions: list[ExtractedEntity]) -> list[list[ExtractedEntity]]:
    """Union-find over shared name/alias keys; clusters ordered by first appearance."""
    parent = list(range(len(mentions)))

    def find(i: int) -> int:
        while parent[i] != i:
            parent[i] = parent[parent[i]]
            i = parent[i]
        return i

    owner: dict[str, int] = {}
    for i, mention in enumerate(mentions):
        for key in _keys(mention):
            if key in owner:
                a, b = find(owner[key]), find(i)
                parent[max(a, b)] = min(a, b)
            else:
                owner[key] = i
    clusters: dict[int, list[ExtractedEntity]] = {}
    for i, mention in enumerate(mentions):
        clusters.setdefault(find(i), []).append(mention)
    return [clusters[root] for root in sorted(clusters)]


def _merge_cluster(members: list[ExtractedEntity], used_ids: set[str]) -> Entity:
    name = Counter(m.name.strip() for m in members).most_common(1)[0][0]
    kind = Counter(normalize_kind(m.kind) for m in members).most_common(1)[0][0]
    base = slugify(name) or "entity"
    entity_id, n = base, 2
    while entity_id in used_ids:
        entity_id, n = f"{base}-{n}", n + 1
    return Entity(
        id=entity_id,
        name=name,
        kind=kind,
        summary=_merge_summaries([m.summary for m in members]),
        source_chunk_ids=sorted({i for m in members for i in m.chunk_ids}),
    )


def _merge_summaries(summaries: list[str]) -> str:
    seen: set[str] = set()
    kept: list[str] = []
    for summary in summaries:
        summary = " ".join(summary.split())
        if summary and summary.lower() not in seen:
            seen.add(summary.lower())
            kept.append(summary)
    return _cap(" ".join(kept), SUMMARY_CAP)


def _cap(text: str, limit: int) -> str:
    if len(text) <= limit:
        return text
    cut = text[:limit - 1]
    sentence_end = max(cut.rfind(". "), cut.rfind("! "), cut.rfind("? "))
    if sentence_end >= limit // 2:
        return cut[:sentence_end + 1]
    return cut.rsplit(" ", 1)[0].rstrip(",;:") + "…"


def _merge_relations(raw: list[ExtractedRelation], key_to_id: dict[str, str]) -> list[Relation]:
    merged: dict[tuple[str, str, str], Relation] = {}
    for rel in raw:
        source = key_to_id.get(match_key(rel.source))
        target = key_to_id.get(match_key(rel.target))
        label = " ".join(rel.label.split()).lower()
        if not source or not target or source == target or not label:
            continue
        evidence = _cap(" ".join(rel.evidence.split()), EVIDENCE_CAP)
        key = (source, target, label)
        if key not in merged:
            merged[key] = Relation(source=source, target=target, label=label, evidence=evidence)
        elif not merged[key].evidence:
            merged[key].evidence = evidence
    return list(merged.values())
