# Tellfold architecture (build contract)

Tellfold is a local swarm-simulation tool inspired by the *idea* of MiroFish (seed documents in, simulated
social reaction and a prediction report out). It is an original implementation: no MiroFish code, no Zep,
no OASIS, no cloud services. The only LLM is the user's locally installed **Claude Code CLI**, driven through
the Python **claude-agent-sdk** on the user's logged-in subscription. No API key.

## Clean-room rule

This is a clean-room implementation. Builders work only from this document, `tellfold/models.py`, and
public documentation of the libraries we depend on (claude-agent-sdk, pydantic, typer, rich, jinja2,
pymupdf, sqlite3). Do **not** open, clone, fetch, search or recall the MiroFish repository, its forks,
DeepWiki pages about it, or the OASIS / camel-ai source. Do not reuse its prompts, file names, function names,
data layout or UI text. Design every prompt, schema and algorithm independently from the contract below.

Python >= 3.11, managed with `uv`. Package: `tellfold/`. Tests: `tests/`. All shared types live in
`tellfold/models.py` (already written — import from it, never redefine).

## Pipeline

```
seed files ──ingest──▶ chunks ──graph──▶ KnowledgeGraph ──personas──▶ [Persona]
                                                                     │
                     god events ───────────────────────────────▶ engine (rounds × batches)
                                                                     │  writes posts/actions to World
                                                                     ▼
                                                         report ──▶ report.json + report.html
                                                         chat   ──▶ interview any agent afterwards
```

## Run directory

Every run lives in `runs/<run_id>/` (run_id = `YYYYMMDD-HHMMSS-<slug>`; created by the CLI):

| file | written by | content |
|---|---|---|
| `config.json` | cli | `SimConfig` |
| `world.db` | world | SQLite: everything below in tables |
| `graph.json` | graph | `KnowledgeGraph` |
| `personas.json` | personas | `list[Persona]` |
| `calls.jsonl` | llm | one line per LLM call: ts, purpose, model, seconds, ok, error, prompt_chars, output_chars |
| `report.json` / `report.html` / `report.md` | report | `Report` + rendered pages |

Runs compared with each other form a **study**, kept in `runs/studies/<study_id>/`: `study.json` (`Study`: the
member runs and their labels, "Draft A", "Draft B"), `compare.json` (`CompareResult`) and, for the comparison's own
model calls, `config.json` (a copy of the newest member's config) and `calls.jsonl`. The folder has no run of its own.

## Module contracts

### `tellfold/llm.py` — local Claude backend
```python
class LLMError(Exception): ...

class LLM(Protocol):
    async def text(self, prompt: str, *, system: str, model: str, purpose: str) -> str: ...
    async def json(self, prompt: str, *, system: str, model: str, purpose: str,
                   schema: type[BaseModel]) -> BaseModel: ...

class ClaudeLocal:  # implements LLM
    def __init__(self, *, concurrency: int = 4, log_path: Path | None = None,
                 cli_path: str | None = None, timeout_s: float = 240, retries: int = 2): ...

class FakeLLM:  # implements LLM, for tests: deterministic, no subprocess
    def __init__(self, responder: Callable[[str, str, type[BaseModel] | None], str | dict]): ...
```
`LLM`, `LLMError` and `FakeLLM` are **already written** in `tellfold/llm.py` — use them as-is. FakeLLM's
responder is called as `responder(purpose, prompt, schema)` (sync or async); return a str for `text()`,
a dict or JSON string for `json()`, or an `Exception` instance to make the call raise it. `FakeLLM.calls`
records `(purpose, model, prompt)`. Every module must pass a short, stable `purpose` string to each call
(e.g. `"graph.extract"`, `"personas.batch"`, `"engine.round"`, `"engine.sentiment"`, `"report.write"`, `"report.repair"`,
`"chat.interview"`, `"chat.ask"`) so tests can route fake responses by purpose.
- Uses `claude_agent_sdk.query` with `ClaudeAgentOptions(system_prompt=system, model=model, max_turns=1,
  tools=[], allowed_tools=[], setting_sources=[], cli_path=cli_path or shutil.which("claude"))`.
  `setting_sources=[]` is required so the user's own CLAUDE.md, hooks and plugins do not leak into
  simulation prompts. Verify option names against the *installed* SDK source before relying on them.
- `json()`: prefer the SDK's structured output (`output_format={"type": "json_schema", "schema": ...}` →
  `ResultMessage.structured_output`) if the installed SDK supports it; otherwise instruct "reply with only
  JSON matching this schema", extract the first JSON object/array from the text, validate with pydantic,
  and on validation failure retry once with the error appended. Raise `LLMError` after retries.
- Concurrency via an `anyio.Semaphore`; per-call timeout; exponential backoff on rate-limit / process errors.
- Append one JSONL record per call to `log_path` (never log full prompts — only lengths).
- Model aliases `haiku`, `sonnet`, `opus` are passed straight to the CLI.

