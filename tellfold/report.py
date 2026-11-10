"""Evidence digest from the simulated world, LLM-written report, and HTML / Markdown rendering."""

from __future__ import annotations

import logging
import re
from collections.abc import Callable, Iterable
from dataclasses import dataclass, field
from functools import partial
from pathlib import Path
from statistics import fmean
from typing import TYPE_CHECKING, Literal

from jinja2 import Environment, FileSystemLoader, select_autoescape
from markupsafe import Markup, escape

from tellfold.llm import LLM, LLMError
from tellfold.models import (
    ActionType,
    CitationAudit,
    GodEvent,
    KnowledgeGraph,
    Misquote,
    Persona,
    Post,
    Camp,
    Report,
    RoundStats,
    SimConfig,
)

if TYPE_CHECKING:
    from tellfold.world import World

log = logging.getLogger(__name__)

TEMPLATES = Path(__file__).parent / "templates"
SNIPPET_CHARS = 280
MIX_TYPES = [t.value for t in ActionType if t is not ActionType.NOTHING]


# ---------------------------------------------------------------- evidence


@dataclass
class AgentStats:
    persona: Persona
    followers: int
    posts: int  # authored posts, replies and reposts
    engagement: int  # summed engagement of authored posts


@dataclass
class Thread:
    root: Post
    replies: list[Post]
    size: int  # all descendants, not only the ones kept in `replies`


@dataclass
class StanceShift:
    persona: Persona
    first: Post
    last: Post
    posts: int
    last_note: str


@dataclass
class EventImpact:
    event: GodEvent
    sentiment_before: float | None
    sentiment_after: float | None
    posts_before: float  # mean new posts per round in the window
    posts_after: float


@dataclass
class Evidence:
    people: dict[str, Persona]
    posts: dict[int, Post]
    stats: list[RoundStats]
    agents: list[AgentStats]  # most-followed first
    top_posts: list[Post]
    threads: list[Thread]
    shifts: list[StanceShift]
    events: list[EventImpact]
    action_totals: dict[str, int]
    post_sentiment: dict[int, float] = field(default_factory=dict)  # post id -> scored sentiment, when scored

    def handle(self, agent_id: str) -> str:
        persona = self.people.get(agent_id)
        return persona.handle if persona else agent_id


def engagement(post: Post) -> int:
    return post.likes + 2 * post.reposts + post.replies - post.dislikes


def collect_evidence(world: World, *, top_k: int = 8, event_window: int = 2) -> Evidence:
    """Pure-python digest of the run: everything the report and the report chat reason over."""
    people = {p.id: p for p in world.personas()}
    posts = {p.id: p for p in world.posts()}
    stats = sorted(world.round_stats(), key=lambda s: s.round)
    totals: dict[str, int] = {}
    for s in stats:
        for kind, count in s.actions.items():
            totals[kind] = totals.get(kind, 0) + count
    return Evidence(
        people=people,
        posts=posts,
        stats=stats,
        agents=_agent_stats(world, people, posts),
        top_posts=_top_posts(posts, top_k),
        threads=_threads(posts, limit=4),
        shifts=_stance_shifts(world, people, posts, limit=12),
        events=[_event_impact(e, stats, event_window) for e in sorted(world.events(), key=lambda e: e.round)],
        action_totals=totals,
        post_sentiment=world.post_sentiments(),
    )


def _agent_stats(world: World, people: dict[str, Persona], posts: dict[int, Post]) -> list[AgentStats]:
    authored: dict[str, list[Post]] = {}
    for post in posts.values():
        authored.setdefault(post.author_id, []).append(post)
    rows = [
        AgentStats(
            persona=p,
            followers=len(world.followers(p.id)),
            posts=len(authored.get(p.id, [])),
            engagement=sum(engagement(x) for x in authored.get(p.id, [])),
        )
        for p in people.values()
    ]
    return sorted(rows, key=lambda a: (-a.followers, -a.engagement, a.persona.id))


def _top_posts(posts: dict[int, Post], k: int) -> list[Post]:
    originals = [p for p in posts.values() if p.repost_of is None and p.content.strip()]
    return sorted(originals, key=lambda p: (-engagement(p), p.id))[:k]


def _children(posts: dict[int, Post]) -> dict[int, list[Post]]:
    children: dict[int, list[Post]] = {}
    for post in posts.values():
        if post.parent_id is not None:
            children.setdefault(post.parent_id, []).append(post)
    return children


def _descendants(root_id: int, children: dict[int, list[Post]]) -> list[Post]:
    found: list[Post] = []
    stack = [root_id]
    while stack:
        for child in children.get(stack.pop(), []):
            found.append(child)
            stack.append(child.id)
    return sorted(found, key=lambda p: p.id)


