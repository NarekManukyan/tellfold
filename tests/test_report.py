import re
from pathlib import Path

import pytest

from tellfold.chat import ask_report, interview, relevant_posts
from tellfold.llm import FakeLLM
from tellfold.models import (
    Action,
    ActionType,
    AgentMemory,
    Entity,
    GodEvent,
    KnowledgeGraph,
    Persona,
    Report,
    ReportSection,
    RoundStats,
    SimConfig,
)
from tellfold.report import (
    _markdown,
    _Refs,
    build_report,
    collect_evidence,
    digest_text,
    render_html,
    render_markdown,
)
from tellfold.world import World

QUESTION = "Will the city council's congestion charge survive public backlash?"


def persona(i: int, handle: str, stance: str, influence: float = 0.5) -> Persona:
    return Persona(
        id=f"a0{i}",
        handle=handle,
        name=handle.replace("_", " ").title(),
        bio=f"Bio of {handle}",
        background=f"Private background of {handle}",
        stance=stance,
        traits=["vocal"],
        influence=influence,
    )


def act(agent: str, kind: ActionType, **kw) -> Action:
    return Action(agent_id=agent, type=kind, **kw)


@pytest.fixture
def world(tmp_path: Path) -> World:
    w = World(tmp_path / "world.db")
    w.add_personas(
        [
            persona(1, "mayor_office", "supports the charge", 0.9),
            persona(2, "cab_driver_sam", "opposes the charge"),
            persona(3, "cyclist_ana", "supports the charge"),
            persona(4, "quiet_reader", "indifferent", 0.1),
        ]
    )
    w.add_event(GodEvent(round=2, content="Council publishes first-month revenue figures"))
    # round 1
    p1 = w.apply(1, act("a01", ActionType.POST, content="The charge starts Monday. Cleaner air for everyone."))
    p2 = w.apply(1, act("a02", ActionType.POST, content="This charge will ruin small drivers. <script>alert(1)</script>"))
    w.apply(1, act("a02", ActionType.REPLY, content="Who pays for this? Not the mayor.", target_post_id=p1))
    w.apply(1, act("a03", ActionType.REPLY, content="Finally safer streets for bikes.", target_post_id=p1))
    w.apply(1, act("a03", ActionType.LIKE, target_post_id=p1))
    w.apply(1, act("a04", ActionType.LIKE, target_post_id=p1))
    w.apply(1, act("a03", ActionType.DISLIKE, target_post_id=p2))
    w.apply(1, act("a02", ActionType.FOLLOW, target_agent_id="a01"))
    w.apply(1, act("a03", ActionType.FOLLOW, target_agent_id="a01"))
    w.apply(1, act("a04", ActionType.FOLLOW, target_agent_id="a01"))
    w.apply(1, act("a04", ActionType.FOLLOW, target_agent_id="a03"))
    w.save_round_stats(
        RoundStats(round=1, actions={"post": 2, "reply": 2, "like": 2, "dislike": 1, "follow": 4},
                   new_posts=4, active_agents=4, sentiment=-0.2)
    )
    # round 2
    w.apply(2, act("a04", ActionType.REPOST, content="Worth reading", target_post_id=p1))
    w.apply(2, act("a02", ActionType.POST, content="Okay, the revenue numbers are better than I feared."))
    w.apply(2, act("a01", ActionType.LIKE, target_post_id=999))  # invalid -> nothing
    w.save_round_stats(
        RoundStats(round=2, actions={"repost": 1, "post": 1, "nothing": 1}, new_posts=2, active_agents=3, sentiment=0.3)
    )
    w.save_memory(AgentMemory(agent_id="a02", notes=["angry at the mayor", "revenue figures softened me"]))
    return w


@pytest.fixture
def config() -> SimConfig:
    return SimConfig(question=QUESTION, seed_files=["policy.md"], rounds=3, language="German")


@pytest.fixture
def graph() -> KnowledgeGraph:
    return KnowledgeGraph(
        entities=[Entity(id="city-council", name="City Council", kind="organization", summary="Runs the city")]
    )


