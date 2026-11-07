"""The round loop: who is active, what they see, what they decide, and how the world changes."""

from __future__ import annotations

import asyncio
import random
import time
from collections import Counter
from dataclasses import dataclass, field
from typing import Callable

from pydantic import BaseModel, Field, field_validator

from shoal.llm import LLM
from shoal.models import (
    Action,
    ActionType,
    AgentMemory,
    GodEvent,
    KnowledgeGraph,
    Persona,
    Post,
    RoundStats,
    SimConfig,
)
from shoal.world import World

EventCallback = Callable[[str, dict], None]

MAX_ACTIONS_PER_AGENT = 3
MAX_NOTE_CHARS = 280
OPENING_SHARE = 0.3  # fraction of agents that write a round-0 opening post
EVENT_ACTIVITY_BOOST = 0.15  # breaking news pulls lurkers in

_ACTION_ALIASES = {
    "comment": "reply",
    "respond": "reply",
    "share": "repost",
    "retweet": "repost",
    "quote": "repost",
    "upvote": "like",
    "downvote": "dislike",
    "none": "nothing",
    "skip": "nothing",
    "lurk": "nothing",
}


# ---------- LLM response schemas ----------


class ProposedAction(BaseModel):
    type: str = Field(description="one of: post, reply, repost, like, dislike, follow, nothing")
    content: str = Field("", description="text for post / reply, optional comment for repost")
    target_post_id: int | None = Field(None, description="post id from THIS user's feed (reply/repost/like/dislike)")
    target_agent: str | None = Field(None, description="agent id or handle of an author in this user's feed (follow)")
    reasoning: str = Field("", description="private one-line motive")

    @field_validator("target_post_id", mode="before")
    @classmethod
    def _strip_hash(cls, v: object) -> object:
        if isinstance(v, str):
            digits = v.strip().lstrip("#")
            return int(digits) if digits.isdigit() else None
        return v


class AgentDecision(BaseModel):
    agent_id: str
    actions: list[ProposedAction] = Field(default_factory=list, description="1 to 3 actions")
    memory_note: str = Field("", description="first-person note: what stood out, current view on the topic")


class RoundDecisions(BaseModel):
    decisions: list[AgentDecision]


class OpeningPost(BaseModel):
    agent_id: str
    content: str
    memory_note: str = ""


class OpeningPosts(BaseModel):
    posts: list[OpeningPost]


class PostScore(BaseModel):
    post_id: int
    score: float = Field(description="-1 (strongly negative) .. 1 (strongly positive)")


class SentimentScores(BaseModel):
    scores: list[PostScore]


# ---------- prompts ----------

_ROUND_SYSTEM = """You drive several independent users of a social media platform in a social simulation.
Each user is a distinct person or organisation with their own background, interests, stance and voice. For
every user listed you decide what they do during this round, after they have scrolled their own feed.

Behave like a real platform, not a debate club:
- Most people mostly read. The most common outcome is doing nothing, or a single like. Replies come when
  something provokes, worries or excites a user. A brand-new original post is rarer and comes mainly from
  highly active users, organisations, or people with a strong reaction to fresh news.
- Respect each user's activity level: a low-activity user should usually do nothing or just like something.
- Users react to what is actually in their feed. They may agree, push back, joke, ask questions, share
  personal experience, or ignore the topic entirely.
- Opinions can move. Repeated exposure, a persuasive argument, social proof or breaking news may soften or
  harden a user's view; stubborn users barely move. Shifts are gradual and must fit the person.
- Voices must differ: vary length, tone, vocabulary and certainty. Organisations speak in an official or PR
  register. Avoid hashtag spam, slogans, and everybody sounding alike. Posts stay under 280 characters.
- Never invent facts presented as certain unless the user would plausibly believe or spread them.

Hard rules:
- A user may only reference post ids shown in THAT user's own feed (reply, repost, like, dislike).
- A user can like or dislike a post only once: never like or dislike a post marked as already liked or disliked.
- A user may only follow an author who appears in their own feed, and never themselves.
- Give each user 1 to 3 actions. Use "nothing" when they just scroll past.
- Write all post, reply and memory text in {language}. Keep JSON keys, action types and agent ids exactly as specified.

Reply with JSON only, shaped like:
{{"decisions": [{{"agent_id": "a03", "actions": [{{"type": "like", "target_post_id": 12, "reasoning": "..."}},
{{"type": "reply", "target_post_id": 12, "content": "...", "reasoning": "..."}}],
"memory_note": "..."}}]}}
Action types: post (content), reply (target_post_id, content), repost (target_post_id, optional content),
like / dislike (target_post_id), follow (target_agent), nothing.
memory_note: one first-person sentence for the user's own memory: what stood out this round and where their
view on the topic now stands (unchanged, softened, hardened, or newly formed)."""