def _threads(posts: dict[int, Post], *, limit: int, max_replies: int = 6) -> list[Thread]:
    children = _children(posts)
    threads = []
    for post in posts.values():
        if post.parent_id is None and post.repost_of is None and post.id in children:
            replies = _descendants(post.id, children)
            threads.append(Thread(root=post, replies=replies[:max_replies], size=len(replies)))
    threads.sort(key=lambda t: (-t.size, -engagement(t.root), t.root.id))
    return threads[:limit]


def _stance_shifts(
    world: World, people: dict[str, Persona], posts: dict[int, Post], *, limit: int
) -> list[StanceShift]:
    by_author: dict[str, list[Post]] = {}
    for post in sorted(posts.values(), key=lambda p: p.id):
        if post.content.strip():
            by_author.setdefault(post.author_id, []).append(post)
    shifts = []
    for agent_id, authored in by_author.items():
        if len(authored) < 2 or agent_id not in people:
            continue
        notes = world.memory(agent_id).notes
        shifts.append(
            StanceShift(
                persona=people[agent_id],
                first=authored[0],
                last=authored[-1],
                posts=len(authored),
                last_note=notes[-1] if notes else "",
            )
        )
    shifts.sort(key=lambda s: (-s.posts, -s.persona.influence, s.persona.id))
    return shifts[:limit]


def _mean_sentiment(stats: list[RoundStats]) -> float | None:
    values = [s.sentiment for s in stats if s.sentiment is not None]
    return round(fmean(values), 3) if values else None


def _event_impact(event: GodEvent, stats: list[RoundStats], window: int) -> EventImpact:
    before = [s for s in stats if event.round - window <= s.round < event.round]
    after = [s for s in stats if event.round <= s.round < event.round + window]
    return EventImpact(
        event=event,
        sentiment_before=_mean_sentiment(before),
        sentiment_after=_mean_sentiment(after),
        posts_before=round(fmean(s.new_posts for s in before), 1) if before else 0.0,
        posts_after=round(fmean(s.new_posts for s in after), 1) if after else 0.0,
    )


# ---------------------------------------------------------------- digest text


def clip(text: str, limit: int = SNIPPET_CHARS) -> str:
    text = " ".join(text.split())
    return text if len(text) <= limit else text[: limit - 1].rstrip() + "…"


def _signed(value: float | None) -> str:
    return "n/a" if value is None else f"{value:+.2f}"


def post_line(post: Post, ev: Evidence) -> str:
    """One compact line per post, the citation format the LLM is asked to reuse (#id, @handle)."""
    link = ""
    if post.parent_id is not None:
        link = f" reply to #{post.parent_id}"
    elif post.repost_of is not None:
        link = f" repost of #{post.repost_of}"
    counts = f"[{post.likes} likes, {post.dislikes} dislikes, {post.reposts} reposts, {post.replies} replies]"
    body = clip(post.content) if post.content.strip() else "(no comment)"
    return f'#{post.id} r{post.round} @{ev.handle(post.author_id)}{link} {counts}: "{body}"'


def _timeline_lines(ev: Evidence) -> list[str]:
    events_by_round: dict[int, list[str]] = {}
    for impact in ev.events:
        events_by_round.setdefault(impact.event.round, []).append(impact.event.content)
    lines = []
    for s in ev.stats:
        for text in events_by_round.get(s.round, []):
            lines.append(f"  BREAKING NEWS at round {s.round}: {clip(text, 300)}")
        mix = ", ".join(f"{k} {v}" for k, v in sorted(s.actions.items()) if v)
        lines.append(
            f"r{s.round}: {s.active_agents} active, {s.new_posts} new posts, "
            f"sentiment {_signed(s.sentiment)}; actions: {mix or 'none'}"
        )
    return lines


def _event_lines(ev: Evidence) -> list[str]:
    return [
        f'round {e.event.round}: "{clip(e.event.content, 300)}" -> sentiment {_signed(e.sentiment_before)} '
        f"before vs {_signed(e.sentiment_after)} from that round on; posts/round "
        f"{e.posts_before} before vs {e.posts_after} after"
        for e in ev.events
    ]


def _roster_lines(ev: Evidence) -> list[str]:
    """One short line per agent, so the camps can cover everyone, including agents who never posted."""
    scores: dict[str, list[float]] = {}
    for post_id, score in ev.post_sentiment.items():
        if post_id in ev.posts:
            scores.setdefault(ev.posts[post_id].author_id, []).append(score)
    lines = []
    for a in sorted(ev.agents, key=lambda a: a.persona.id):
        values = scores.get(a.persona.id, [])
        mood = f"mean post sentiment {_signed(sum(values) / len(values))}" if values else "no scored posts"
        kind = "organisation" if a.persona.entity_id else "person"
        lines.append(f"{a.persona.id} @{a.persona.handle} ({kind}): {a.posts} posts, {mood}; "
                     f"starting stance: {clip(a.persona.stance, 90)}")
    return lines


