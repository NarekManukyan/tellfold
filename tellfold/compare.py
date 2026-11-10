"""Compare the runs of a study: a Python digest per run, then one cited `compare.write` call.

Every run gets a letter (A, B, ...) in study order, and the model cites posts as `A#12` / `B#7` so each citation
points into the right run. The citation audit from tellfold/report.py checks those refs, @handles and quotes.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass
from pathlib import Path

from pydantic import ValidationError

from tellfold.ingest import load_documents
from tellfold.llm import LLM
from tellfold.models import (
    CampDigest,
    CompareReport,
    CompareResult,
    Post,
    Report,
    RoundPoint,
    RunDigest,
    ShiftDigest,
    SimConfig,
    VariantObjections,
    VariantSummary,
)
from tellfold.report import (
    Evidence,
    _POST_REF,
    UNVERIFIED,
    audit_texts,
    clip,
    collect_evidence,
    engagement,
    mark_unverified_texts,
)
from tellfold.runner import SEEDS_DIR, now_iso
from tellfold.world import World

log = logging.getLogger(__name__)

COMPARE_FILE = "compare.json"
SEED_CHARS = 4000  # of each version's text shown to the model
TOP_POSTS = 8
TOP_SHIFTS = 5


@dataclass
class Member:
    """One run of a study, as the comparison sees it."""

    letter: str
    run_id: str
    variant: str
    run_dir: Path
    immutable: bool = False  # a read-only example: open its world without writing


@dataclass
class _Loaded:
    member: Member
    digest: RunDigest
    config: SimConfig | None
    report: Report | None
    evidence: Evidence | None


# ---------------------------------------------------------------- digest


def digest_run(member: Member, state: str = "") -> RunDigest:
    return _load(member, state).digest


def _load(member: Member, state: str = "") -> _Loaded:
    config = _json(member.run_dir / "config.json", SimConfig)
    report = _json(member.run_dir / "report.json", Report)
    evidence = None
    db = member.run_dir / "world.db"
    if db.is_file():
        world = World(db, seed=config.seed if config else 0, read_only=member.immutable)
        try:
            evidence = collect_evidence(world)
        finally:
            world.close()
    digest = RunDigest(letter=member.letter, run_id=member.run_id, variant=member.variant, state=state,
                       headline=report.title if report else None)
    if evidence is not None:
        per_round: dict[int, int] = {}
        for post in evidence.posts.values():
            per_round[post.round] = per_round.get(post.round, 0) + 1
        rounds = [RoundPoint(round=s.round, sentiment=s.sentiment, posts=per_round.get(s.round, s.new_posts))
                  for s in evidence.stats]
        scored = [p.sentiment for p in rounds if p.sentiment is not None]
        digest = digest.model_copy(update={
            "rounds": rounds,
            "final_sentiment": round(scored[-1], 3) if scored else None,
            "shifts": _shifts(evidence),
            "posts": len(evidence.posts),
        })
    if report is not None:
        digest = digest.model_copy(update={
            "camps": [CampDigest(name=c.name, size=len(c.members), sentiment=c.sentiment, summary=c.summary)
                      for c in report.camps],
            "objections": list(report.risks),
        })
    return _Loaded(member, digest, config, report, evidence)


def _shifts(ev: Evidence) -> list[ShiftDigest]:
    out = []
    for s in ev.shifts:
        first, last = ev.post_sentiment.get(s.first.id), ev.post_sentiment.get(s.last.id)
        change = round(last - first, 3) if first is not None and last is not None else None
        out.append(ShiftDigest(agent_id=s.persona.id, handle=s.persona.handle, first_post=s.first.id,
                               last_post=s.last.id, change=change))
    out.sort(key=lambda d: -abs(d.change or 0))
    return out[:TOP_SHIFTS]


def _json(path: Path, model):
    try:
        return model.model_validate_json(path.read_text(encoding="utf-8"))
    except (OSError, ValueError, ValidationError):
        return None


def _seed_text(run_dir: Path) -> str:
    folder = run_dir / SEEDS_DIR
    if not folder.is_dir():
        return ""
    try:
        docs = load_documents(sorted(p for p in folder.iterdir() if p.is_file() and not p.name.startswith(".")))
    except (OSError, ValueError, RuntimeError):
        return ""
    text = "\n\n".join(f"[{name}]\n{body}" for name, body in docs)
    return text if len(text) <= SEED_CHARS else text[:SEED_CHARS].rstrip() + " [...]"


def _signed(value: float | None) -> str:
    return "n/a" if value is None else f"{value:+.2f}"


def _prefixed(text: str, letter: str) -> str:
    """A run's own report cites `#12`; inside a comparison that becomes `A#12`."""
    return _POST_REF.sub(lambda m: f"{letter}#{m.group(1)}", text)


