"""Persona generation: a planned cast (entity voices + a varied public) filled in by the LLM in batches."""

from __future__ import annotations

import math
import random
import re
import unicodedata
from dataclasses import dataclass
from typing import TypeVar

from pydantic import BaseModel, Field

from tellfold.llm import LLM, LLMError
from tellfold.models import Entity, KnowledgeGraph, Persona

BATCH_SIZE = 10
ENTITY_SHARE = 0.3
EMBODIED_KINDS = ("organization", "person")
HANDLE_MAX = 20
MAX_TRAITS = 5
DIGEST_ENTITIES = 30
DIGEST_RELATIONS = 25

# (starting leaning, weight). Most people hold moderate or no strong view; a minority is vocal.
STANCE_MIX = [
    ("leans supportive, holds the view moderately", 0.21),
    ("leans critical, holds the view moderately", 0.21),
    ("undecided or conflicted, can see both sides", 0.18),
    ("indifferent, barely follows the topic and only reacts if it touches daily life", 0.16),
    ("strongly supportive and vocal about it", 0.12),
    ("strongly critical and vocal about it", 0.12),
]
# (label, activity band). Most accounts mostly read; few post a lot.
ENGAGEMENT_MIX = [
    (("lurker: mostly reads, rarely posts", (0.05, 0.3)), 0.4),
    (("occasional: posts or replies now and then", (0.3, 0.6)), 0.4),
    (("very active: posts and argues often", (0.6, 0.95)), 0.2),
]
AGE_BANDS = ["18-24", "25-34", "35-44", "45-54", "55-64", "65+"]

V = TypeVar("V")


class PersonaDraft(BaseModel):
    slot: str
    handle: str
    name: str
    bio: str
    background: str
    stance: str
    traits: list[str] = Field(default_factory=list)
    activity: float = 0.5
    influence: float = 0.3


class PersonaBatch(BaseModel):
    personas: list[PersonaDraft] = Field(default_factory=list)


class VoicePick(BaseModel):
    entity_ids: list[str] = Field(default_factory=list)  # most relevant first


VOICES_SYSTEM = """\
You choose which organisations and public figures get their own account in a social-media simulation \
about a question. Pick only entities that would plausibly post publicly about this topic, in roughly \
this order: the subject itself (the company or person the question is about), direct competitors, \
journalists, critics, advocacy or professional bodies, regulators, and well-known commentators. \
Leave out background vendors and infrastructure that ordinary people never hear from on this topic: \
cloud hosts, analytics, crash reporting, payment processors, SDK and API providers, app stores and \
platforms, unless the question is specifically about them. Fewer, credible voices beat many odd ones. \
Reply with JSON only."""


@dataclass(frozen=True)
class Slot:
    label: str
    brief: str
    entity_id: str | None = None
    activity_band: tuple[float, float] = (0.0, 1.0)


SYSTEM_PROMPT = """\
You are the casting director for a social-media simulation that forecasts how the public reacts to a \
real-world question. You write believable, specific people and organisational accounts, each with a \
private inner life that will drive how they post, reply, like and argue.

Realism rules:
- Real publics are mostly not experts and mostly not extreme. Respect each slot's assigned stance and \
engagement exactly; do not drift everyone toward strong opinions.
- Make people concrete and different from one another: age, occupation, place, income, family situation, \
values, media habits, writing style. Avoid stereotypes and caricatures; give critics and supporters \
equally intelligent reasons.
- `background` (private, 2-4 sentences): who they are and the personal stakes, interests or experiences \
that explain their stance.
- `stance` (private, 1-2 sentences): their starting attitude toward the question's subject, why, and what \
might change their mind.
- `bio` (public, at most 160 characters): what their profile would actually say, in their own voice.
- `traits`: 2-5 short behavioural descriptors (e.g. "sarcastic", "data-driven", "joins pile-ons").
- `activity` (0-1): how often they act in a given round; stay inside the slot's band.
- `influence` (0-1): reach of the account. Major organisations and public figures 0.6-0.95; ordinary \
people usually 0.05-0.35; local micro-influencers up to 0.5.
- Entity slots speak as that organisation's official account or that public figure, consistent with the \
documented facts; do not invent scandals or positions the material does not support.
- `handle`: lowercase ASCII letters, digits and underscores, at most 20 characters, plausible for the \
person, and different from every handle already taken.
Reply with JSON only."""