def _agent_lines(ev: Evidence, limit: int = 10) -> list[str]:
    return [
        f"@{a.persona.handle} ({a.persona.id}, {a.persona.name}): {a.followers} followers, "
        f"{a.posts} posts, engagement {a.engagement}; {clip(a.persona.bio, 140)}"
        for a in ev.agents[:limit]
    ]


def _shift_lines(ev: Evidence) -> list[str]:
    lines = []
    for s in ev.shifts:
        lines.append(f"@{s.persona.handle} ({s.persona.id}), {s.posts} posts. Starting stance: {clip(s.persona.stance, 160)}")
        lines.append(f"    first #{s.first.id} r{s.first.round}: \"{clip(s.first.content, 200)}\"")
        lines.append(f"    last  #{s.last.id} r{s.last.round}: \"{clip(s.last.content, 200)}\"")
        if s.last_note:
            lines.append(f"    latest private note: {clip(s.last_note, 160)}")
    return lines


def _thread_lines(ev: Evidence) -> list[str]:
    lines = []
    for t in ev.threads:
        lines.append(f"Thread ({t.size} replies) " + post_line(t.root, ev))
        lines.extend("    " + post_line(r, ev) for r in t.replies)
    return lines


def _entity_lines(graph: KnowledgeGraph, limit: int = 15) -> list[str]:
    return [f"{e.name} ({e.kind}): {clip(e.summary, 120)}" for e in graph.entities[:limit]]


def digest_text(ev: Evidence, *, graph: KnowledgeGraph | None = None) -> str:
    total_actions = sum(v for k, v in ev.action_totals.items() if k != ActionType.NOTHING.value)
    blocks: list[tuple[str, list[str]]] = [
        (
            "SCALE",
            [
                f"{len(ev.people)} agents, {_rounds_done(ev)} rounds simulated, {len(ev.posts)} posts, "
                f"{total_actions} actions in total",
                "action totals: " + (", ".join(f"{k} {v}" for k, v in sorted(ev.action_totals.items())) or "none"),
            ],
        ),
        ("KEY ENTITIES FROM THE SEED DOCUMENTS", _entity_lines(graph) if graph else []),
        ("ROUND TIMELINE (sentiment -1 hostile .. +1 favourable toward the subject)", _timeline_lines(ev)),
        ("INJECTED EVENTS AND MEASURED IMPACT", _event_lines(ev)),
        ("TOP POSTS BY ENGAGEMENT (likes + 2*reposts + replies - dislikes)", [post_line(p, ev) for p in ev.top_posts]),
        ("MOST ACTIVE THREADS", _thread_lines(ev)),
        ("MOST-FOLLOWED AGENTS", _agent_lines(ev)),
        ("EVERY AGENT (place each one in exactly one camp)", _roster_lines(ev)),
        ("STANCE TRAJECTORIES (first vs latest post of the most vocal agents)", _shift_lines(ev)),
    ]
    return "\n\n".join(f"## {title}\n" + "\n".join(lines) for title, lines in blocks if lines)


# ---------------------------------------------------------------- report


REPORT_SYSTEM = """\
You are the lead analyst of a social-simulation study. A population of language-model agents, each playing a \
distinct persona with its own background and starting stance, reacted to a scenario on a simulated microblogging \
platform over several rounds. You receive a digest computed directly from the simulation database: round-by-round \
numbers, the most engaged posts and threads, the most-followed agents, how vocal agents' positions moved, and the \
measured effect of injected news events. You write the final briefing that answers the study question.

Ground rules:
- Use only the digest. Never invent posts, agents, quotes, numbers or events. When evidence is thin or mixed, say so.
- Cite inline: agents as @handle and posts as #id (for example "@some_handle's reply #41"). Every substantive claim \
needs at least one citation; quote short fragments of posts when they make a point vivid.
- Answer the question first, then explain how the simulated crowd got there.
- Keep two things apart: what the simulated agents did, and what that suggests about the real world.
- Look for structure, not just averages: camps and who leads them, minority views that gained or lost ground, \
turning points, and agents who changed their mind.
- Calibrate confidence. "high" only when the signal is consistent across rounds and across different kinds of \
agents and survives injected events; "medium" for a clear lean with notable dissent or volatility; "low" when \
sentiment flipped, activity was sparse, or the crowd split evenly.
- Write every prose field in {language}, including the title, reasons and camp names. Keep @handles, #ids, \
persona ids and JSON keys exactly as given."""