_OPENING_SYSTEM = """You write the first posts of a social simulation, on a platform where the conversation about a
topic is just starting. Each listed user writes ONE original post in their own voice, grounded in their
background and starting stance and in the situation described.

- Make the posts differ in angle, length, tone and certainty; organisations speak officially, individuals
  personally. Some users are passionate, some casual, some only tangentially interested.
- Under 280 characters each, no hashtag spam, no two posts making the same point the same way.
- Write all post and memory text in {language}. Keep agent ids exactly as given.

Reply with JSON only, shaped like:
{{"posts": [{{"agent_id": "a01", "content": "...", "memory_note": "..."}}]}}
memory_note: one first-person sentence about why they posted and how they currently feel about the topic."""

_SENTIMENT_SYSTEM = """You are a careful annotator of social media posts. For each post, rate the attitude it
expresses toward the subject of the question below, from -1 (strongly negative, pessimistic or opposed)
through 0 (neutral, mixed or off-topic) to +1 (strongly positive, optimistic or supportive). A plain repost
endorses the original post. Judge the author's own attitude, not the topic's mood in general.

Reply with JSON only: {"scores": [{"post_id": 12, "score": -0.4}]} with one entry per post."""


# ---------- engine ----------


@dataclass
class _Ledger:
    """LLM usage for one round."""

    calls: int = 0
    seconds: float = 0.0


@dataclass
class _AgentView:
    """What one agent is shown this round, and therefore what it may reference."""

    persona: Persona
    feed: list[Post]
    allowed_posts: set[int] = field(default_factory=set)
    visible_authors: set[str] = field(default_factory=set)