async def generate_personas(
    llm: LLM, graph: KnowledgeGraph, *, question: str, n: int, model: str, language: str
) -> list[Persona]:
    if n <= 0:
        return []
    voices = await select_voices(llm, graph, question=question, n=n, model=model)
    slots = plan_slots(graph, question=question, n=n, voices=voices)
    digest = graph_digest(graph)
    filled: dict[str, Persona] = {}
    taken: set[str] = set()
    pending = list(slots)
    last_error = ""
    calls_left = 2 * -(-len(slots) // BATCH_SIZE) + 1  # every batch may be retried once, plus one spare
    while pending and calls_left > 0:
        calls_left -= 1
        batch = pending[:BATCH_SIZE]
        prompt = _batch_prompt(batch, digest, question, language, list(filled.values()))
        try:
            result = await llm.json(prompt, system=SYSTEM_PROMPT, model=model,
                                    purpose="personas.batch", schema=PersonaBatch)
        except LLMError as e:
            last_error = f": {e}"
            continue
        for slot, draft in _match_drafts(batch, result.personas):
            filled[slot.label] = _to_persona(slot, draft, taken)
        pending = [s for s in pending if s.label not in filled]
    if not filled:
        raise LLMError(f"persona generation produced no personas{last_error}")
    return _renumber([filled[s.label] for s in slots if s.label in filled])


async def select_voices(
    llm: LLM, graph: KnowledgeGraph, *, question: str, n: int, model: str
) -> list[Entity] | None:
    """Ask the planner which entities would really speak up about the question.

    Returns None when the call fails, so `plan_slots` falls back to ranking by graph prominence."""
    candidates = _embodiable(graph)
    limit = _entity_limit(n)
    if not candidates or limit == 0:
        return []
    listing = "\n".join(f"- {e.id} [{e.kind}]: {e.name}: {e.summary[:160]}" for e in candidates[:40])
    prompt = (f"Question: {question}\n\nCandidate entities (id [kind]: name: summary):\n{listing}\n\n"
              f"Return up to {limit} entity_ids from the list, most relevant first. Return fewer, or none, "
              "if few would genuinely post about this.")
    try:
        pick = await llm.json(prompt, system=VOICES_SYSTEM, model=model, purpose="personas.cast", schema=VoicePick)
    except LLMError:
        return None
    by_id = {e.id: e for e in candidates}
    chosen: list[Entity] = []
    for entity_id in pick.entity_ids:
        entity = by_id.get(entity_id)
        if entity is not None and entity not in chosen:
            chosen.append(entity)
    return chosen[:limit]


def _entity_limit(n: int) -> int:
    return max(1, round(n * ENTITY_SHARE)) if n >= 2 else 0


def plan_slots(
    graph: KnowledgeGraph, *, question: str, n: int, voices: list[Entity] | None = None
) -> list[Slot]:
    """Decide who the cast is before asking the LLM: which entities get a voice, and the public's mix.

    `voices` is the planner's pick from `select_voices`; None means rank by graph prominence instead."""
    candidates = _embodiable(graph) if voices is None else voices
    n_entity = min(len(candidates), _entity_limit(n))
    slots = [
        Slot(label=agent_id(i, n), entity_id=e.id,
             brief=f"ENTITY VOICE for {e.name} ({e.kind}, id {e.id}): {e.summary or 'see material'}")
        for i, e in enumerate(candidates[:n_entity])
    ]
    n_public = n - n_entity
    rng = random.Random(f"{question}|{n}")
    stances = _allocate(STANCE_MIX, n_public, rng)
    engagement = _allocate(ENGAGEMENT_MIX, n_public, rng)
    ages = [AGE_BANDS[i % len(AGE_BANDS)] for i in range(n_public)]
    rng.shuffle(ages)
    groups = [e.name for e in graph.entities if e.kind == "group"]
    for j in range(n_public):
        eng_label, band = engagement[j]
        segment = groups[j % len(groups)] if groups and j % 2 == 0 else "any affected segment not yet covered"
        brief = (f"ordinary member of the public; age {ages[j]}; part of: {segment}; "
                 f"starting stance: {stances[j]}; engagement: {eng_label} "
                 f"(activity {band[0]:.2f}-{band[1]:.2f})")
        slots.append(Slot(label=agent_id(n_entity + j, n), brief=brief, activity_band=band))
    return slots


def graph_digest(graph: KnowledgeGraph) -> str:
    names = {e.id: e.name for e in graph.entities}
    lines = [f"- {e.name} [{e.kind}]: {e.summary}" for e in graph.entities[:DIGEST_ENTITIES]]
    rels = [f"- {names[r.source]} {r.label} {names[r.target]}"
            for r in graph.relations if r.source in names and r.target in names][:DIGEST_RELATIONS]
    parts = ["Key entities:", *(lines or ["- (none extracted)"])]
    if rels:
        parts += ["Relations:", *rels]
    return "\n".join(parts)


def _embodiable(graph: KnowledgeGraph) -> list[Entity]:
    """Organizations and people, most-connected and most-mentioned first."""
    degree: dict[str, int] = {}
    for r in graph.relations:
        degree[r.source] = degree.get(r.source, 0) + 1
        degree[r.target] = degree.get(r.target, 0) + 1
    picks = [e for e in graph.entities if e.kind in EMBODIED_KINDS]
    return sorted(picks, key=lambda e: -(degree.get(e.id, 0) + len(e.source_chunk_ids)))


def _allocate(mix: list[tuple[V, float]], count: int, rng: random.Random) -> list[V]:
    """Largest-remainder apportionment of `count` items over weighted categories, then shuffled."""
    total = sum(w for _, w in mix)
    quotas = [count * w / total for _, w in mix]
    counts = [int(q) for q in quotas]
    by_remainder = sorted(range(len(mix)), key=lambda i: quotas[i] - counts[i], reverse=True)
    for i in by_remainder[: count - sum(counts)]:
        counts[i] += 1
    items = [value for (value, _), c in zip(mix, counts) for _ in range(c)]
    rng.shuffle(items)
    return items


def _batch_prompt(batch: list[Slot], digest: str, question: str, language: str,
                  existing: list[Persona]) -> str:
    slot_lines = "\n".join(f"- slot {s.label}: {s.brief}" for s in batch)
    taken = "\n".join(f"- @{p.handle} ({p.name}): {p.bio[:70]}" for p in existing) or "- (none yet)"
    return f"""\
Question being simulated:
{question}

Background material (from the seed documents):
{digest}

Accounts already created (do not reuse these handles or near-duplicate these people):
{taken}

Write exactly one persona for each slot below, in the same order, honouring every slot's brief:
{slot_lines}

Write `name`, `bio`, `background`, `stance` and `traits` in {language}; pick names that fit the setting \
of the material. Handles stay lowercase ASCII.

Return a JSON object:
{{"personas": [{{"slot": slot id, "handle": str, "name": str, "bio": str, "background": str,
  "stance": str, "traits": [str], "activity": float, "influence": float}}]}}"""


def _match_drafts(batch: list[Slot], drafts: list[PersonaDraft]) -> list[tuple[Slot, PersonaDraft]]:
    """Pair drafts with slots by echoed slot id, then hand leftovers to unclaimed slots in order."""
    by_label = {s.label: s for s in batch}
    pairs: dict[str, PersonaDraft] = {}
    leftovers: list[PersonaDraft] = []
    for draft in drafts:
        label = draft.slot.strip()
        if label in by_label and label not in pairs:
            pairs[label] = draft
        else:
            leftovers.append(draft)
    for slot in batch:
        if slot.label not in pairs and leftovers:
            pairs[slot.label] = leftovers.pop(0)
    return [(s, pairs[s.label]) for s in batch if s.label in pairs]


def _to_persona(slot: Slot, draft: PersonaDraft, taken: set[str]) -> Persona:
    handle = unique_handle(draft.handle, draft.name, taken)
    taken.add(handle)
    low, high = slot.activity_band
    traits = [t.strip() for t in draft.traits if t.strip()][:MAX_TRAITS]
    return Persona(
        id=slot.label,
        handle=handle,
        name=draft.name.strip() or handle,
        bio=draft.bio.strip(),
        background=draft.background.strip(),
        stance=draft.stance.strip(),
        traits=traits,
        activity=round(_clamp(draft.activity, low, high), 3),
        influence=round(_clamp(draft.influence, 0.0, 1.0), 3),
        entity_id=slot.entity_id,
    )


def unique_handle(raw: str, name: str, taken: set[str]) -> str:
    """Sanitize to [a-z0-9_]{1,20}; derive from the name if needed; suffix a number on collisions."""
    base = _handle_base(raw) or _handle_base(name) or "user"
    candidate, n = base, 2
    while candidate in taken:
        suffix = f"_{n}"
        candidate, n = base[: HANDLE_MAX - len(suffix)] + suffix, n + 1
    return candidate


def _handle_base(text: str) -> str:
    text = unicodedata.normalize("NFKD", text.lstrip("@"))
    text = text.encode("ascii", "ignore").decode().lower()
    text = re.sub(r"[^a-z0-9_]+", "_", text)
    return re.sub(r"_+", "_", text).strip("_")[:HANDLE_MAX].rstrip("_")


def _clamp(value: float, low: float, high: float) -> float:
    if math.isnan(value):
        return low
    return min(high, max(low, value))


def agent_id(index: int, total: int) -> str:
    """Zero-based index -> "a01".. (wider padding once there are 100+ agents)."""
    return f"a{index + 1:0{max(2, len(str(total)))}d}"


def _renumber(personas: list[Persona]) -> list[Persona]:
    """Guarantee contiguous ids a01..aNN even if some slots could not be filled."""
    return [p.model_copy(update={"id": agent_id(i, len(personas))}) for i, p in enumerate(personas)]