def report_payload(**overrides) -> dict:
    data = {
        "title": "Charge survives as opposition softens",
        "summary": "Yes. @mayor_office framed it early in #1 and @cab_driver_sam softened in #6.",
        "prediction": "The charge stays, with backlash fading after revenue data.",
        "confidence": "medium",
        "sections": [
            {"title": "Camps", "body_markdown": "- **Supporters** led by @cyclist_ana (#4)\n- Critics around #2"}
        ],
        "key_voices": ["a01", "@cab_driver_sam", "ghost_user"],
        "risks": ["Real drivers organise faster than agents did."],
        "caveats": ["Agents share one model."],
    }
    data.update(overrides)
    return data


# ---------------------------------------------------------------- evidence


def test_collect_evidence(world: World):
    ev = collect_evidence(world)
    assert ev.top_posts[0].id == 1  # 2 likes + 2 replies + 1 repost*2
    assert all(p.repost_of is None for p in ev.top_posts)
    assert ev.agents[0].persona.id == "a01" and ev.agents[0].followers == 3
    assert ev.threads[0].root.id == 1 and [r.id for r in ev.threads[0].replies] == [3, 4]
    shift = next(s for s in ev.shifts if s.persona.id == "a02")
    assert (shift.first.id, shift.last.id, shift.last_note) == (2, 6, "revenue figures softened me")
    impact = ev.events[0]
    assert (impact.sentiment_before, impact.sentiment_after) == (-0.2, 0.3)
    assert ev.action_totals["follow"] == 4


def test_digest_text_is_compact_and_citable(world: World, graph: KnowledgeGraph):
    text = digest_text(collect_evidence(world), graph=graph)
    assert "#1 r1 @mayor_office" in text
    assert "BREAKING NEWS at round 2" in text
    assert "-0.20 before vs +0.30" in text
    assert "City Council (organization)" in text
    assert "latest private note: revenue figures softened me" in text


# ---------------------------------------------------------------- report


async def test_build_report_prompt_and_repairs(world: World, graph: KnowledgeGraph, config: SimConfig):
    seen = {}

    def responder(purpose, prompt, schema):
        seen["purpose"], seen["prompt"], seen["schema"] = purpose, prompt, schema
        return report_payload()

    llm = FakeLLM(responder)
    report = await build_report(llm, world, graph, config, model="sonnet")
    assert seen["purpose"] == "report.write" and seen["schema"] is Report
    assert QUESTION in seen["prompt"] and "@cab_driver_sam" in seen["prompt"]
    assert llm.calls[0][1] == "sonnet"
    assert report.key_voices == ["a01", "a02"]  # handle mapped to id, unknown dropped
    assert report.caveats[0].startswith("Scale: 4 simulated agents over 2 rounds")
    assert any("Only 2 of 3 planned rounds" in c for c in report.caveats)
    assert report.caveats[-1] == "Agents share one model."


async def test_build_report_system_uses_language(world, graph, config):
    systems = []

    class Spy(FakeLLM):
        async def json(self, prompt, *, system, model, purpose, schema):
            systems.append(system)
            return await super().json(prompt, system=system, model=model, purpose=purpose, schema=schema)

    report = await build_report(
        Spy(lambda *_: report_payload(key_voices=[])), world, graph, config, model="sonnet"
    )
    assert "German" in systems[0]
    assert report.key_voices  # falls back to the most engaging authors


def test_render_markdown(world):
    md = render_markdown(Report.model_validate(report_payload()))
    assert md.startswith("# Charge survives")
    assert "## Camps" in md and "**Confidence:** medium" in md
    assert "- Agents share one model." in md


def test_markdown_subset_links_and_escapes():
    refs = _Refs(post_ids={4}, handles={"cyclist_ana"})
    html = str(_markdown("Intro **bold** <b>x</b>\n\n- one @cyclist_ana\n- two #4 #99\n\n1. first", refs))
    assert "<strong>bold</strong>" in html and "&lt;b&gt;" in html
    assert '<ul><li>one <a class="ref" href="#agent-cyclist_ana">@cyclist_ana</a></li>' in html
    assert 'href="#p-4"' in html and "#99" in html and 'href="#p-99"' not in html
    assert "<ol><li>first</li></ol>" in html