class Engine:
    def __init__(
        self, llm: LLM, world: World, config: SimConfig, *, on_event: EventCallback | None = None
    ):
        self.llm = llm
        self.world = world
        self.config = config
        self._emit = on_event or (lambda name, payload: None)

    # ---------- seeding ----------

    async def seed_world(self, graph: KnowledgeGraph) -> None:
        personas = self.world.personas()
        if not personas:
            raise ValueError("seed_world needs personas in the world")
        self._emit("stage", {"stage": "follow_graph"})
        edges = build_follow_graph(personas, graph, seed=self.config.seed)
        for follower, followee in edges:
            self.world.add_follow(follower, followee)
        self._emit("stage", {"stage": "follow_graph_done", "edges": len(edges)})

        events = self._inject_events(0)
        authors = pick_opening_authors(personas, seed=self.config.seed)
        self._emit("stage", {"stage": "opening_posts", "authors": len(authors)})
        ledger = _Ledger()
        batches = _chunks(authors, self.config.batch_size)
        context = _graph_digest(graph)
        results = await asyncio.gather(
            *(self._opening_batch(i, batch, context, events, ledger) for i, batch in enumerate(batches))
        )
        posts = sorted((p for r in results if r for p in r), key=lambda p: p.agent_id)
        allowed = {p.id for p in authors}
        written: set[str] = set()
        post_ids: list[int] = []
        for op in posts:
            if op.agent_id not in allowed or op.agent_id in written:
                continue
            written.add(op.agent_id)
            action = Action(agent_id=op.agent_id, type=ActionType.POST, content=op.content)
            post_id = self.world.apply(0, action)
            if post_id is not None:
                post_ids.append(post_id)
            self._remember(op.agent_id, 0, op.memory_note)
        if post_ids:
            self._emit("posts_added", {"round": 0, "post_ids": post_ids})
        new_posts = self.world.posts(0)
        stats = RoundStats(
            round=0,
            actions=self._action_counts(0),
            new_posts=len(new_posts),
            active_agents=len(authors),
            sentiment=await self._sentiment(0, new_posts, ledger),
            llm_calls=ledger.calls,
            llm_seconds=round(ledger.seconds, 2),
        )
        self.world.save_round_stats(stats)
        self._emit("stage", {"stage": "opening_posts_done", "posts": len(new_posts)})
        self._emit("round_done", {**stats.model_dump(mode="json"), "total_rounds": self.config.rounds})

    async def _opening_batch(
        self, index: int, batch: list[Persona], context: str, events: list[GodEvent], ledger: _Ledger
    ) -> list[OpeningPost] | None:
        prompt = "\n\n".join(
            [
                f"Question being simulated: {self.config.question}",
                f"Situation (from the source material):\n{context}" if context else "",
                _news_block(events, 0),
                "Users who post now:",
                *(_persona_block(p, self.world.memory(p.id)) for p in batch),
            ]
        )
        try:
            system = _OPENING_SYSTEM.format(language=self.config.language)
            out = await self._call(prompt, system, "engine.seed", OpeningPosts, ledger)
        except Exception as e:  # noqa: BLE001 - one failed batch must not stop the run
            self._emit("batch_failed", {"round": 0, "batch": index, "agents": [p.id for p in batch], "error": str(e)})
            return None
        return out.posts

    # ---------- rounds ----------

    async def run(self) -> list[RoundStats]:
        results = []
        for r in range(1, self.config.rounds + 1):
            results.append(await self.run_round(r))
        return results

    async def run_round(self, r: int) -> RoundStats:
        events = self._inject_events(r)
        personas = self.world.personas()
        active = sample_active(personas, r, seed=self.config.seed, news=any(e.round == r for e in events))
        batches = _chunks(active, self.config.batch_size)
        self._emit(
            "round_start",
            {
                "round": r,
                "active": len(active),
                "total_rounds": self.config.rounds,
                "active_agents": [p.id for p in active],
                "batches": len(batches),
                "events": [e.content for e in events if e.round == r],
            },
        )
        ledger = _Ledger()
        views = {p.id: self._view(p, r) for p in active}
        tasks = [
            asyncio.ensure_future(self._decide_batch(r, i, [views[p.id] for p in batch], events, ledger))
            for i, batch in enumerate(batches)
        ]
        handles = self._handle_index()
        new_posts = 0
        try:
            # Batches decide concurrently but are applied in batch order (agent id order inside a batch), so
            # the result is deterministic while posts still stream out as soon as the earlier batches are in.
            for task in tasks:
                decisions = {d.agent_id: d for d in (await task or [])}
                post_ids: list[int] = []
                for agent_id in sorted(decisions):
                    decision = decisions[agent_id]
                    proposed = decision.actions[:MAX_ACTIONS_PER_AGENT] or [ProposedAction(type="nothing")]
                    for p in proposed:
                        post_id = self.world.apply(r, _to_action(views[agent_id], p, handles))
                        if post_id is not None:
                            post_ids.append(post_id)
                    self._remember(agent_id, r, decision.memory_note)
                new_posts += len(post_ids)
                if post_ids:
                    self._emit("posts_added", {"round": r, "post_ids": post_ids})
        finally:
            for task in tasks:
                task.cancel()

        stats = RoundStats(
            round=r,
            actions=self._action_counts(r),
            new_posts=new_posts,
            active_agents=len(active),
            sentiment=await self._sentiment(r, self.world.posts(r), ledger),
            llm_calls=ledger.calls,
            llm_seconds=round(ledger.seconds, 2),
        )
        self.world.save_round_stats(stats)
        self._emit("round_done", {**stats.model_dump(mode="json"), "total_rounds": self.config.rounds})
        return stats

    def _view(self, persona: Persona, r: int) -> _AgentView:
        feed = self.world.feed_for(persona.id, r, self.config.feed_size)
        view = _AgentView(persona=persona, feed=feed)
        for post in feed:
            view.allowed_posts.add(post.id)
            view.visible_authors.add(post.author_id)
            for ref in (post.parent_id, post.repost_of):
                if ref is not None:
                    shown = self.world.post(ref)
                    view.allowed_posts.add(shown.id)
                    view.visible_authors.add(shown.author_id)
        return view

    async def _decide_batch(
        self, r: int, index: int, views: list[_AgentView], events: list[GodEvent], ledger: _Ledger
    ) -> list[AgentDecision] | None:
        prompt = self._round_prompt(r, views, events)
        try:
            system = _ROUND_SYSTEM.format(language=self.config.language)
            out = await self._call(prompt, system, "engine.round", RoundDecisions, ledger)
        except Exception as e:  # noqa: BLE001 - one failed batch must not stop the run
            self._emit(
                "batch_failed",
                {"round": r, "batch": index, "agents": [v.persona.id for v in views], "error": str(e)},
            )
            return None
        expected = {v.persona.id for v in views}
        seen: set[str] = set()
        decisions = []
        for d in out.decisions:
            if d.agent_id in expected and d.agent_id not in seen:
                seen.add(d.agent_id)
                decisions.append(d)
        return decisions

    def _round_prompt(self, r: int, views: list[_AgentView], events: list[GodEvent]) -> str:
        handles = {p.id: p.handle for p in self.world.personas()}
        following = {v.persona.id: set(self.world.following(v.persona.id)) for v in views}
        blocks = [
            f"Question being simulated: {self.config.question}",
            f"Round {r} of {self.config.rounds}.",
            _news_block(events, r),
            f"Users to decide for: {len(views)}. Each block holds one user's private profile, memory and own feed.",
        ]
        for v in views:
            p = v.persona
            blocks.append(
                "\n".join(
                    [
                        _persona_block(p, self.world.memory(p.id)),
                        f"Follows {len(following[p.id])} accounts, followed by {len(self.world.followers(p.id))}.",
                        _own_posts_block(self.world, p.id, r),
                        _feed_block(self.world, v.feed, handles, following[p.id], self.world.engagement_by(p.id)),
                    ]
                )
            )
        blocks.append(f"Return exactly one decision per user: {', '.join(v.persona.id for v in views)}.")
        return "\n\n".join(b for b in blocks if b)

    def _action_counts(self, r: int) -> dict[str, int]:
        counts: dict[str, int] = {}
        for _, action in self.world.actions(r):
            counts[action.type.value] = counts.get(action.type.value, 0) + 1
        return counts

    def _handle_index(self) -> dict[str, str]:
        index = {}
        for p in self.world.personas():
            index[p.id] = p.id
            index[p.handle.lower()] = p.id
        return index

    # ---------- shared helpers ----------

    async def _call(self, prompt: str, system: str, purpose: str, schema: type[BaseModel], ledger: _Ledger):
        started = time.monotonic()
        try:
            return await self.llm.json(
                prompt,
                system=system,
                model=self.config.agent_model,
                purpose=purpose,
                schema=schema,
            )
        finally:
            ledger.calls += 1
            ledger.seconds += time.monotonic() - started

    async def _sentiment(self, r: int, posts: list[Post], ledger: _Ledger) -> float | None:
        if not posts:
            return None
        lines = []
        for post in posts:
            text = post.content or "(no comment)"
            if post.repost_of is not None:
                text = f"[repost of: {self.world.post(post.repost_of).content}] {post.content}".strip()
            elif post.parent_id is not None:
                text = f"[reply to: {_clip(self.world.post(post.parent_id).content, 140)}] {text}"
            lines.append(f"#{post.id}: {text}")
        prompt = f"Question: {self.config.question}\n\nPosts from round {r}:\n" + "\n".join(lines)
        try:
            out = await self._call(prompt, _SENTIMENT_SYSTEM, "engine.sentiment", SentimentScores, ledger)
        except Exception as e:  # noqa: BLE001 - sentiment is optional
            self._emit("batch_failed", {"round": r, "batch": "sentiment", "agents": [], "error": str(e)})
            return None
        wanted = {p.id for p in posts}
        per_post = {s.post_id: max(-1.0, min(1.0, s.score)) for s in out.scores if s.post_id in wanted}
        self.world.save_post_sentiment(per_post)
        scores = list(per_post.values())
        return round(sum(scores) / len(scores), 3) if scores else None

    def _inject_events(self, r: int) -> list[GodEvent]:
        """Store this round's god events and return every event up to and including round r.

        Events may also be added straight to the world while the run is in progress (from the UI); the world
        is re-read every round, and any event for round r that is not one of `config.events` is announced
        with `event_injected`.
        """
        planned = [e.content for e in self.config.events if e.round == r]
        for content in planned:
            self.world.add_event(GodEvent(round=r, content=content))
        events = [e for e in self.world.events() if e.round <= r]
        remaining = Counter(planned)
        for event in (e for e in events if e.round == r):
            if remaining[event.content] > 0:
                remaining[event.content] -= 1
            else:
                self._emit("event_injected", {"round": r, "content": event.content})
        return events

    def _remember(self, agent_id: str, r: int, note: str) -> None:
        note = " ".join(note.split())
        if not note:
            return
        mem = self.world.memory(agent_id)
        notes = [*mem.notes, f"R{r}: {_clip(note, MAX_NOTE_CHARS)}"][-self.config.memory_size :]
        self.world.save_memory(AgentMemory(agent_id=agent_id, notes=notes))