### `tellfold/ingest.py`
```python
def load_documents(paths: list[Path]) -> list[tuple[str, str]]  # (source name, text); .pdf .md .txt .markdown
def chunk_text(docs: list[tuple[str, str]], size: int = 1800, overlap: int = 200) -> list[Chunk]
```
PDF via `pymupdf`; text via `charset-normalizer` for non-UTF-8 files. Chunk on paragraph boundaries where possible.

### `tellfold/graph.py` — local knowledge store (replaces Zep)
```python
async def extract_graph(llm: LLM, chunks: list[Chunk], *, question: str, model: str,
                        on_progress: Callable[[str], None] | None = None) -> KnowledgeGraph
```
- One LLM call per group of chunks (≈4 chunks/call), run concurrently, then a merge step that dedupes
  entities by normalized name/slug and drops relations pointing at unknown ids. Pure-python merge; no LLM.
- Prompts steer extraction toward actors who would *react* to the question (people, orgs, groups, media).

### `tellfold/world.py` — the simulated social platform (SQLite)
```python
class World:
    def __init__(self, db_path: Path): ...                 # creates tables if absent
    def add_personas(self, personas: list[Persona]) -> None
    def personas(self) -> list[Persona]
    def persona(self, agent_id: str) -> Persona
    def apply(self, round: int, action: Action) -> int | None   # returns new post id for post/reply/repost
    def feed_for(self, agent_id: str, round: int, k: int) -> list[Post]
    def post(self, post_id: int) -> Post
    def posts(self, round: int | None = None) -> list[Post]
    def followers(self, agent_id: str) -> list[str]
    def following(self, agent_id: str) -> list[str]
    def actions(self, round: int | None = None) -> list[tuple[int, Action]]
    def save_memory(self, mem: AgentMemory) -> None
    def memory(self, agent_id: str) -> AgentMemory
    def save_round_stats(self, stats: RoundStats) -> None
    def round_stats(self) -> list[RoundStats]
    def add_event(self, event: GodEvent) -> None
    def events(self) -> list[GodEvent]
```
- `apply` validates: unknown target post / agent → downgrade to `nothing` (never crash on LLM output);
  no self-follow; one like/dislike per agent per post; counters on `Post` kept in sync.
- `feed_for` ranking: posts from followed agents and replies to the agent's own posts first, then by
  engagement score `likes + 2*reposts + replies - dislikes` weighted by author influence and recency,
  excluding the agent's own posts. Deterministic for a given seed.

### `tellfold/personas.py`
```python
async def generate_personas(llm: LLM, graph: KnowledgeGraph, *, question: str, n: int, model: str,
                            language: str) -> list[Persona]
```
- Mix of (a) personas that embody key graph entities (orgs, public figures) and (b) ordinary members of the
  affected public with diverse demographics, incentives and starting stances (supporters, critics, neutral,
  indifferent). Generate in batches of ≤10 per call with the already-made handles listed to avoid duplicates.
  Assign ids `a01..aNN`, unique handles, clamp activity/influence to 0..1.

### `tellfold/engine.py` — the round loop
```python
class Engine:
    def __init__(self, llm: LLM, world: World, config: SimConfig, *,
                 on_event: Callable[[str, dict], None] | None = None): ...
    async def seed_world(self, graph: KnowledgeGraph) -> None   # initial follow graph + round-0 opening posts
    async def run(self) -> list[RoundStats]
```
Per round r:
1. Inject `GodEvent`s for r (stored in world, shown to all agents as breaking news this round).
2. Sample active agents by `activity` (seeded RNG; at least one agent).
3. Split active agents into batches of `batch_size`; one `llm.json(...)` call per batch, concurrently.
   The prompt holds, per agent: persona (incl. private background/stance), memory notes, and that
   agent's own feed (`feed_for`). Response schema: `{"decisions": [{agent_id, actions: [Action...] (1–3),
   memory_note}]}`. Agents may only reference post ids that were in *their* feed.
4. Apply actions in a deterministic order (by agent id), append memory notes (keep last `memory_size`).
5. Cheap sentiment pass: one `llm.json` call over the round's new posts → mean sentiment −1..1 toward
   the question's subject; store `RoundStats`.
6. `on_event("round_done", {...})` for the CLI live view.
The engine must tolerate a failed batch (log it, those agents do nothing this round) and keep going.

### `tellfold/report.py`
```python
async def build_report(llm: LLM, world: World, graph: KnowledgeGraph, config: SimConfig, *,
                       model: str) -> Report