def test_render_html(world, config):
    report = Report.model_validate(report_payload(key_voices=["a01", "a02"]))
    html = render_html(report, world, config)
    assert "<svg" in html and "prefers-color-scheme: dark" in html
    assert "Charge survives as opposition softens" in html
    assert "<script>" not in html and "&lt;script&gt;" in html
    for pid in re.findall(r'href="#p-(\d+)"', html):
        assert f'id="p-{pid}"' in html  # every citation resolves to a rendered post
    for handle in re.findall(r'href="#agent-(\w+)"', html):
        assert f'id="agent-{handle}"' in html
    assert 'id="t-1"' in html  # top thread rendered as a feed
    assert "Council publishes first-month revenue figures" in html
    assert not re.search(r'(src|href)="https?://', html)  # self-contained


def test_render_html_print_rules(world, config):
    # The Mac app's Export PDF prints this page, so the print rules decide how the PDF looks.
    html = render_html(Report.model_validate(report_payload()), world, config)
    assert "@page { size: auto;" in html  # A4 or Letter, whichever the printer asks for
    css = html[html.index("@media print {") : html.index("</style>")]
    assert "print-color-adjust: exact" in css  # the sentiment chart and action bars keep their colours
    assert re.search(r"\.post, \.card[^{]*\{ break-inside: avoid; \}", css)
    for section in ("analysis", "cited", "threads", "people"):
        assert f'section[aria-labelledby="{section}"]' in css
        assert f'aria-labelledby="{section}"' in html  # the selector still matches the markup
    assert "details" in css and "display: none" in css  # the collapsed round table is screen-only
    assert "--page: #fff;" in css  # always light on paper


def test_render_html_without_rounds(tmp_path, config):
    w = World(tmp_path / "empty.db")
    w.add_personas([persona(1, "solo", "neutral")])
    html = render_html(Report.model_validate(report_payload()), w, config)
    assert "No sentiment was recorded" in html


# ---------------------------------------------------------------- chat


async def test_interview_in_character(world):
    def responder(purpose, prompt, schema):
        assert purpose == "chat.interview"
        return "  Look, I was furious at first.  "

    llm = FakeLLM(responder)
    answer = await interview(
        llm, world, "@cab_driver_sam", "Has your view changed?", model="haiku",
        history=[("Who are you?", "I drive a cab.")],
    )
    assert answer == "Look, I was furious at first."
    prompt = llm.calls[0][2]
    assert "Private background of cab_driver_sam" in prompt
    assert "revenue figures softened me" in prompt
    assert "posted #2" in prompt and "replied to @mayor_office's #1" in prompt
    assert "followed @mayor_office" in prompt
    assert "Council publishes first-month revenue figures" in prompt
    assert prompt.rstrip().endswith("Interviewer: Has your view changed?\nYou:")
    assert "Interviewer: Who are you?\nYou: I drive a cab." in prompt


async def test_interview_reception_and_unknown_agent(world):
    llm = FakeLLM(lambda *_: "ok")
    await interview(llm, world, "a01", "How did people react?", model="haiku")
    prompt = llm.calls[0][2]
    assert '@cab_driver_sam replied to your #1 in round 1: "Who pays for this? Not the mayor."' in prompt
    with pytest.raises(KeyError):
        await interview(llm, world, "a99", "hi", model="haiku")


async def test_ask_report(world):
    llm = FakeLLM(lambda purpose, prompt, schema: "Mostly drivers, see #2.")
    report = Report.model_validate(report_payload())
    answer = await ask_report(llm, world, report, "What did drivers think about revenue?", model="sonnet")
    assert answer == "Mostly drivers, see #2."
    purpose, model, prompt = llm.calls[0]
    assert (purpose, model) == ("chat.ask", "sonnet")
    assert "# Charge survives as opposition softens" in prompt
    assert "POSTS MATCHING THE QUESTION\n#" in prompt and "revenue numbers" in prompt


