import re
from pathlib import Path

import pytest

from shoal.chat import ask_report, interview, relevant_posts
from shoal.llm import FakeLLM
from shoal.models import (
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
from shoal.report import (
    _markdown,
    _Refs,
    build_report,
    collect_evidence,
    digest_text,
    render_html,
    render_markdown,
)
from shoal.world import World

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
    from shoal.models import Camp, Persona, Post
    from shoal.report import Evidence, _finalize_camps

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