def render_html(report: Report, world: World, config: SimConfig) -> str   # self-contained, no CDN
def render_markdown(report: Report) -> str
```
- Python first computes evidence: per-round stats, top posts by engagement, most-followed agents,
  stance shift (first vs last posts of each agent), notable threads. The LLM gets that digest (not the raw
  DB) and returns a `Report`. Report must answer `config.question` directly and cite post ids / handles.
- HTML (Jinja2 template in `tellfold/templates/report.html`): report sections, sentiment-by-round chart (inline
  SVG), action mix per round, top threads rendered as a feed, persona directory. Light/dark via
  `prefers-color-scheme`. No external assets.

### `tellfold/compare.py` and `tellfold/study.py` — studies
```python
def plan_variant(runs_dir, source_id, source: SimConfig, *, study_id=None, variant=None) -> tuple[Study, str]
def join_study(runs_dir, study: Study, run_id: str, variant: str) -> Study
def digest_run(member: Member, state: str = "") -> RunDigest
async def build_compare(llm, study_id, members: list[Member], *, question, model, language) -> CompareResult
```
- A run with `SimConfig.cast_from` set reuses the source run's `graph.json` and `personas.json`: no graph or persona
  calls; both stages are still announced (with `reused: true`). It joins the source's study, or starts one with the
  source labelled "Draft A". A read-only example joins through `study.json` only.
- Python digests every run (sentiment by round, final sentiment, camps, stance shifts, objections from the report's
  risks, posts per round). One `compare.write` call gets the digests and each version's text and returns a
  `CompareReport`. Posts are cited `A#12` (post 12 of the study's first run); the citation audit checks those refs
  against the right run and marks the ones that don't match `[unverified]`.
- The server writes the comparison by itself once every member has finished with a report.
- "Check it" (`tellfold/agree.py`, `Study.kind == "repeat"`): a finished run is run again 1-4 times with the same
  people, text and settings and the next RNG seeds, one run after another. Python measures the sentiment spread (at
  the end and per round); `study.match_camps` matches camps across runs and `study.agree` sorts the conclusions into
  claims that held in every run and claims that split, with a verdict (consistent, mixed, unstable). Written to
  `studies/<id>/agreement.json`.

### `tellfold/chat.py`
```python
async def interview(llm: LLM, world: World, agent_id: str, question: str, *, model: str,
                    history: list[tuple[str, str]] | None = None) -> str
async def ask_report(llm: LLM, world: World, report: Report, question: str, *, model: str) -> str
```
Agent answers in character using persona + memory + its own posts and replies.

### `tellfold/cli.py` (Typer app `app`, entry point `tellfold`)
```
tellfold run SEED... -q "question" [--agents 20] [--rounds 6] [--batch 6] [--agent-model haiku]
          [--planner-model sonnet] [--concurrency 4] [--event "3:Breaking news text"]... [--out runs]
          [--language English] [--open]
          [--cast-from RUN_DIR] [--variant "Draft B"]   # reuse a run's people, join its study
tellfold compare RUN_DIR           # compare the runs of RUN_DIR's study (studies/<id>/compare.json)
tellfold check RUN_DIR [--runs 3]  # run it again with other seeds; what held (studies/<id>/agreement.json)
tellfold report RUN_DIR            # rebuild report from an existing run
tellfold chat RUN_DIR --agent a03  # interactive interview (REPL)
tellfold ask RUN_DIR "question"    # one-shot question to the report agent
tellfold agents RUN_DIR            # table of personas
tellfold doctor                    # checks claude CLI on PATH, logged in (tiny haiku call), SDK import
```
Live progress with `rich` (stage spinners, per-round table: active agents, posts, sentiment).
`--event` may repeat; format `ROUND:TEXT`. `--open` opens report.html in the browser at the end.
Ctrl-C mid-run keeps everything written so far; `tellfold report` can rebuild from it.

## Testing
- `tests/` uses `FakeLLM` only — no test may spawn the real CLI. Cover: chunking, graph merge/dedup,
  world.apply validation + feed ranking, engine round loop with a fake responder (including a failing batch
  and invalid post ids), report digest + HTML render, CLI `--help` and a full fake run via Typer's CliRunner
  (inject the fake via an env var `TELLFOLD_FAKE_LLM=1` that `cli.py` honours).
- Run with `uv run pytest -q`.

## Ownership (build team)
Each builder edits only its own files. If you need something from another module, code against the
contract above exactly; do not edit the other module.

| builder | files |
|---|---|
| llm | `tellfold/llm.py`, `tests/test_llm.py`, `tests/conftest.py` (shared FakeLLM fixtures) |
| knowledge | `tellfold/ingest.py`, `tellfold/graph.py`, `tellfold/personas.py`, `tests/test_ingest.py`, `tests/test_graph.py`, `tests/test_personas.py` |
| world | `tellfold/world.py`, `tellfold/engine.py`, `tests/test_world.py`, `tests/test_engine.py` |
| report | `tellfold/report.py`, `tellfold/chat.py`, `tellfold/templates/report.html`, `tests/test_report.py` |
| cli | `tellfold/cli.py`, `tellfold/__init__.py`, `tellfold/__main__.py`, `README.md`, `examples/`, `tests/test_cli.py` |