REPORT_TASK = """\
STUDY QUESTION: {question}

Return one JSON object with these fields:
- title: a specific headline (at most 12 words) that states the finding. Not "Simulation report".
- summary: 3-5 sentences that answer the study question directly, with citations.
- prediction: one paragraph naming the single most likely outcome, the conditions it depends on, and the strongest \
counter-signal.
- confidence: "low", "medium" or "high", following the calibration rules.
- sections: 3-5 objects {{"title", "body_markdown"}}. Cover how the conversation evolved round by round, the camps \
and the voices leading them, turning points (including the injected events and their measured effect), and \
dissenting views worth watching. body_markdown may use short paragraphs, "- " bullet lists and **bold**; no \
tables, links or headings.
- key_voices: 3-8 persona ids (like "a04", not handles) whose posts most shaped the outcome.
- reasons: 2-4 one-line reasons (at most 20 words each) that explain the title, each with at least one citation. \
They are the first thing a reader sees under the headline.
- camps: 2-5 objects {{"name", "summary", "members"}} that split the crowd into opinion groups by where each agent \
ended up. name is a short plain label (2-5 words, e.g. "Stayed on free", "Turned against Plus"); summary is one \
sentence with citations; members lists persona ids. Every agent in the digest belongs to exactly one camp; put \
organisations with the camp their posts support, or in their own camp when they mostly stayed neutral.
- risks: 2-5 concrete ways the real-world outcome could differ from the prediction.
- caveats: 2-5 honest limits of this run as evidence, tied to its numbers where possible (population size and mix, \
all agents share one underlying model and can converge, a compressed timeline, the seed documents frame what agents \
know, sentiment is model-scored).

EVIDENCE DIGEST
{digest}"""


async def build_report(llm: LLM, world: World, graph: KnowledgeGraph, config: SimConfig, *, model: str) -> Report:
    ev = collect_evidence(world)
    prompt = REPORT_TASK.format(question=config.question, digest=digest_text(ev, graph=graph))
    system = REPORT_SYSTEM.format(language=config.language)
    draft = await llm.json(prompt, system=system, model=model, purpose="report.write", schema=Report)
    report = _finalize(draft, ev, config)
    audit = audit_citations(report, ev.posts, ev.people.values())
    repaired = False
    if audit.problems:
        # One repair call, like the JSON repair in llm.py: show the model what did not match and ask again.
        repaired = True
        try:
            draft = await llm.json(
                _citation_repair_prompt(prompt, draft, audit, ev.posts),
                system=system,
                model=model,
                purpose="report.repair",
                schema=Report,
            )
            report = _finalize(draft, ev, config)
            audit = audit_citations(report, ev.posts, ev.people.values())
        except LLMError:
            log.warning("citation repair call failed; marking the unmatched citations instead", exc_info=True)
    return _mark_unverified(report, audit, ev, repaired=repaired)


def _finalize(report: Report, ev: Evidence, config: SimConfig) -> Report:
    """Repair what the model may get wrong and add the caveats Python can state with certainty."""
    by_handle = {p.handle.lower(): p.id for p in ev.people.values()}
    voices: list[str] = []
    for raw in report.key_voices:
        key = raw.strip().lstrip("@")
        agent_id = key if key in ev.people else by_handle.get(key.lower())
        if agent_id and agent_id not in voices:
            voices.append(agent_id)
    if not voices:
        voices = [a.persona.id for a in sorted(ev.agents, key=lambda a: -a.engagement)[:3] if a.posts]
    return report.model_copy(update={
        "key_voices": voices,
        "camps": _finalize_camps(report.camps, ev, by_handle),
        "caveats": _fact_caveats(ev, config) + report.caveats,
    })


def _finalize_camps(camps: list[Camp], ev: Evidence, by_handle: dict[str, str]) -> list[Camp]:
    """Every agent in exactly one camp (first mention wins, leftovers in "Other"), with a measured sentiment."""
    seen: set[str] = set()
    out: list[Camp] = []
    for camp in camps:
        members: list[str] = []
        for raw in camp.members:
            key = raw.strip().lstrip("@")
            agent_id = key if key in ev.people else by_handle.get(key.lower())
            if agent_id and agent_id not in seen:
                seen.add(agent_id)
                members.append(agent_id)
        if members and camp.name.strip():
            out.append(camp.model_copy(update={"name": camp.name.strip(), "members": members}))
    if out:
        rest = [pid for pid in ev.people if pid not in seen]
        if rest:
            out.append(Camp(name="Other", summary="Agents the report did not place in a camp.", members=rest))
    scores: dict[str, list[float]] = {}
    for post_id, score in ev.post_sentiment.items():
        if post_id in ev.posts:
            scores.setdefault(ev.posts[post_id].author_id, []).append(score)
    for i, camp in enumerate(out):
        values = [v for pid in camp.members for v in scores.get(pid, [])]
        out[i] = camp.model_copy(update={"sentiment": round(sum(values) / len(values), 3) if values else None})
    return out


def _rounds_done(ev: Evidence) -> int:
    """Simulated rounds only; round 0 holds the opening posts."""
    return sum(1 for s in ev.stats if s.round > 0)