# ---------- pure helpers (seeded, deterministic) ----------


def sample_active(personas: list[Persona], r: int, *, seed: int, news: bool = False) -> list[Persona]:
    """Each agent acts with probability `activity` (boosted on news rounds); never an empty round."""
    rng = random.Random(f"shoal:active:{seed}:{r}")
    boost = EVENT_ACTIVITY_BOOST if news else 0.0
    ordered = sorted(personas, key=lambda p: p.id)
    active = [p for p in ordered if rng.random() < min(1.0, p.activity + boost)]
    if not active and ordered:
        active = rng.choices(ordered, weights=[p.activity + 0.01 for p in ordered], k=1)
    rng.shuffle(active)  # mix batch composition so the same agents are not always decided together
    return active


def pick_opening_authors(personas: list[Persona], *, seed: int) -> list[Persona]:
    """Opening posts come mostly from entity accounts and the most active, influential people."""
    rng = random.Random(f"shoal:opening:{seed}")
    m = max(1, round(len(personas) * OPENING_SHARE))

    def weight(p: Persona) -> float:
        return (0.1 + p.activity + p.influence) * (2.0 if p.entity_id else 1.0)

    chosen = _weighted_sample(sorted(personas, key=lambda p: p.id), weight, m, rng)
    return sorted(chosen, key=lambda p: p.id)


