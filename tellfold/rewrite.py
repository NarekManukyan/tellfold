"""Suggest a better version of the seed text: one `report.rewrite` call, its citations checked like the report's.

The model gets the original text, the report (camps, reasons, risks) and the posts the report cites, and returns a
`RewriteSuggestion`: the revised text, each change with the posts that motivated it, and what it kept on purpose.
The 2A flow ("Test this version") then runs the revised text past the same people.
"""

from __future__ import annotations

from pathlib import Path

from tellfold.llm import LLM
from tellfold.models import Report, RewriteChange, RewriteSuggestion, SimConfig
from tellfold.report import (
    _POST_REF,
    Evidence,
    audit_texts,
    collect_evidence,
    mark_unverified_texts,
    post_line,
)
from tellfold.runner import SEEDS_DIR, now_iso
from tellfold.world import World

REWRITE_FILE = "rewrite.json"
TEXT_SUFFIXES = {".md", ".markdown", ".txt"}
DESCRIPTION_NAME = "description.md"  # the pasted description the Mac app sends as a seed document
MAX_CITED_POSTS = 24
MAX_TEXT_CHARS = 12000


class RewriteError(Exception):
    """The run has nothing to rewrite (no report, or no text seed document)."""


def editable_seed(run_dir: Path) -> tuple[str, str]:
    """(name, text) of the seed document the suggestion rewrites: the pasted description, else the first text file.
    The Mac app picks the same file (`StudyLogic.editableSeed`)."""
    folder = run_dir / SEEDS_DIR
    files = sorted(p for p in folder.iterdir() if p.is_file() and p.suffix.lower() in TEXT_SUFFIXES
                   and not p.name.startswith(".")) if folder.is_dir() else []
    chosen = next((p for p in files if p.name.lower() == DESCRIPTION_NAME), files[0] if files else None)
    if chosen is None:
        raise RewriteError("this run has no text document to rewrite (only .md and .txt seeds can be rewritten)")
    text = chosen.read_text(encoding="utf-8", errors="replace").strip()
    if not text:
        raise RewriteError(f"{chosen.name} is empty")
    return chosen.name, text


REWRITE_SYSTEM = """\
You are an editor. A simulated crowd of people read an announcement on a social platform, and an analyst wrote a \
report on how they reacted. You rewrite the announcement so the same people would take it better, without changing \
what is being announced: keep the decision, the facts and the numbers unless the crowd showed they were the problem \
and the text can honestly change them.

Rules:
- Change only what the crowd's reaction gives a reason to change. Keep the author's voice, format and length.
- Every change cites the posts that motivated it, by id. In "why", cite posts as #id and people as @handle, and \
quote their exact words when it helps.
- List what you kept on purpose even though someone objected, and say why.
- Write the revised text and every explanation in {language}."""

REWRITE_TASK = """\
STUDY QUESTION: {question}

ORIGINAL TEXT ({name})
{text}

THE REPORT
{report}

POSTS THE REPORT CITES AND THE MOST ENGAGED POSTS
{posts}

Return one JSON object:
- revised_text: the whole announcement, rewritten. Plain text or the same markdown as the original.
- changes: 2-6 objects {{"what", "why", "cites"}}: what changed (a few words), why (one or two sentences on the \
crowd's reaction, with #id citations) and cites (the post ids, as numbers).
- kept_on_purpose: 0-4 short notes on what you did not change, and why."""


def _report_text(report: Report) -> str:
    lines = [f"{report.title}", report.summary, f"Prediction ({report.confidence}): {report.prediction}"]
    lines += [f"- reason: {r}" for r in report.reasons]
    lines += [f"- camp {c.name} ({len(c.members)} people): {c.summary}" for c in report.camps]
    lines += [f"- risk: {r}" for r in report.risks]
    return "\n".join(lines)


def _cited_posts(report: Report, ev: Evidence) -> list[int]:
    texts = [report.summary, report.prediction, *report.reasons, *(s.body_markdown for s in report.sections),
             *(c.summary for c in report.camps), *report.risks]
    ids = [int(m.group(1)) for t in texts for m in _POST_REF.finditer(t)]
    ids += [p.id for p in ev.top_posts]
    return [pid for pid in dict.fromkeys(ids) if pid in ev.posts][:MAX_CITED_POSTS]


async def build_rewrite(llm: LLM, run_dir: Path, config: SimConfig, report: Report, *, model: str,
                        read_only: bool = False) -> RewriteSuggestion:
    name, text = editable_seed(run_dir)
    if len(text) > MAX_TEXT_CHARS:
        # "Test this version" replaces the whole document with the revised text, so a rewrite of only its start
        # would silently drop the rest
        raise RewriteError(f"{name} is too long to rewrite ({len(text):,} characters; the limit is "
                           f"{MAX_TEXT_CHARS:,}), because the suggestion replaces the whole text")
    world = World(run_dir / "world.db", seed=config.seed, read_only=read_only)
    try:
        ev = collect_evidence(world)
    finally:
        world.close()
    posts = "\n".join(post_line(ev.posts[pid], ev) for pid in _cited_posts(report, ev)) or "none"
    prompt = REWRITE_TASK.format(question=config.question, name=name, text=text, report=_report_text(report),
                                 posts=posts)
    draft = await llm.json(prompt, system=REWRITE_SYSTEM.format(language=config.language), model=model,
                           purpose="report.rewrite", schema=RewriteSuggestion)
    return _audit(draft, ev).model_copy(update={"source_name": name, "created_at": now_iso()})


def _audit(draft: RewriteSuggestion, ev: Evidence) -> RewriteSuggestion:
    """Unknown ids leave `cites` and count as unknown posts; citations in `why` that don't match are marked."""
    handles = {p.handle.lower() for p in ev.people.values()}
    audit = audit_texts([c.why for c in draft.changes] + list(draft.kept_on_purpose), ev.posts, handles)
    unknown = [pid for c in draft.changes for pid in c.cites if pid not in ev.posts]
    marked = 0

    def mark(text: str) -> str:
        nonlocal marked
        text, count = mark_unverified_texts(text, ev.posts, handles)
        marked += count
        return text

    changes = [RewriteChange(what=c.what.strip(), why=mark(c.why) if audit.problems else c.why,
                             cites=[pid for pid in dict.fromkeys(c.cites) if pid in ev.posts])
               for c in draft.changes if c.what.strip()]
    kept = [mark(k) for k in draft.kept_on_purpose] if audit.problems else list(draft.kept_on_purpose)
    audit = audit.model_copy(update={
        "checked": audit.checked + sum(len(c.cites) for c in draft.changes),
        "unknown_posts": list(dict.fromkeys([*audit.unknown_posts, *unknown])),
        "marked": marked + len(unknown),
    })
    return draft.model_copy(update={"changes": changes, "kept_on_purpose": kept, "citation_audit": audit})