def _fact_caveats(ev: Evidence, config: SimConfig) -> list[str]:
    caveats = [
        f"Scale: {len(ev.people)} simulated agents over {_rounds_done(ev)} rounds produced {len(ev.posts)} posts; "
        f"agent decisions came from the '{config.agent_model}' model. Treat this as a scenario exploration, "
        "not a poll or a measurement of real people."
    ]
    unscored = [s.round for s in ev.stats if s.sentiment is None]
    if unscored:
        caveats.append(f"Sentiment could not be scored for round(s) {', '.join(map(str, unscored))}.")
    if _rounds_done(ev) < config.rounds:
        caveats.append(f"Only {_rounds_done(ev)} of {config.rounds} planned rounds completed.")
    return caveats


# ---------------------------------------------------------------- citation audit

UNVERIFIED = "[unverified]"
# A quoted fragment: straight, curly, German low-high or guillemet quotes, on one line.
_QUOTE = re.compile(r'"([^"\n]+)"|“([^”\n]+)”|„([^“”\n]+)[“”]|«\s*([^»\n]+?)\s*»')
_SENTENCE_END = re.compile(r"[.!?。！？](?=\s|$)|\n")
_ELLIPSIS = re.compile(r"\.{3}|…")
_MIN_QUOTE_WORDS = 3


# A post in one run of a study, cited as "A#12": the letter names the run (see tellfold/compare.py).
_PREFIXED_REF = re.compile(r"(?<![\w&#])([A-Z])#(\d+)\b")

# Letter -> that run's posts, for citations in a study comparison.
RunPosts = dict[str, dict[int, Post]]


@dataclass
class _Citation:
    kind: Literal["post", "handle", "quote", "ref"]
    start: int  # span in the text, replaced by UNVERIFIED when the citation is marked
    end: int
    ok: bool
    post_id: int | None = None
    handle: str | None = None
    quote: str | None = None
    ref: str | None = None  # "A#12", for kind "ref" and for quotes paired with a prefixed ref


def _report_texts(report: Report) -> list[str]:
    """Every prose field that can carry citations."""
    return [
        report.summary,
        report.prediction,
        *report.reasons,
        *(s.body_markdown for s in report.sections),
        *(c.summary for c in report.camps),
        *report.risks,
        *report.caveats,
    ]


def _map_texts(report: Report, fn: Callable[[str], str]) -> Report:
    return report.model_copy(update={
        "summary": fn(report.summary),
        "prediction": fn(report.prediction),
        "reasons": [fn(t) for t in report.reasons],
        "sections": [s.model_copy(update={"body_markdown": fn(s.body_markdown)}) for s in report.sections],
        "camps": [c.model_copy(update={"summary": fn(c.summary)}) for c in report.camps],
        "risks": [fn(t) for t in report.risks],
        "caveats": [fn(t) for t in report.caveats],
    })


def _normalise(text: str) -> str:
    """Case, whitespace and punctuation folded away, so a quote matches however the model punctuated it."""
    text = text.lower().replace("’", "'").replace("‘", "'")
    return " ".join(re.sub(r"[^\w\s]", " ", text).split())


def _quote_matches(quote: str, sources: list[str]) -> bool:
    """Every piece of the quote (split at "..." elisions) appears, as whole words, in one of the sources."""
    parts = [p for p in (_normalise(x) for x in _ELLIPSIS.split(quote)) if p]
    if not parts:
        return True
    for source in sources:
        hay = f" {_normalise(source)} "
        if all(f" {part} " in hay for part in parts):
            return True
    return False


def _post_sources(post_id: int, posts: dict[int, Post]) -> list[str]:
    post = posts[post_id]
    sources = [post.content]
    if post.repost_of is not None and post.repost_of in posts:
        sources.append(posts[post.repost_of].content)
    return sources


def _sentence_bounds(masked: str, start: int, end: int) -> tuple[int, int]:
    lo = 0
    for m in _SENTENCE_END.finditer(masked, 0, start):
        lo = m.end()
    nxt = _SENTENCE_END.search(masked, end)
    return lo, nxt.end() if nxt else len(masked)