def _post_line(post: Post, ev: Evidence, letter: str) -> str:
    score = ev.post_sentiment.get(post.id)
    body = clip(post.content) if post.content.strip() else "(no comment)"
    return (f'{letter}#{post.id} r{post.round} @{ev.handle(post.author_id)} '
            f'[engagement {engagement(post)}, sentiment {_signed(score)}]: "{body}"')


def _run_block(loaded: _Loaded) -> str:
    m, d, ev = loaded.member, loaded.digest, loaded.evidence
    lines = [f"## {m.letter} = {m.variant}"]
    seed = _seed_text(m.run_dir)
    if seed:
        lines += ["TEXT THE CROWD READ:", seed]
    lines.append(f"final sentiment {_signed(d.final_sentiment)}; {d.posts} posts")
    if d.rounds:
        lines.append("by round: " + ", ".join(f"r{p.round} {_signed(p.sentiment)} ({p.posts} posts)"
                                             for p in d.rounds))
    if d.camps:
        lines.append("camps:")
        lines += [f"- {c.name}: {c.size} people, sentiment {_signed(c.sentiment)}. {_prefixed(c.summary, m.letter)}"
                  for c in d.camps]
    if loaded.report is not None:
        r = loaded.report
        lines.append(f"its report: {r.title}. {_prefixed(r.summary, m.letter)}")
        lines += [f"- reason: {_prefixed(x, m.letter)}" for x in r.reasons]
    if d.objections:
        lines.append("objections (risks from its report):")
        lines += [f"- {_prefixed(x, m.letter)}" for x in d.objections]
    if ev is not None:
        top = sorted(ev.posts.values(), key=lambda p: (-engagement(p), p.id))[:TOP_POSTS]
        if top:
            lines.append("top posts by engagement:")
            lines += [_post_line(p, ev, m.letter) for p in top]
        if d.shifts:
            lines.append("stance shifts (first vs latest post of vocal agents):")
            for s in d.shifts:
                first, last = ev.posts[s.first_post], ev.posts[s.last_post]
                lines.append(f'- @{s.handle} moved {_signed(s.change)}: {m.letter}#{first.id} "{clip(first.content, 160)}"'
                             f' -> {m.letter}#{last.id} "{clip(last.content, 160)}"')
    return "\n".join(lines)


# ---------------------------------------------------------------- the comparison

COMPARE_SYSTEM = """\
You compare how one simulated crowd reacted to different versions of the same announcement. The same people (the \
same personas, with the same backgrounds and starting stances) read each version on a simulated microblogging \
platform over several rounds. You receive a digest per version, computed from each run's database: the text the \
crowd read, sentiment by round, opinion camps, the run's own report, its top posts and stance shifts.

Ground rules:
- Use only the digests. Never invent posts, people, quotes or numbers.
- Every run has a letter. Cite posts with the letter of their run, like A#12 or B#7, never a bare #12. Cite \
people as @handle. Quote short fragments of posts exactly when they make a point vivid.
- Explain differences by what changed in the text, and say when a difference is small enough to be chance: \
one run per version is a single sample.
- Write every prose field in {language}. Keep the version labels, @handles and refs exactly as given."""

COMPARE_TASK = """\
STUDY QUESTION: {question}

VERSIONS: {legend}

Return one JSON object with these fields:
- headline: one sentence (at most 16 words) that says which version the crowd took better, or that it was a tie.
- better_variant: the exact label of the version the crowd took better ({labels}), or null when it is a tie or \
the evidence is too thin.
- why: 2-4 sentences explaining the verdict, with citations like A#12 and B#7 from both runs.
- per_variant: one object per version, {{"variant", "summary", "objections"}}: variant is its exact label; summary \
is 1-2 cited sentences on how the crowd took it; objections lists its 1-4 main objections.
- objections_only_in: objects {{"variant", "objections"}} with the objections that came up against one version \
and not the other. Leave a version out when it has none.
- caveats: 1-3 honest limits of this comparison.

DIGESTS
{digests}"""