def build_follow_graph(personas: list[Persona], graph: KnowledgeGraph, *, seed: int) -> list[tuple[str, str]]:
    """Influence-weighted preferential attachment with a bit of homophily.

    Entity accounts attract more followers but follow few. Accounts whose entities are related in the
    knowledge graph tend to follow each other; people sharing traits do too.
    """
    rng = random.Random(f"shoal:follows:{seed}")
    ordered = sorted(personas, key=lambda p: p.id)
    related = {(r.source, r.target) for r in graph.relations} | {(r.target, r.source) for r in graph.relations}
    edges: list[tuple[str, str]] = []
    for follower in ordered:
        others = [p for p in ordered if p.id != follower.id]
        if not others:
            break
        wanted = 1 + round(follower.activity * 3) if follower.entity_id else 2 + round(follower.activity * 6)

        def weight(t: Persona, f: Persona = follower) -> float:
            w = (0.15 + t.influence) ** 2 * (2.5 if t.entity_id else 1.0)
            if f.entity_id and t.entity_id and (f.entity_id, t.entity_id) in related:
                w *= 3.0
            shared = len({x.lower() for x in f.traits} & {x.lower() for x in t.traits})
            return w * (1.0 + 0.5 * shared)

        for target in _weighted_sample(others, weight, min(wanted, len(others)), rng):
            edges.append((follower.id, target.id))
    return edges


def _weighted_sample(items: list, weight: Callable, k: int, rng: random.Random) -> list:
    """Weighted sampling without replacement (Efraimidis-Spirakis keys)."""
    keyed = [(rng.random() ** (1.0 / max(weight(x), 1e-6)), i) for i, x in enumerate(items)]
    keyed.sort(reverse=True)
    return [items[i] for _, i in keyed[:k]]


def _to_action(view: _AgentView, p: ProposedAction, handles: dict[str, str]) -> Action:
    """Convert one proposal, downgrading anything outside this agent's own view to `nothing`."""
    agent_id = view.persona.id
    kind = p.type.strip().lower()
    kind = _ACTION_ALIASES.get(kind, kind)
    content = p.content.strip()

    def nothing(problem: str) -> Action:
        return Action(agent_id=agent_id, type=ActionType.NOTHING, reasoning=f"[invalid {kind}: {problem}] {p.reasoning}".strip())

    if kind not in {t.value for t in ActionType}:
        return nothing("unknown action type")
    action_type = ActionType(kind)
    if action_type in (ActionType.REPLY, ActionType.REPOST, ActionType.LIKE, ActionType.DISLIKE):
        if p.target_post_id not in view.allowed_posts:
            return nothing(f"post {p.target_post_id} not in own feed")
    if action_type is ActionType.FOLLOW:
        raw = (p.target_agent or "").strip().lstrip("@").lower()
        target = handles.get(raw)
        if target is None or target not in view.visible_authors:
            return nothing(f"follow target {p.target_agent!r} not in own feed")
        return Action(agent_id=agent_id, type=action_type, target_agent_id=target, reasoning=p.reasoning)
    return Action(
        agent_id=agent_id,
        type=action_type,
        content=content if action_type in (ActionType.POST, ActionType.REPLY, ActionType.REPOST) else "",
        target_post_id=p.target_post_id if action_type is not ActionType.POST else None,
        reasoning=p.reasoning,
    )