def _citations(text: str, posts: dict[int, Post], handles: set[str], *,
               runs: RunPosts | None = None) -> list[_Citation]:
    """#ids, @handles and quoted fragments (3+ words) in one text, each checked against the run.

    With `runs` (a study comparison), "A#12" refs are checked against run A's posts too, and quotes pair with them."""
    quotes = [(m.start(), m.end(), next(g for g in m.groups() if g)) for m in _QUOTE.finditer(text)]

    def inside_quote(i: int) -> bool:
        return any(a <= i < b for a, b, _ in quotes)

    found: list[_Citation] = []
    for m in _POST_REF.finditer(text):
        if not inside_quote(m.start()):
            pid = int(m.group(1))
            found.append(_Citation("post", m.start(), m.end(), pid in posts, post_id=pid))
    if runs is not None:
        for m in _PREFIXED_REF.finditer(text):
            if not inside_quote(m.start()):
                letter, pid = m.group(1), int(m.group(2))
                found.append(_Citation("ref", m.start(), m.end(), pid in runs.get(letter, {}), post_id=pid,
                                       ref=f"{letter}#{pid}"))
    for m in _HANDLE_REF.finditer(text):
        if not inside_quote(m.start()):
            handle = m.group(1)
            found.append(_Citation("handle", m.start(), m.end(), handle.lower() in handles, handle=handle))
    # Sentence ends inside quotes don't count, so a quote is paired with the #ids of the sentence around it.
    masked = list(text)
    for a, b, _ in quotes:
        masked[a:b] = "\0" * (b - a)
    masked_text = "".join(masked)
    for a, b, quote in quotes:
        if len(quote.split()) < _MIN_QUOTE_WORDS:
            continue
        lo, hi = _sentence_bounds(masked_text, a, b)
        sentence = masked_text[lo:hi]
        # (posts of the run the ref points into, post id, "A#12" or None)
        cited = [(posts, int(m.group(1)), None) for m in _POST_REF.finditer(sentence)]
        if runs is not None:
            cited += [(runs.get(m.group(1), {}), int(m.group(2)), f"{m.group(1)}#{m.group(2)}")
                      for m in _PREFIXED_REF.finditer(sentence)]
        cited = [c for c in cited if c[1] in c[0]]
        if not cited:
            continue  # not tied to a post that exists (an unknown #id is reported on its own)
        ok = any(_quote_matches(quote, _post_sources(pid, source)) for source, pid, _ in cited)
        found.append(_Citation("quote", a, b, ok, post_id=cited[0][1], quote=quote.strip(), ref=cited[0][2]))
    return found


def audit_citations(report: Report, posts: dict[int, Post], personas: Iterable[Persona]) -> CitationAudit:
    """Check every #id, @handle and quoted fragment in the report against the run's posts and agents."""
    return audit_texts(_report_texts(report), posts, {p.handle.lower() for p in personas})


def audit_texts(texts: Iterable[str], posts: dict[int, Post], handles: set[str], *,
                runs: RunPosts | None = None) -> CitationAudit:
    """The audit of any cited prose: `handles` are lowercased; `runs` checks "A#12" refs in a study comparison."""
    found = [c for text in texts for c in _citations(text, posts, handles, runs=runs)]
    unknown_posts: list[int] = []
    unknown_handles: list[str] = []
    unknown_refs: list[str] = []
    misquotes: list[Misquote] = []
    for c in found:
        if c.ok:
            continue
        if c.kind == "post" and c.post_id not in unknown_posts:
            unknown_posts.append(c.post_id)
        elif c.kind == "ref" and c.ref not in unknown_refs:
            unknown_refs.append(c.ref)
        elif c.kind == "handle" and c.handle not in unknown_handles:
            unknown_handles.append(c.handle)
        elif c.kind == "quote" and not any(q.post_id == c.post_id and q.ref == c.ref and q.quote == c.quote
                                           for q in misquotes):
            misquotes.append(Misquote(post_id=c.post_id, quote=c.quote, ref=c.ref))
    return CitationAudit(checked=len(found), unknown_posts=unknown_posts, unknown_handles=unknown_handles,
                         unknown_refs=unknown_refs, misquotes=misquotes)


def mark_unverified_texts(text: str, posts: dict[int, Post], handles: set[str], *,
                          runs: RunPosts | None = None) -> tuple[str, int]:
    """`text` with every citation that does not match replaced by [unverified], and how many were replaced."""
    bad = sorted((c for c in _citations(text, posts, handles, runs=runs) if not c.ok), key=lambda c: -c.start)
    for c in bad:
        text = text[: c.start] + UNVERIFIED + text[c.end :]
    return text, len(bad)


def _citation_repair_prompt(prompt: str, draft: Report, audit: CitationAudit, posts: dict[int, Post]) -> str:
    problems = [f"- #{pid} is not a post in this run." for pid in audit.unknown_posts]
    problems += [f"- @{h} is not an agent in this run." for h in audit.unknown_handles]
    problems += [
        f'- The quote "{clip(q.quote, 160)}" is not in post #{q.post_id}, which says: "{clip(posts[q.post_id].content, 240)}"'
        for q in audit.misquotes
    ]
    previous = draft.model_dump_json(exclude={"citation_audit"})
    return (
        f"{prompt}\n\n---\nYour previous report cites things that are not in the digest:\n" + "\n".join(problems)
        + f"\n\nPrevious report:\n{previous}\n\n"
        "Return the whole report again with every citation fixed: cite a post or agent that exists in the digest, "
        "quote the post's exact words, or drop the claim. Keep everything else as it was."
    )


def citation_line(audit: CitationAudit | None) -> str | None:
    """The one-line summary shown under the confidence badge; None for older reports and reports without citations."""
    if audit is None or audit.checked == 0:
        return None
    if audit.marked:
        return f"{audit.checked - audit.marked} of {audit.checked} citations checked against the posts, {audit.marked} marked"
    if audit.checked == 1:
        return "The 1 citation was checked against the posts"
    return f"All {audit.checked} citations checked against the posts"


