"""Shared data models. Every module imports its types from here; do not redefine them elsewhere."""

from __future__ import annotations

from enum import Enum
from typing import Literal

from pydantic import BaseModel, Field
from pydantic.json_schema import SkipJsonSchema


# ---------- knowledge graph ----------

class Chunk(BaseModel):
    id: int
    source: str  # file name the chunk came from
    text: str


class Entity(BaseModel):
    id: str  # slug, e.g. "acme-corp"
    name: str
    kind: str  # person | organization | group | product | place | concept | event
    summary: str
    source_chunk_ids: list[int] = Field(default_factory=list)


class Relation(BaseModel):
    source: str  # Entity.id
    target: str  # Entity.id
    label: str  # short verb phrase, e.g. "competes with"
    evidence: str = ""


class KnowledgeGraph(BaseModel):
    entities: list[Entity] = Field(default_factory=list)
    relations: list[Relation] = Field(default_factory=list)


# ---------- agents ----------

class Persona(BaseModel):
    id: str  # "a01", "a02", ...
    handle: str  # social handle without "@", e.g. "maria_dev"
    name: str
    bio: str  # one or two sentences shown on the profile
    background: str  # private: who they are, what they care about
    stance: str  # private: starting attitude toward the topic of the question
    traits: list[str] = Field(default_factory=list)  # e.g. ["skeptical", "early adopter"]
    activity: float = 0.6  # 0..1, probability of acting in a given round
    influence: float = 0.5  # 0..1, initial follower weight
    entity_id: str | None = None  # set when the persona *is* a graph entity (a company, a public figure)


class AgentMemory(BaseModel):
    agent_id: str
    notes: list[str] = Field(default_factory=list)  # newest last; engine keeps the last N


# ---------- social world ----------

class ActionType(str, Enum):
    POST = "post"
    REPLY = "reply"
    REPOST = "repost"
    LIKE = "like"
    DISLIKE = "dislike"
    FOLLOW = "follow"
    NOTHING = "nothing"


class Action(BaseModel):
    """One thing an agent decided to do in a round, as returned by the LLM."""

    agent_id: str
    type: ActionType
    content: str = ""  # text for post / reply / repost-with-comment
    target_post_id: int | None = None  # reply / repost / like / dislike
    target_agent_id: str | None = None  # follow
    reasoning: str = ""  # private one-liner; kept for the report, never shown in the feed


class Post(BaseModel):
    id: int
    round: int
    author_id: str
    content: str
    parent_id: int | None = None  # set for replies
    repost_of: int | None = None  # set for reposts
    likes: int = 0
    dislikes: int = 0
    reposts: int = 0
    replies: int = 0


class GodEvent(BaseModel):
    """A variable injected from the "god's-eye view" at the start of a round."""

    round: int
    content: str  # shown to every agent as breaking news


class RoundStats(BaseModel):
    round: int
    actions: dict[str, int] = Field(default_factory=dict)  # ActionType value -> count
    new_posts: int = 0
    active_agents: int = 0
    sentiment: float | None = None  # -1..1 average, filled by the engine's per-round sentiment pass
    llm_calls: int = 0
    llm_seconds: float = 0.0


# ---------- run config / results ----------

class SimConfig(BaseModel):
    question: str  # what the user wants predicted
    seed_files: list[str]
    num_agents: int = 20
    rounds: int = 6
    batch_size: int = 6  # agents decided per LLM call
    feed_size: int = 12  # posts each agent sees per round
    memory_size: int = 8  # notes kept per agent
    agent_model: str = "haiku"  # model used for per-round agent decisions
    planner_model: str = "sonnet"  # model used for extraction, personas, report
    concurrency: int = 4  # parallel CLI calls
    language: str = "English"
    events: list[GodEvent] = Field(default_factory=list)
    seed: int = 7  # RNG seed for activity sampling and feed tie-breaks
    think: bool = False  # extended thinking for graph/personas/report calls (slower, more thorough)
    agent_runtime: str = "claude-code"  # tellfold.runtimes id for engine.* and chat.interview calls
    planner_runtime: str = "claude-code"  # tellfold.runtimes id for graph, personas, report and ask calls


class ReportSection(BaseModel):
    title: str
    body_markdown: str


class Camp(BaseModel):
    """One opinion group in the simulated crowd, named by the report writer."""

    name: str  # short plain label, e.g. "Stayed on free"
    summary: str = ""  # one sentence: what this camp believes, with citations
    members: list[str] = Field(default_factory=list)  # persona ids; every agent sits in exactly one camp
    sentiment: float | None = None  # mean of members' scored posts, filled in by Python


class Misquote(BaseModel):
    post_id: int
    quote: str


class CitationAudit(BaseModel):
    """What Python could verify about the report's citations, filled in after the report call."""

    checked: int = 0  # post ids, handles and quotes found in the report text
    unknown_posts: list[int] = Field(default_factory=list)  # cited #ids that are not posts in this run
    unknown_handles: list[str] = Field(default_factory=list)  # cited @handles that are not agents in this run
    misquotes: list[Misquote] = Field(default_factory=list)  # quotes not found in the post cited next to them
    repaired: bool = False  # a repair call was made because the first draft had problems
    marked: int = 0  # citations still wrong after the repair, replaced in the text with "[unverified]"

    @property
    def problems(self) -> int:
        return len(self.unknown_posts) + len(self.unknown_handles) + len(self.misquotes)


class Report(BaseModel):
    title: str
    summary: str  # 3-5 sentence direct answer to SimConfig.question
    prediction: str  # the single most likely outcome, one paragraph
    confidence: Literal["low", "medium", "high"]
    sections: list[ReportSection] = Field(default_factory=list)
    key_voices: list[str] = Field(default_factory=list)  # persona ids whose posts drove the outcome
    reasons: list[str] = Field(default_factory=list)  # 2-4 one-line reasons for the answer, each with a citation
    camps: list[Camp] = Field(default_factory=list)  # 2-5 opinion camps covering every agent
    risks: list[str] = Field(default_factory=list)
    caveats: list[str] = Field(default_factory=list)
    # Filled in by Python, never asked of the model (kept out of the JSON schema). None in older reports.
    citation_audit: SkipJsonSchema[CitationAudit | None] = None


# ---------- run lifecycle (written to runs/<id>/status.json by tellfold.runner) ----------

class RunState(str, Enum):
    QUEUED = "queued"
    RUNNING = "running"
    DONE = "done"
    FAILED = "failed"
    STOPPED = "stopped"


class RunStage(str, Enum):
    READING = "reading"
    GRAPH = "graph"
    PERSONAS = "personas"
    SEEDING = "seeding"
    ROUNDS = "rounds"
    REPORT = "report"
    FINISHED = "finished"


class FailedBatch(BaseModel):
    """One model batch that failed during a run; the run continued without those agents' turn."""

    round: int
    batch: int | str  # batch index, or "sentiment" for the per-round scoring call
    agents: list[str] = Field(default_factory=list)
    error: str = ""


class RunStatus(BaseModel):
    state: RunState
    stage: RunStage
    round: int = 0  # last completed round (0 = opening posts done or not started)
    total_rounds: int
    detail: str = ""  # human-readable current activity, e.g. "round 3: 4 batches deciding"
    error: str | None = None
    pid: int | None = None  # process that owns the run; lets the UI tell a dead CLI run from a live one
    started_at: str  # ISO 8601 UTC
    updated_at: str  # ISO 8601 UTC
    failed_batches: list[FailedBatch] = Field(default_factory=list)  # kept so a reload still shows them
