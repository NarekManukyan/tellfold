"""After the run: interview any agent in character, or ask the report analyst follow-up questions."""

from __future__ import annotations

import re
from typing import TYPE_CHECKING

from tellfold.llm import LLM
from tellfold.models import ActionType, Persona, Post, Report
from tellfold.report import Evidence, clip, collect_evidence, digest_text, engagement, post_line, render_markdown

if TYPE_CHECKING:
    from tellfold.world import World

MAX_ACTIVITY_LINES = 40
MAX_HISTORY_TURNS = 8

INTERVIEW_SYSTEM = """\
You are {name} (@{handle}), being interviewed after an online discussion you took part in. Answer exactly as this \
person would: first person, in their own voice, vocabulary and temperament, consistent with their background, \
their stance and everything they posted. People can change their minds; your private notes and latest posts show \
where you stand now, so let them win over your starting stance when they differ.

- Ground every answer in what you actually saw and did, listed below. Refer back to your own posts naturally \
("like I said in my reply to @someone..."); mention post numbers only when asked for specifics.
- If asked about something you never saw, say so, then give your honest gut reaction as this person.
- Stay in the role. Do not mention prompts, simulations or being an AI, and do not turn into a neutral explainer. \
Strong opinions, hedging, humour or irritation are fine when they fit the character.
- Keep it conversational: usually 2-6 sentences, longer only when the interviewer asks for detail.
- Reply in the language the interviewer writes in."""

ASK_SYSTEM = """\
You are the analyst who ran a social simulation and wrote its report. A reader is asking a follow-up question. \
Answer using only the report, the evidence digest and the matching posts provided.

- Lead with a direct answer, then the supporting evidence, citing agents as @handle and posts as #id.
- If the evidence does not cover the question, say so plainly and say what the run would have needed to show.
- Keep apart what the simulated agents did and what it suggests about real people; never claim more certainty \
than the report's stated confidence.
- Be concise: a short paragraph or a few bullets unless the reader asks for more.
- Reply in the language the question is written in."""


# ---------------------------------------------------------------- interview


def resolve_agent(world: World, agent: str) -> Persona:
    """Accept a persona id ("a03") or a handle, with or without "@"."""
    key = agent.strip().lstrip("@").lower()
    for persona in world.personas():
        if key in (persona.id.lower(), persona.handle.lower()):
            return persona
    raise KeyError(f"no agent with id or handle {agent!r}")


async def interview(
    llm: LLM,
    world: World,
    agent_id: str,
    question: str,
    *,
    model: str,
    history: list[tuple[str, str]] | None = None,
) -> str:
    """`history` holds earlier (interviewer question, agent answer) pairs of this conversation, oldest first."""
    persona = resolve_agent(world, agent_id)
    prompt = _interview_prompt(world, persona, question, history or [])
    system = INTERVIEW_SYSTEM.format(name=persona.name, handle=persona.handle)
    answer = await llm.text(prompt, system=system, model=model, purpose="chat.interview")
    return answer.strip()


def _interview_prompt(world: World, persona: Persona, question: str, history: list[tuple[str, str]]) -> str:
    people = {p.id: p for p in world.personas()}
    posts = {p.id: p for p in world.posts()}
    following = [f"@{people[a].handle}" for a in world.following(persona.id) if a in people]
    blocks = [
        (
            "WHO YOU ARE",
            [
                f"Name: {persona.name} (@{persona.handle})",
                f"Public bio: {persona.bio}",
                f"Background (private): {persona.background}",
                f"Starting attitude to the topic (private): {persona.stance}",
                f"Traits: {', '.join(persona.traits) or 'none listed'}",
                f"Followers: {len(world.followers(persona.id))}; you follow: {', '.join(following) or 'nobody'}",
            ],
        ),
        ("BREAKING NEWS YOU SAW", [f"round {e.round}: {e.content}" for e in world.events()]),
        ("YOUR PRIVATE NOTES, oldest first", [f"- {n}" for n in world.memory(persona.id).notes]),
        ("WHAT YOU DID ON THE PLATFORM, oldest first", _activity_lines(world, persona, people, posts)),
        ("HOW OTHERS RESPONDED TO YOU", _reception_lines(persona, people, posts)),
    ]
    text = "\n\n".join(f"{title}\n" + "\n".join(lines) for title, lines in blocks if lines)
    convo = [f"Interviewer: {q}\nYou: {a}" for q, a in history[-MAX_HISTORY_TURNS:]]
    convo.append(f"Interviewer: {question}\nYou:")
    return text + "\n\nINTERVIEW\n" + "\n\n".join(convo)