def _mark_unverified(report: Report, audit: CitationAudit, ev: Evidence, *, repaired: bool) -> Report:
    """Replace every citation that still doesn't match with [unverified] and say so in a caveat."""
    handles = {p.handle.lower() for p in ev.people.values()}
    marked = 0

    def mark(text: str) -> str:
        nonlocal marked
        text, count = mark_unverified_texts(text, ev.posts, handles)
        marked += count
        return text

    if audit.problems:
        report = _map_texts(report, mark)
    caveats = report.caveats
    if marked:
        noun = "citation" if marked == 1 else "citations"
        caveats = [*caveats, f"{marked} {noun} could not be matched to posts and were marked {UNVERIFIED}."]
    return report.model_copy(update={
        "caveats": caveats,
        "citation_audit": audit.model_copy(update={"repaired": repaired, "marked": marked}),
    })


# ---------------------------------------------------------------- markdown


def render_markdown(report: Report) -> str:
    parts = [
        f"# {report.title}",
        report.summary,
        f"## Prediction\n\n{report.prediction}\n\n**Confidence:** {report.confidence}",
    ]
    parts += [f"## {s.title}\n\n{s.body_markdown}" for s in report.sections]
    if report.key_voices:
        parts.append("## Key voices\n\n" + ", ".join(report.key_voices))
    for title, items in (("Risks", report.risks), ("Caveats", report.caveats)):
        if items:
            parts.append(f"## {title}\n\n" + "\n".join(f"- {item}" for item in items))
    return "\n\n".join(parts) + "\n"


# ---------------------------------------------------------------- html

_POST_REF = re.compile(r"(?<![\w&#])#(\d+)\b")
_HANDLE_REF = re.compile(r"(?<![\w@./])@([A-Za-z0-9_]{1,40})")


@dataclass
class _Refs:
    post_ids: set[int]
    handles: set[str]

    def link(self, html: str) -> str:
        def post(m: re.Match[str]) -> str:
            pid = int(m.group(1))
            return f'<a class="ref" href="#p-{pid}">#{pid}</a>' if pid in self.post_ids else m.group(0)

        def handle(m: re.Match[str]) -> str:
            h = m.group(1)
            return f'<a class="ref" href="#agent-{h}">@{h}</a>' if h in self.handles else m.group(0)

        return _HANDLE_REF.sub(handle, _POST_REF.sub(post, html))


def _inline(text: str, refs: _Refs) -> str:
    out = []
    for part in re.split(r"(`[^`\n]+`)", text):
        if len(part) > 1 and part.startswith("`") and part.endswith("`"):
            out.append(f"<code>{escape(part[1:-1])}</code>")
            continue
        html = str(escape(part))
        html = re.sub(r"\*\*(.+?)\*\*", r"<strong>\1</strong>", html)
        html = re.sub(r"(?<![*\w])\*(?=\S)(.+?)(?<=\S)\*(?![*\w])", r"<em>\1</em>", html)
        out.append(refs.link(html))
    return "".join(out)


def _markdown(text: str, refs: _Refs) -> Markup:
    """Small Markdown subset: paragraphs, ### headings, bullet and numbered lists, quotes, bold, italic, code."""
    html: list[str] = []
    para: list[str] = []
    items: list[str] = []
    list_tag = ""

    def flush() -> None:
        nonlocal list_tag
        if para:
            html.append(f"<p>{_inline(' '.join(para), refs)}</p>")
            para.clear()
        if items:
            html.append(f"<{list_tag}>" + "".join(f"<li>{i}</li>" for i in items) + f"</{list_tag}>")
            items.clear()
            list_tag = ""

    for raw in text.splitlines():
        line = raw.strip()
        bullet = re.match(r"[-*•]\s+(.*)", line)
        numbered = re.match(r"\d+[.)]\s+(.*)", line)
        heading = re.match(r"#{1,6}\s+(.*)", line)
        if not line:
            flush()
        elif heading:
            flush()
            html.append(f"<h4>{_inline(heading.group(1), refs)}</h4>")
        elif bullet or numbered:
            tag = "ul" if bullet else "ol"
            if para or (items and list_tag != tag):
                flush()
            list_tag = tag
            items.append(_inline((bullet or numbered).group(1), refs))
        elif line.startswith(">"):
            flush()
            html.append(f"<blockquote>{_inline(line.lstrip('> '), refs)}</blockquote>")
        elif items:
            items[-1] += " " + _inline(line, refs)
        else:
            para.append(line)
    flush()
    return Markup("\n".join(html))


def _cited_posts(report: Report, posts: dict[int, Post], limit: int = 24) -> list[Post]:
    texts = [report.summary, report.prediction, *report.risks, *report.caveats]
    texts += [s.body_markdown for s in report.sections]
    seen: list[int] = []
    for text in texts:
        for m in _POST_REF.finditer(text):
            pid = int(m.group(1))
            if pid in posts and pid not in seen:
                seen.append(pid)
    return [posts[pid] for pid in seen[:limit]]