def test_relevant_posts_ranks_overlap(world):
    ev = collect_evidence(world)
    hits = relevant_posts(ev, "safer streets for bikes?")
    assert hits[0].id == 4
    assert relevant_posts(ev, "?!") == []


def test_finalize_camps_places_every_agent_once_and_scores_sentiment():
    from tellfold.models import Camp, Persona, Post
    from tellfold.report import Evidence, _finalize_camps

    people = {
        pid: Persona(id=pid, handle=f"h_{pid}", name=pid, bio="", background="", stance="")
        for pid in ("a01", "a02", "a03")
    }
    posts = {1: Post(id=1, round=1, author_id="a01", content="x"), 2: Post(id=2, round=1, author_id="a02", content="y")}
    ev = Evidence(people=people, posts=posts, stats=[], agents=[], top_posts=[], threads=[], shifts=[], events=[],
                  action_totals={}, post_sentiment={1: -0.5, 2: 0.4})
    camps = [
        Camp(name="Against", members=["a01", "@h_a01", "ghost"]),
        Camp(name="For", members=["@h_a02", "a01"]),
        Camp(name="Empty", members=["nobody"]),
    ]
    out = _finalize_camps(camps, ev, {p.handle: p.id for p in people.values()})
    assert [(c.name, c.members) for c in out] == [("Against", ["a01"]), ("For", ["a02"]), ("Other", ["a03"])]
    assert out[0].sentiment == -0.5 and out[1].sentiment == 0.4 and out[2].sentiment is None


# ---------------------------------------------------------------- citation audit


BAD_CITATIONS = (
    'Drivers were split. @nobody said it all in #999, and @cab_driver_sam wrote "the charge is a gift" (#2).'
)


def test_audit_finds_unknown_posts_handles_and_misquotes(world: World):
    from tellfold.report import audit_citations

    ev = collect_evidence(world)
    report = Report.model_validate(report_payload(summary=BAD_CITATIONS))
    audit = audit_citations(report, ev.posts, ev.people.values())
    assert audit.unknown_posts == [999]
    assert audit.unknown_handles == ["nobody"]
    assert [(q.post_id, q.quote) for q in audit.misquotes] == [(2, "the charge is a gift")]
    assert audit.problems == 3 and audit.checked > 3


def test_audit_accepts_real_quotes_however_punctuated(world: World):
    from tellfold.report import audit_citations

    ev = collect_evidence(world)
    summary = (
        "@cab_driver_sam warned that “this charge will ruin small drivers” (#2), then admitted in #6: "
        '"Okay, the revenue numbers... better than I feared". @quiet_reader only reposted #5 "cleaner air for everyone".'
    )
    report = Report.model_validate(report_payload(summary=summary, reasons=["Supporters held #1 @MAYOR_OFFICE"]))
    audit = audit_citations(report, ev.posts, ev.people.values())
    assert audit.problems == 0, audit
    # a quote with no post id in its sentence is not checked; "@" inside a quote is not a handle citation
    report = Report.model_validate(report_payload(summary='People kept saying "we will @ghost you all". See #1.'))
    assert audit_citations(report, ev.posts, ev.people.values()).problems == 0


async def test_build_report_repairs_bad_citations(world, graph, config):
    def responder(purpose, prompt, schema):
        if purpose == "report.write":
            return report_payload(summary=BAD_CITATIONS)
        assert purpose == "report.repair"
        assert "#999 is not a post" in prompt and "@nobody is not an agent" in prompt
        assert 'The quote "the charge is a gift" is not in post #2' in prompt
        return report_payload()

    llm = FakeLLM(responder)
    report = await build_report(llm, world, graph, config, model="sonnet")
    assert [c[0] for c in llm.calls] == ["report.write", "report.repair"]
    assert report.citation_audit.problems == 0 and report.citation_audit.repaired
    assert report.citation_audit.marked == 0 and report.citation_audit.checked > 0
    assert "[unverified]" not in render_markdown(report)
    assert sum("Scale:" in c for c in report.caveats) == 1  # fact caveats are not added twice