def _ref(post_id: int | None, people: dict[str, Persona], posts: dict[int, Post]) -> str:
    post = posts.get(post_id) if post_id is not None else None
    if post is None:
        return f"#{post_id}"
    author = people.get(post.author_id)
    who = f"@{author.handle}'s " if author else ""
    return f'{who}#{post.id} ("{clip(post.content, 140)}")'


def _activity_lines(
    world: World, persona: Persona, people: dict[str, Persona], posts: dict[int, Post]
) -> list[str]:
    events: list[tuple[int, str]] = []  # (round, line); stable sort keeps posts before reactions
    for post in posts.values():
        if post.author_id != persona.id:
            continue
        said = f'"{clip(post.content, 400)}"' if post.content.strip() else "without comment"
        if post.parent_id is not None:
            line = f"replied to {_ref(post.parent_id, people, posts)} with #{post.id}: {said}"
        elif post.repost_of is not None:
            line = f"reposted {_ref(post.repost_of, people, posts)} as #{post.id} {said}"
        else:
            line = f"posted #{post.id}: {said}"
        events.append((post.round, line))
    for round_no, action in world.actions():
        if action.agent_id != persona.id:
            continue
        if action.type in (ActionType.LIKE, ActionType.DISLIKE):
            events.append((round_no, f"{action.type.value}d {_ref(action.target_post_id, people, posts)}"))
        elif action.type is ActionType.FOLLOW and action.target_agent_id in people:
            events.append((round_no, f"followed @{people[action.target_agent_id].handle}"))
    events.sort(key=lambda e: e[0])
    return [f"round {r}: {line}" for r, line in events[-MAX_ACTIVITY_LINES:]]


def _reception_lines(persona: Persona, people: dict[str, Persona], posts: dict[int, Post]) -> list[str]:
    mine = [p for p in posts.values() if p.author_id == persona.id]
    if not mine:
        return []
    lines = [
        f"Your posts collected {sum(p.likes for p in mine)} likes, {sum(p.dislikes for p in mine)} dislikes, "
        f"{sum(p.reposts for p in mine)} reposts and {sum(p.replies for p in mine)} replies."
    ]
    mine_ids = {p.id for p in mine}
    replies = sorted(
        (p for p in posts.values() if p.parent_id in mine_ids and p.author_id != persona.id), key=lambda p: p.id
    )
    for reply in replies[-12:]:
        author = people.get(reply.author_id)
        who = f"@{author.handle}" if author else reply.author_id
        lines.append(f'{who} replied to your #{reply.parent_id} in round {reply.round}: "{clip(reply.content, 240)}"')
    return lines


# ---------------------------------------------------------------- report Q&A


async def ask_report(llm: LLM, world: World, report: Report, question: str, *, model: str) -> str:
    ev = collect_evidence(world)
    matches = relevant_posts(ev, question)
    prompt = "\n\n".join(
        [
            "REPORT\n" + render_markdown(report),
            "EVIDENCE DIGEST\n" + digest_text(ev),
            "POSTS MATCHING THE QUESTION\n" + ("\n".join(post_line(p, ev) for p in matches) or "none"),
            f"QUESTION: {question}",
        ]
    )
    answer = await llm.text(prompt, system=ASK_SYSTEM, model=model, purpose="chat.ask")
    return answer.strip()


_WORD = re.compile(r"[^\W_]{4,}|@\w+|#\d+", re.UNICODE)


def relevant_posts(ev: Evidence, question: str, k: int = 12) -> list[Post]:
    """Cheap lexical retrieval: posts sharing words, handles or ids with the question, best overlap first."""
    terms = {t.lower() for t in _WORD.findall(question)}
    if not terms:
        return []

    def score(post: Post) -> int:
        words = {t.lower() for t in _WORD.findall(post.content)}
        words |= {f"@{ev.handle(post.author_id).lower()}", f"#{post.id}"}
        return len(terms & words)

    scored = [(score(p), p) for p in ev.posts.values()]
    hits = [(s, p) for s, p in scored if s > 0]
    hits.sort(key=lambda sp: (-sp[0], -engagement(sp[1]), sp[1].id))
    return [p for _, p in hits[:k]]
