"""The "Check it" study: what held across the runs of a repeat study (same question, people and text, other seeds).

Python measures the spread of sentiment, per round and at the end. Two cheap calls do the rest: `study.match_camps`
groups the camps of every run (their names differ from run to run), and `study.agree` sorts the runs' conclusions
into the claims that held in every run and the ones that split, with a verdict: consistent, mixed or unstable.
"""

from __future__ import annotations

from tellfold.compare import Member, _load
from tellfold.llm import LLM
from tellfold.models import Agreement, AgreementCall, CampMatch, CampMatches, RoundBand
from tellfold.runner import now_iso

MAX_RUNS = 5
DEFAULT_RUNS = 3

MATCH_SYSTEM = """\
You group opinion camps from several runs of the same social simulation. Each run named its camps itself, so the \
same group of opinion can carry a different name in each run. Put camps that describe the same position in one \
group; a camp that matches nothing is a group of its own. Never put two camps of the same run in one group. Write \
labels in {language}."""

MATCH_TASK = """\
Camps, one per line, as "<run letter>: <camp name> (<people>) - <summary>":
{camps}

Return {{"groups": [{{"label", "members"}}]}}. label is a short plain name for the group (2-5 words). members lists \
each matching camp exactly as "<run letter>: <camp name>", copied from the lines above."""

AGREE_SYSTEM = """\
You check whether a forecast holds up. The same simulated crowd read the same text several times; only chance \
differed between runs. You get each run's conclusion. Decide which claims every run supports and which only some \
runs support. Be strict: a claim held only if every run supports it. Write in {language}."""

AGREE_TASK = """\
STUDY QUESTION: {question}

Final sentiment per run: {finals} (spread {spread}).

{conclusions}

Return one JSON object:
- held: 1-5 short claims that every run supports.
- split: 0-5 short claims that only some runs support; say which runs, e.g. "(A and C)".
- verdict: "consistent" when the main conclusion and its direction held in every run, "mixed" when the main \
conclusion held but important parts split, "unstable" when the runs disagree on the main conclusion.
- summary: one plain sentence for the user saying whether the conclusion held up."""


def bands(members_rounds: list[list[tuple[int, float | None]]]) -> list[RoundBand]:
    """Per round, the lowest and highest score among the runs that scored it."""
    by_round: dict[int, list[float]] = {}
    for rounds in members_rounds:
        for r, score in rounds:
            by_round.setdefault(r, [])
            if score is not None:
                by_round[r].append(score)
    return [RoundBand(round=r, low=round(min(v), 3) if v else None, high=round(max(v), 3) if v else None,
                      runs=len(v)) for r, v in sorted(by_round.items())]


async def build_agreement(llm: LLM, study_id: str, members: list[Member], *, question: str, model: str,
                          language: str = "English") -> Agreement:
    loaded = [_load(m) for m in members]
    finals = [x.digest.final_sentiment for x in loaded]
    scored = [f for f in finals if f is not None]
    low, high = (min(scored), max(scored)) if scored else (None, None)
    spread = round(high - low, 3) if scored else None

    camp_lines = [f"{x.member.letter}: {c.name} ({c.size}) - {c.summary}" for x in loaded for c in x.digest.camps]
    camps: list[CampMatch] = []
    if camp_lines:
        matched = await llm.json(MATCH_TASK.format(camps="\n".join(camp_lines)),
                                 system=MATCH_SYSTEM.format(language=language), model=model,
                                 purpose="study.match_camps", schema=CampMatches)
        camps = match_camps(matched, loaded_camps={x.member.letter: [c.name for c in x.digest.camps] for x in loaded})

    conclusions = []
    for x in loaded:
        r = x.report
        if r is None:
            continue
        reasons = "".join(f"\n- {t}" for t in r.reasons)
        conclusions.append(f"## Run {x.member.letter}\n{r.title}\n{r.summary}\nPrediction: {r.prediction}"
                           f"\nConfidence: {r.confidence}{reasons}")
    fmt = ", ".join(f"{x.member.letter} {'n/a' if f is None else f'{f:+.2f}'}" for x, f in zip(loaded, finals))
    call = await llm.json(AGREE_TASK.format(question=question, finals=fmt,
                                            spread="n/a" if spread is None else f"{spread:.2f}",
                                            conclusions="\n\n".join(conclusions)),
                          system=AGREE_SYSTEM.format(language=language), model=model, purpose="study.agree",
                          schema=AgreementCall)
    return Agreement(study_id=study_id, created_at=now_iso(), runs=len(members), final_low=low, final_high=high,
                     spread=spread, bands=bands([[(p.round, p.sentiment) for p in x.digest.rounds] for x in loaded]),
                     camps=camps, held=call.held, split=call.split, verdict=call.verdict, summary=call.summary)


def match_camps(matched: CampMatches, *, loaded_camps: dict[str, list[str]]) -> list[CampMatch]:
    """The model's groups, checked: members must name a real camp of a real run, at most one per run, and every camp
    the model left out becomes a group of its own. Sorted by how many runs a group appeared in."""
    known = {(letter, name.strip().lower()): name for letter, names in loaded_camps.items() for name in names}
    placed: set[tuple[str, str]] = set()
    out: list[CampMatch] = []
    for group in matched.groups:
        names: list[str] = []
        letters: set[str] = set()
        for raw in group.members:
            letter, _, name = raw.partition(":")
            key = (letter.strip(), name.strip().lower())
            if key in known and key not in placed and key[0] not in letters:
                placed.add(key)
                letters.add(key[0])
                names.append(f"{key[0]}: {known[key]}")
        if names:
            out.append(CampMatch(label=group.label.strip() or names[0].partition(": ")[2], names=names,
                                 runs=len(letters)))
    for key, name in known.items():
        if key not in placed:
            out.append(CampMatch(label=name, names=[f"{key[0]}: {name}"], runs=1))
    out.sort(key=lambda c: -c.runs)
    return out