async def test_build_report_marks_what_the_repair_did_not_fix(world, graph, config):
    llm = FakeLLM(lambda *_: report_payload(summary=BAD_CITATIONS))
    report = await build_report(llm, world, graph, config, model="sonnet")
    assert report.summary == (
        "Drivers were split. [unverified] said it all in [unverified], and @cab_driver_sam wrote [unverified] (#2)."
    )
    assert report.citation_audit.marked == 3 and report.citation_audit.repaired
    assert report.caveats[-1] == "3 citations could not be matched to posts and were marked [unverified]."


async def test_build_report_marks_when_the_repair_call_fails(world, graph, config):
    from tellfold.llm import LLMError

    def responder(purpose, prompt, schema):
        return LLMError("boom") if purpose == "report.repair" else report_payload(summary=BAD_CITATIONS)

    report = await build_report(FakeLLM(responder), world, graph, config, model="sonnet")
    assert report.citation_audit.marked == 3 and "[unverified]" in report.summary


async def test_clean_report_makes_one_call(world, graph, config):
    llm = FakeLLM(lambda *_: report_payload())
    report = await build_report(llm, world, graph, config, model="sonnet")
    assert len(llm.calls) == 1
    assert report.citation_audit.problems == 0 and not report.citation_audit.repaired


def test_citation_line_and_html(world, config):
    from tellfold.models import CitationAudit
    from tellfold.report import citation_line

    assert citation_line(None) is None and citation_line(CitationAudit()) is None
    assert citation_line(CitationAudit(checked=14)) == "All 14 citations checked against the posts"
    assert citation_line(CitationAudit(checked=14, marked=2)) == "12 of 14 citations checked against the posts, 2 marked"
    report = Report.model_validate(report_payload()).model_copy(update={"citation_audit": CitationAudit(checked=7)})
    assert "All 7 citations checked against the posts" in render_html(report, world, config)
    assert "citations checked" not in render_html(Report.model_validate(report_payload()), world, config)


def test_old_report_json_without_audit_still_loads():
    report = Report.model_validate_json(
        '{"title": "T", "summary": "s", "prediction": "p", "confidence": "low", "caveats": ["c"]}'
    )
    assert report.citation_audit is None
    assert "citation_audit" not in str(Report.model_json_schema())  # never asked of the model


# ---------------------------------------------------------------- recommendations


def rec(**overrides) -> dict:
    data = {
        "action": "change",
        "title": "Publish the revenue figures before launch",
        "why": "@cab_driver_sam softened only after the numbers came out (#6).",
        "addresses": ["Critics"],
        "evidence": ["#6", "@cab_driver_sam"],
        "expected_effect": "Drivers would likely move from hostile to wary.",
        "confidence": "medium",
    }
    data.update(overrides)
    return data


CAMPS = [
    {"name": "Critics", "summary": "Drivers led by @cab_driver_sam (#2).", "members": ["a02"]},
    {"name": "Supporters", "summary": "Cyclists and the mayor (#1).", "members": ["a01", "a03", "a04"]},
]


def test_recommendations_are_in_the_schema_and_old_reports_still_load():
    schema = Report.model_json_schema()
    rec_schema = schema["$defs"]["Recommendation"]
    assert set(rec_schema["properties"]) == {
        "action", "title", "why", "addresses", "evidence", "expected_effect", "confidence"
    }
    assert rec_schema["properties"]["action"]["enum"] == ["keep", "change", "add", "drop", "test"]
    old = Report.model_validate_json('{"title": "T", "summary": "s", "prediction": "p", "confidence": "low"}')
    assert old.recommendations == []
    assert "What to do next" not in render_markdown(old)


def test_recommendation_parsing_accepts_synonyms_and_case():
    from tellfold.models import Recommendation

    assert Recommendation.model_validate(rec(action="Remove", confidence="HIGH")).action == "drop"
    assert Recommendation.model_validate(rec(action="Don’t do")).action == "drop"
    assert Recommendation.model_validate(rec(action="build")).action == "add"
    assert Recommendation.model_validate(rec(action="pilot")).action == "test"
    assert Recommendation.model_validate({"action": "keep", "title": "Keep it"}).confidence is None  # no made-up rating
    with pytest.raises(ValueError):
        Recommendation.model_validate(rec(action="celebrate"))