async def build_compare(llm: LLM, study_id: str, members: list[Member], *, question: str, model: str,
                        language: str = "English", states: dict[str, str] | None = None) -> CompareResult:
    """Digest every member run, make one `compare.write` call, audit and mark its citations."""
    if len(members) < 2:
        raise ValueError("a comparison needs at least two runs")
    loaded = [_load(m, (states or {}).get(m.run_id, "")) for m in members]
    labels = [m.variant for m in members]
    legend = "; ".join(f"{m.letter} = {m.variant}" for m in members)
    prompt = COMPARE_TASK.format(question=question, legend=legend, labels=", ".join(f'"{x}"' for x in labels),
                                 digests="\n\n".join(_run_block(x) for x in loaded))
    draft = await llm.json(prompt, system=COMPARE_SYSTEM.format(language=language), model=model,
                           purpose="compare.write", schema=CompareReport)
    report = _finalize(draft, members)
    report = _audit_and_mark(report, loaded)
    return CompareResult(study_id=study_id, created_at=now_iso(), runs=[x.digest for x in loaded], report=report)


def _finalize(report: CompareReport, members: list[Member]) -> CompareReport:
    """Labels the model got slightly wrong are mapped back; unknown ones are dropped. One run per version is a
    sample of one, which Python can say with certainty."""
    def label(raw: str | None) -> str | None:
        key = (raw or "").strip().strip('"').lower()
        for m in members:
            if key in (m.variant.lower(), m.letter.lower(), f"{m.letter.lower()} = {m.variant.lower()}"):
                return m.variant
        return None

    per_variant: dict[str, VariantSummary] = {}
    for v in report.per_variant:
        if (name := label(v.variant)) and name not in per_variant:
            per_variant[name] = v.model_copy(update={"variant": name})
    only_in: dict[str, VariantObjections] = {}
    for v in report.objections_only_in:
        if (name := label(v.variant)) and v.objections and name not in only_in:
            only_in[name] = v.model_copy(update={"variant": name})
    order = [m.variant for m in members]
    fact = ("Each version was played once by the same crowd; part of any difference is chance. Use Check it on "
            "each version to see what holds.")
    return report.model_copy(update={
        "better_variant": label(report.better_variant),
        "per_variant": [per_variant[n] for n in order if n in per_variant],
        "objections_only_in": [only_in[n] for n in order if n in only_in],
        "caveats": [fact, *report.caveats],
    })


def _texts(report: CompareReport) -> list[str]:
    return [report.headline, report.why,
            *(t for v in report.per_variant for t in (v.summary, *v.objections)),
            *(t for v in report.objections_only_in for t in v.objections),
            *report.caveats]


def _audit_and_mark(report: CompareReport, loaded: list[_Loaded]) -> CompareReport:
    runs = {x.member.letter: (x.evidence.posts if x.evidence else {}) for x in loaded}
    handles = {p.handle.lower() for x in loaded if x.evidence for p in x.evidence.people.values()}
    audit = audit_texts(_texts(report), {}, handles, runs=runs)
    marked = 0

    def mark(text: str) -> str:
        nonlocal marked
        text, count = mark_unverified_texts(text, {}, handles, runs=runs)
        marked += count
        return text

    if audit.problems:
        report = report.model_copy(update={
            "headline": mark(report.headline),
            "why": mark(report.why),
            "per_variant": [v.model_copy(update={"summary": mark(v.summary), "objections": [mark(o) for o in v.objections]})
                            for v in report.per_variant],
            "objections_only_in": [v.model_copy(update={"objections": [mark(o) for o in v.objections]})
                                   for v in report.objections_only_in],
            "caveats": [mark(c) for c in report.caveats],
        })
    if marked:
        noun = "citation" if marked == 1 else "citations"
        report = report.model_copy(update={
            "caveats": [*report.caveats, f"{marked} {noun} could not be matched to posts and were marked {UNVERIFIED}."]})
    return report.model_copy(update={"citation_audit": audit.model_copy(update={"marked": marked})})