@dataclass
class _ChartPoint:
    x: float
    y: float
    round: int
    sentiment: float


@dataclass
class _SentimentChart:
    width: int
    height: int
    left: int
    right: int
    top: int
    bottom: int
    gridlines: list[tuple[float, str]]  # (y, label)
    ticks: list[tuple[float, str]]  # (x, label)
    paths: list[str]
    points: list[_ChartPoint]
    events: list[tuple[float, str, str]]  # (x, short label, full text)


def _sentiment_chart(stats: list[RoundStats], events: list[GodEvent]) -> _SentimentChart | None:
    if not any(s.sentiment is not None for s in stats):
        return None
    width, height, left, right, top, bottom = 480, 260, 40, 12, 18, 226
    right_edge = width - right
    rounds = [s.round for s in stats]
    lo, hi = min(rounds), max(rounds)

    def x_of(r: int) -> float:
        return (left + right_edge) / 2 if hi == lo else left + (r - lo) / (hi - lo) * (right_edge - left)

    def y_of(v: float) -> float:
        return top + (1 - max(-1.0, min(1.0, v))) / 2 * (bottom - top)

    points, paths, segment = [], [], []
    for s in stats:
        if s.sentiment is None:
            segment = []
            continue
        pt = _ChartPoint(round(x_of(s.round), 1), round(y_of(s.sentiment), 1), s.round, s.sentiment)
        points.append(pt)
        if not segment:
            paths.append("")
        segment.append(pt)
        paths[-1] = "M" + " L".join(f"{p.x},{p.y}" for p in segment)
    step = max(1, len(rounds) // 10)
    return _SentimentChart(
        width=width,
        height=height,
        left=left,
        right=right_edge,
        top=top,
        bottom=bottom,
        gridlines=[(round(y_of(v), 1), lbl) for v, lbl in ((1, "+1"), (0.5, "+0.5"), (0, "0"), (-0.5, "−0.5"), (-1, "−1"))],
        ticks=[(round(x_of(r), 1), f"R{r}") for r in rounds[::step]],
        paths=[p for p in paths if " L" in p],
        points=points,
        events=[
            (round(x_of(e.round), 1), f"E{i}", e.content)
            for i, e in enumerate(events, start=1)
            if lo <= e.round <= hi
        ],
    )


@dataclass
class _MixRow:
    round: int
    total: int
    idle: int
    width_pct: float  # bar length relative to the busiest round
    segments: list[tuple[str, int, float]]  # (action type, count, share of this bar in %)


def _action_mix(stats: list[RoundStats]) -> list[_MixRow]:
    counted = [(s, {t: s.actions.get(t, 0) for t in MIX_TYPES}) for s in stats]
    busiest = max((sum(c.values()) for _, c in counted), default=0) or 1
    rows = []
    for s, counts in counted:
        total = sum(counts.values())
        rows.append(
            _MixRow(
                round=s.round,
                total=total,
                idle=s.actions.get(ActionType.NOTHING.value, 0),
                width_pct=round(100 * total / busiest, 1),
                segments=[(t, n, round(100 * n / total, 2)) for t, n in counts.items() if n and total],
            )
        )
    return rows


def _environment() -> Environment:
    env = Environment(
        loader=FileSystemLoader(str(TEMPLATES)),
        autoescape=select_autoescape(["html"]),
        trim_blocks=True,
        lstrip_blocks=True,
    )
    env.filters["signed"] = _signed
    return env


def render_html(report: Report, world: World, config: SimConfig) -> str:
    ev = collect_evidence(world)
    refs = _Refs(post_ids=set(ev.posts), handles={p.handle for p in ev.people.values()})
    events = [i.event for i in ev.events]
    return (
        _environment()
        .get_template("report.html")
        .render(
            report=report,
            config=config,
            citations=citation_line(report.citation_audit),
            md=partial(_markdown, refs=refs),
            inline=lambda text: Markup(_inline(text, refs)),
            people=ev.people,
            posts=ev.posts,
            key_voices=[ev.people[v] for v in report.key_voices if v in ev.people],
            tiles=[
                ("Agents", len(ev.people)),
                ("Rounds", _rounds_done(ev)),
                ("Posts", len(ev.posts)),
                ("Actions", sum(v for k, v in ev.action_totals.items() if k != ActionType.NOTHING.value)),
            ],
            chart=_sentiment_chart(ev.stats, events),
            stats=ev.stats,
            mix=_action_mix(ev.stats),
            mix_types=MIX_TYPES,
            events=ev.events,
            cited=_cited_posts(report, ev.posts),
            threads=ev.threads,
            directory=ev.agents,
            engagement=engagement,
        )
    )