def test_one_bad_recommendation_does_not_fail_the_report():
    report = Report.model_validate(report_payload(recommendations=[
        rec(title="Keep going"), rec(action="celebrate"), {"action": "drop"}, "not an object", rec(action="Remove"),
    ]))
    assert [(r.action, r.title) for r in report.recommendations] == [
        ("change", "Keep going"), ("drop", "Publish the revenue figures before launch"),
    ]


async def test_report_prompt_asks_for_grounded_recommendations(world, graph, config):
    llm = FakeLLM(lambda *_: report_payload())
    await build_report(llm, world, graph, config, model="sonnet")
    prompt = llm.calls[0][2]
    assert "recommendations: 3-6 objects" in prompt
    assert '"drop" - something the crowd clearly rejected' in prompt
    assert "No generic advice" in prompt and "A recommendation without evidence is" in prompt
    assert "tested with real people" in prompt


async def test_build_report_normalises_recommendations(world, graph, config):
    payload = report_payload(camps=CAMPS, recommendations=[
        rec(evidence=["6", "post #2 by @CAB_DRIVER_SAM", "a03", "#6"], addresses=["critics", "@cyclist_ana", "Everyone"]),
        rec(action="drop", title="  Drop the weekend surcharge ", evidence=[], why="Rejected in #2 and #3."),
        rec(action="add", title="Add a generic FAQ", evidence=["the comments"], why="People were confused (#4)."),
        rec(action="keep", title="", evidence=["#1"]),
        rec(action="drop", title="drop the  weekend surcharge", evidence=["#2"]),
        rec(action="test", title="Test without evidence", evidence=["the vibe"], why="Hard to say."),
    ])
    report = await build_report(FakeLLM(lambda *_: payload), world, graph, config, model="sonnet")
    first, second, third = report.recommendations
    assert first.evidence == ["#6", "#2", "@cab_driver_sam", "@cyclist_ana"]
    assert first.addresses == ["Critics", "a03"]  # camp name canonicalised, handle to id, unknown dropped
    assert (second.action, second.title, second.evidence) == ("drop", "Drop the weekend surcharge", ["#2", "#3"])
    # evidence entries that hold no citation fall back to the citations in the prose
    assert (third.title, third.evidence) == ("Add a generic FAQ", ["#4"])
    # the untitled one and the repeated one are gone
    assert report.citation_audit.problems == 0


async def test_recommendations_are_capped_at_six(world, graph, config):
    payload = report_payload(recommendations=[rec(title=f"Step {i}") for i in range(9)])
    report = await build_report(FakeLLM(lambda *_: payload), world, graph, config, model="sonnet")
    assert [r.title for r in report.recommendations] == [f"Step {i}" for i in range(6)]


def test_audit_checks_recommendation_citations(world):
    from tellfold.report import audit_citations

    ev = collect_evidence(world)
    report = Report.model_validate(report_payload(recommendations=[
        rec(evidence=["#999", "@ghost_user"], why='@cab_driver_sam said "the charge is a gift" (#2).')
    ]))
    audit = audit_citations(report, ev.posts, ev.people.values())
    assert audit.unknown_posts == [999] and audit.unknown_handles == ["ghost_user"]
    assert [(q.post_id, q.quote) for q in audit.misquotes] == [(2, "the charge is a gift")]


async def test_bad_recommendation_citations_are_repaired_then_removed(world, graph, config):
    bad = report_payload(recommendations=[
        rec(title="Keep cycle lanes", action="keep", evidence=["#4", "#999"]),
        rec(title="Drop the charge", action="drop", evidence=["#998"], why="Nobody wanted it."),
    ])

    def responder(purpose, prompt, schema):
        if purpose == "report.repair":
            assert "#999 is not a post" in prompt and "#998 is not a post" in prompt
        return bad  # the repair does not fix anything

    report = await build_report(FakeLLM(responder), world, graph, config, model="sonnet")
    assert [(r.title, r.evidence) for r in report.recommendations] == [("Keep cycle lanes", ["#4"])]
    assert report.citation_audit.repaired and report.citation_audit.marked == 2
    assert report.caveats[-1] == "2 citations could not be matched to posts and were marked [unverified]."