def _chunks(items: list, size: int) -> list[list]:
    size = max(1, size)
    return [items[i : i + size] for i in range(0, len(items), size)]


def _clip(text: str, limit: int) -> str:
    text = " ".join(text.split())
    return text if len(text) <= limit else text[: limit - 1].rstrip() + "…"


def _activity_label(activity: float) -> str:
    if activity < 0.3:
        return "low (mostly lurks)"
    if activity < 0.6:
        return "medium (reads a lot, chimes in now and then)"
    return "high (posts and replies often)"


def _persona_block(p: Persona, memory: AgentMemory) -> str:
    lines = [
        f"=== {p.id} @{p.handle} ===",
        f"Name: {p.name}" + (" (organisation / public account)" if p.entity_id else ""),
        f"Public bio: {p.bio}",
        f"Private background: {p.background}",
        f"Starting stance on the topic: {p.stance}",
        f"Traits: {', '.join(p.traits) if p.traits else 'none noted'}",
        f"Activity level: {_activity_label(p.activity)}",
    ]
    if memory.notes:
        lines.append("Memory (oldest first):")
        lines.extend(f"- {n}" for n in memory.notes)
    return "\n".join(lines)


def _own_posts_block(world: World, agent_id: str, r: int, limit: int = 3) -> str:
    own = [p for p in world.posts() if p.author_id == agent_id and p.round < r][-limit:]
    if not own:
        return ""
    lines = ["Your recent posts (for consistency; you cannot react to these):"]
    lines.extend(
        f"- round {p.round}: {_clip(p.content or '(plain repost)', 160)} "
        f"[likes {p.likes}, dislikes {p.dislikes}, reposts {p.reposts}, replies {p.replies}]"
        for p in own
    )
    return "\n".join(lines)


def _feed_block(
    world: World,
    feed: list[Post],
    handles: dict[str, str],
    following: set[str],
    engaged: dict[int, list[str]] | None = None,
) -> str:
    engaged = engaged or {}
    if not feed:
        return "Feed: (empty - nothing to react to yet; only post, follow nobody, or do nothing)"
    lines = ["Feed (the only posts this user may reference):"]
    for post in feed:
        author = f"@{handles.get(post.author_id, post.author_id)} ({post.author_id})"
        if post.author_id in following:
            author += " [you follow]"
        done = f" [you already {' and '.join(engaged[post.id])} this]" if post.id in engaged else ""
        lines.append(
            f"[#{post.id}] {author}, round {post.round} - likes {post.likes}, dislikes {post.dislikes}, "
            f"reposts {post.reposts}, replies {post.replies}{done}"
        )
        if post.repost_of is not None:
            original = world.post(post.repost_of)
            if post.content:
                lines.append(f"  {_clip(post.content, 280)}")
            lines.append(
                f"  reposted [#{original.id}] @{handles.get(original.author_id, original.author_id)} "
                f"({original.author_id}): {_clip(original.content, 280)}"
            )
            continue
        if post.parent_id is not None:
            parent = world.post(post.parent_id)
            lines.append(
                f"  replying to [#{parent.id}] @{handles.get(parent.author_id, parent.author_id)} "
                f"({parent.author_id}): {_clip(parent.content, 140)}"
            )
        lines.append(f"  {_clip(post.content, 280)}")
    return "\n".join(lines)


def _news_block(events: list[GodEvent], r: int) -> str:
    current = [e.content for e in events if e.round == r]
    earlier = [e.content for e in events if e.round < r]
    lines = []
    if current:
        lines.append("BREAKING NEWS this round (every user has just seen this):")
        lines.extend(f"- {c}" for c in current)
    if earlier:
        lines.append("Earlier news:")
        lines.extend(f"- {c}" for c in earlier)
    return "\n".join(lines)


def _graph_digest(graph: KnowledgeGraph, limit: int = 25) -> str:
    names = {e.id: e.name for e in graph.entities}
    lines = [f"- {e.name} ({e.kind}): {_clip(e.summary, 160)}" for e in graph.entities[:limit]]
    lines.extend(
        f"- {names[r.source]} {r.label} {names[r.target]}"
        for r in graph.relations[:limit]
        if r.source in names and r.target in names
    )
    return "\n".join(lines)