async def test_repair_fixes_recommendation_citations(world, graph, config):
    def responder(purpose, prompt, schema):
        evidence = ["#999"] if purpose == "report.write" else ["#6"]
        return report_payload(recommendations=[rec(evidence=evidence)])

    llm = FakeLLM(responder)
    report = await build_report(llm, world, graph, config, model="sonnet")
    assert [c[0] for c in llm.calls] == ["report.write", "report.repair"]
    assert report.recommendations[0].evidence == ["#6"]
    assert report.citation_audit.marked == 0


def test_render_markdown_recommendations():
    report = Report.model_validate(report_payload(recommendations=[
        rec(action="drop", title="Drop the weekend surcharge", confidence="high", addresses=["Critics"]),
    ]))
    md = render_markdown(report)
    assert "## What to do next" in md and "ideas to test with real people" in md
    assert "- **Drop the weekend surcharge** (Drop, high confidence)" in md
    assert "  Likely effect: Drivers would likely move" in md
    assert "  For: Critics" in md and "  Based on: #6, @cab_driver_sam" in md
    assert md.index("## What to do next") < md.index("## Risks")


def test_render_html_recommendations(world, config):
    report = Report.model_validate(report_payload(camps=CAMPS, recommendations=[
        rec(action="keep", title="Keep the cycle lanes", evidence=["#4"], addresses=["Supporters", "a03"]),
        rec(action="test", title="Test a lower charge", evidence=["@quiet_reader"], confidence=None),
    ]))
    html = render_html(report, world, config)
    assert 'aria-labelledby="next"' in html and "What to do next" in html
    assert '<span class="act act-keep" title="The crowd valued this. Protect it."><span class="dot"></span>Keep</span>' in html
    assert '<span class="act act-test" title="The crowd split' in html
    section = html[html.index('aria-labelledby="next"') : html.index("</ol>", html.index('aria-labelledby="next"'))]
    assert section.count("confidence</span>") == 1  # the step without a confidence shows no rating
    # the action label is drawn in ink; its colour is on the dot and the border only
    assert ".act { --act: var(--ink-2);" in html and "color: var(--ink);" in html
    assert 'For: Supporters, <a class="ref" href="#agent-cyclist_ana">@cyclist_ana</a>' in html
    assert 'id="p-4"' in html  # evidence posts are in the cited posts, so the link resolves
    for pid in re.findall(r'href="#p-(\d+)"', html):
        assert f'id="p-{pid}"' in html
    css = html[html.index("@media print {") : html.index("</style>")]
    assert ", .rec { break-inside: avoid; }" in css  # a recommendation card is never split across PDF pages
    assert "What to do next" not in render_html(Report.model_validate(report_payload()), world, config)


def test_markdown_leaves_out_a_missing_confidence():
    md = render_markdown(Report.model_validate(report_payload(recommendations=[rec(confidence=None)])))
    assert "- **Publish the revenue figures before launch** (Change)" in md


def test_action_words_match_in_the_studio_and_the_mac_app():
    """The labels, their meanings and the note are written out in Python, app.js and Models.swift; keep them equal."""
    from tellfold.report import ACTION_LABELS, ACTION_MEANINGS, RECOMMENDATIONS_NOTE

    root = Path(__file__).resolve().parents[1]
    js = (root / "tellfold/web/static/js/app.js").read_text(encoding="utf-8")
    swift = (root / "mac/Tellfold/App/Models/Models.swift").read_text(encoding="utf-8")
    assert f'const REC_NOTE = "{RECOMMENDATIONS_NOTE}";' in js
    assert f'static let note = "{RECOMMENDATIONS_NOTE}"' in swift
    for action, label in ACTION_LABELS.items():
        meaning = ACTION_MEANINGS[action]
        assert re.search(re.escape(f'{action}: {{ label: "{label}", icon: "') + r'\w+' + re.escape(f'", tip: "{meaning}" }}'), js), action
        assert f'case .{action}: "{label}"' in swift and f'case .{action}: "{meaning}"' in swift, action
