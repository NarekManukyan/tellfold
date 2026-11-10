# Porting the Tellfold engine to Rust

Status: plan, November 2026. Python 0.3.0 is the reference. Nothing here changes what ships until the last
milestone, and that one waits for the maintainer's approval.

## Why

The app bundles CPython and the engine's dependencies: 186 MB of the 195 MB app, pymupdf alone 57 MB. A run spends
almost all of its time waiting for the AI tool, so speed of the engine itself is not the point. The goals are:

- one small, signed binary instead of a relocatable Python tree;
- faster engine start (the app waits for the `{"port": N}` line on every launch);
- an easier path to Intel Macs, Windows and Linux (a cross-compiled binary, no python-build-standalone per target).

## What must not change

1. **The HTTP contract.** Same routes, methods, status codes, JSON shapes, SSE event names and payloads, bearer
   token (`--token` or `TELLFOLD_SERVE_TOKEN`), local-only guard, and the `tellfold serve --port 0 --runs DIR
   [--examples DIR] --log-level LEVEL` command line with its single `{"port": N}` stdout line.
   Reference: `tellfold/server/`, `mac/Tellfold/App/Networking/TellfoldAPI.swift`.
2. **The CLI.** `run`, `compare`, `check`, `branch`, `rewrite`, `report`, `chat`, `ask`, `agents`, `doctor`,
   `runtimes`, `serve`, `ui`, with the same options, and the deprecated `shoal` alias.
3. **Run folders.** A Rust engine reads and writes the folders 0.3.0 writes, and 0.3.0 can still read what Rust
   writes: `config.json`, `graph.json`, `personas.json`, `report.json`/`.md`/`.html`, `status.json`,
   `calls.jsonl`, `rewrite.json`, `seeds/`, `world.db` (schema below, including `memory_history`), and
   `studies/<id>/{study.json,compare.json,agreement.json,config.json,calls.jsonl}`.
4. **Answers.** The prompts, the output JSON Schemas, the JSON repair loop, the citation audit, the RNG streams and
   the call estimates (`estimate_call_split`, mirrored in the Mac app) stay identical, so a FakeLLM run produces the
   same run folder in both engines.

## Crate layout

A Cargo workspace in `engine-rs/`. Cargo's target directory stays inside it (`engine-rs/target`, ignored by git).

| crate | ports | contents |
|---|---|---|
| `tellfold-models` | `models.py` | serde types, pydantic-compatible validation (lax numbers, recommendation synonyms, dropping bad recommendations), pydantic-identical JSON output, a Python `json.dumps` formatter |
| `tellfold-store` | `world.py`, `server/reader.py`, `study.py`, file parts of `runner.py` | run folder IO (atomic writes, `write_json` layout), `World` on rusqlite, `RunReader`, studies on disk, `estimate_call_split` |
| `tellfold-llm` | `llm.py`, `runtimes/base.py`, `ingest.py` | `Llm` trait, `FakeLlm`, `extract_json`, schema prompts, repair, retries and backoff, `calls.jsonl`, document loading and chunking |
| `tellfold-runtimes` | `runtimes/`, `settings.py` | detection, `settings.json`, Claude Code, CLI tools, HTTP runtimes, routing agent/planner purposes |
| `tellfold-engine` | `graph.py`, `personas.py`, `engine.py`, `runner.py` | graph extraction, personas, round loop, pipeline and `status.json` tracking, Python-compatible RNG |
| `tellfold-report` | `report.py`, `chat.py`, `rewrite.py`, `templates/` | evidence digest, report call, citation audit and repair, HTML/Markdown rendering, interviews, rewrite |
| `tellfold-study` | `compare.py`, `agree.py`, `branch.py`, `examples.py` | comparisons, Check it, branching, packing examples |
| `tellfold-server` | `server/` | axum app, run manager, SSE, watcher for runs written by other processes, uploads, static web UI |
| `tellfold-cli` | `cli.py`, `__main__.py` | the `tellfold` binary (and `shoal` alias), every command, the live terminal view |

Dependencies point down the table: models has none of ours, store depends on models, and so on. The server and the
CLI are the only crates that know about tokio's runtime setup.

## Python dependency replacements

| Python | Rust | notes |
|---|---|---|
| pydantic | `serde`, `serde_json` (`preserve_order`), `indexmap`, `schemars` | output must match `model_dump_json` byte for byte; see "JSON" below |
| FastAPI, uvicorn, python-multipart | `axum`, `tower-http`, `tokio`, axum `multipart` | SSE through `axum::response::sse` |
| typer, rich | `clap` (derive), `indicatif` + `console` for the live view, `comfy-table` for tables | `NO_COLOR` honoured as now |
| jinja2 | `minijinja` | `templates/report.html` is ported as it is; filters re-implemented where minijinja differs |
| pymupdf | `pdf-extract` on `lopdf`, with `pdfium-render` as fallback if text quality falls short | see "Risks" |
| charset-normalizer | `chardetng` + `encoding_rs` | |
| sqlite3 | `rusqlite` (bundled SQLite, `backup` feature) | |
| httpx | `reqwest` (rustls) | |
| anyio, asyncio subprocess | `tokio` (`process`, `sync::Semaphore`, `time`) | process groups killed with `nix::sys::signal::killpg` |
| claude-agent-sdk | the `claude` CLI directly | see below |
| `random.Random` | own MT19937 port | see "RNG" |
| `hashlib.blake2b(digest_size=8)` | `blake2::Blake2bVar` with an 8-byte output | feed tie-breaks |

## Calling the AI tools without the Python SDK

**Claude Code.** The SDK is a wrapper around the `claude` binary. Rust calls it directly:

```
claude -p --output-format json --model <alias> --system-prompt <system> --max-turns 1
       --tools "" --setting-sources "" --strict-mcp-config --no-session-persistence
       [--json-schema <schema>]      # structured output for llm.json()
```

The prompt goes in on stdin. `--setting-sources ""` keeps the user's CLAUDE.md, hooks and plugins out of the
simulation, as `setting_sources=[]` does today; `--tools ""` matches `tools=[]`. The JSON result carries `result`,
`structured_output`, `is_error`, `subtype` and `api_error_status`, which map onto the same `Transient` / `Fatal` /
`BadOutput` classification `llm.py` uses. The process runs in its own process group in the isolated working
directory `IsolatedClaudeLocal` uses, with the same blanked secret variables. Extended thinking for planner calls
(`think`) maps to the CLI's thinking option that the SDK sets today; the exact flag is confirmed against the
installed CLI before milestone 3 lands. The flags are pinned by a test that runs a fake `claude` script and checks
its argv.

**Every other tool** is already called as a subprocess or over HTTP in `tellfold/runtimes/cli_tools.py` and
`http_api.py`: Cursor Agent, Codex, Gemini CLI, Qwen Code, OpenCode, Copilot, Grok, Droid, Ollama, OpenAI-compatible
servers and the Anthropic and OpenAI APIs. Each adapter is ported with the same argv, environment, output parsing
(`parse_json_reply`, `collect_event_text`) and failure classification.

## JSON

- **Files** are written the way pydantic writes them. pydantic-core serialises with Rust's `serde_json`, so floats
  and string escapes already agree; the port keeps field order (struct declaration order), writes `null` for
  `None`, keeps dict insertion order (`IndexMap`), and copies `write_json`'s list layout (`[\n` + items at column 0
  joined by `,\n` + `\n]\n`).
- **Python `json.dumps`** output (memory notes in `world.db`, `calls.jsonl`, the schema inside JSON-mode prompts)
  uses `", "` and `": "` separators and Python's float repr. `tellfold-models::pyjson` reproduces it.
- **Reading** follows pydantic's lax mode where models (or older files) need it: integers from integral floats and
  numeric strings, floats from integers and numeric strings, booleans from `"true"`/`"false"`/`0`/`1`; unknown
  fields ignored; `null` refused where the field is not optional.
- **`extract_json`** keeps `JSONDecoder(strict=False)`: raw line breaks and tabs inside strings are accepted, the
  fenced blocks are tried before the whole text, and the preferred opener (`{` or `[`) wins.
- **Schemas** sent to models must be the ones pydantic produces today, because their text is part of the prompt.
  `schemars` generates them; a golden test compares each with `model_json_schema()` from Python. Where attributes
  cannot make the two agree, the pydantic schema is checked in and used as is.

## RNG

`engine.py` seeds `random.Random` with strings (`"shoal:active:{seed}:{round}"`) and uses `random()` and
`_weighted_sample`. The port implements Python's string seeding (bytes plus their SHA-512, `init_by_array`), MT19937,
`random()` (53-bit from two outputs), `getrandbits`, `_randbelow`, `shuffle`, `sample` and `choice` exactly, and is
tested against sequences dumped from CPython. Float operations keep Python's order (`x ** y` is `powf`, not `powi`),
so the same seed picks the same agents.

## world.db

The schema is unchanged (tables `personas`, `follows`, `posts`, `reactions`, `actions`, `memories`,
`memory_history`, `round_stats`, `events`, `post_sentiment`), created with `IF NOT EXISTS`, WAL with
`synchronous=NORMAL` for writers, `mode=ro` for readers and `mode=ro&immutable=1` for bundled examples. JSON columns
hold compact pydantic JSON (`personas.data`, `actions.data`, `round_stats.data`) and `json.dumps` lists
(`memories.notes`, `memory_history.notes`). Older databases without `post_sentiment` or `memory_history` gain them
on open, as today.

## Milestones

Each milestone is one pull request against `main`, reviewed and merged only with the maintainer's OK. Python keeps
shipping until milestone 10.

| # | milestone | parity is proven by |
|---|---|---|
| 1 | Workspace, `tellfold-models`, `tellfold-store`: types, run folder IO, `World`, `RunReader`, studies on disk, `estimate_call_split` | `test_world.py`, the study-on-disk and reader tests ported; the bundled example runs read and re-written byte for byte; `RunReader` output for both examples equal to Python's (golden JSON); estimate reference table |
| 2 | `tellfold-llm`: `Llm` trait, `FakeLlm` and the conftest routing helper, `extract_json`, prompts for JSON mode and repair, retries, `calls.jsonl`, ingest and chunking | `test_llm.py`, `test_ingest.py`; schema golden test against `model_json_schema()`; chunk golden test on the example seeds and a PDF fixture |
| 3 | `tellfold-runtimes`: settings, detection, Claude Code via the CLI, the other CLI tools, HTTP runtimes, routing | `test_runtimes.py` with fake binaries and a local HTTP stub; argv golden tests; one manual run each with Claude Code and Cursor Agent |
| 4 | `tellfold-engine`: Python RNG, graph, personas, the round loop, `run_pipeline`, status tracking | `test_graph.py`, `test_personas.py`, `test_engine.py`, `test_runner.py`; golden FakeLLM scenario: the whole run folder (normalised timestamps, pids and timings) equal to Python's |
| 5 | `tellfold-report`: digest, report call, citation audit and repair, HTML and Markdown, chat and ask, rewrite | `test_report.py`, `test_rewrite.py`; rendered `report.html`/`.md` equal to Python's for the golden runs and both examples |
| 6 | `tellfold-study`: compare, Check it, branch, examples packing | `test_study.py`, `test_check.py`, `test_branch.py`, `test_examples.py`; golden study folders |
| 7 | `tellfold-cli`: every command, live view, `doctor`, `runtimes`, `shoal` alias, legacy names | `test_cli.py`, `test_rename_compat.py`; golden `--help` texts; CLI runs equal to Python's under `TELLFOLD_FAKE_LLM=1` |
| 8 | `tellfold-server`: all routes, SSE, run manager, watcher, uploads, auth, shutdown, web UI | `test_server.py`, `test_runtimes_server.py`, `test_serve_shutdown.py`; an API recorder replays the same request script against both servers and diffs every response and SSE stream |
| 9 | Parity soak: the Mac app's test suite against the Rust binary, real end-to-end runs with Claude Code and Cursor Agent, size and start-up numbers | Mac tests green with `enginePath` pointing at the Rust binary; side-by-side real runs reviewed by hand |
| 10 | Switch (after approval): `bundle-engine.sh` builds and signs the Rust binary, `EngineCommand` starts `Resources/engine/tellfold serve ...`; Debug builds keep the `uv` path as an option until Python is retired | a Release build of the app, notarisation dry run, Mac tests |

Milestones 2 and 3 can overlap, as can 5 and 6 once 4 has landed.

## Golden tests

`engine-rs/golden/make.py` runs with `uv run python engine-rs/golden/make.py` and writes fixtures from the Python
engine into `engine-rs/golden/fixtures/`: reader output for the example runs now, and later FakeLLM run folders,
rendered reports, schemas, RNG sequences and recorded API sessions. The Rust tests read the fixtures, so `cargo test`
needs neither Python nor network. Changing Python behaviour means regenerating the fixtures in the same pull request,
which makes every parity change visible in review.

Normalised before comparing: `started_at`, `updated_at`, `created_at`, `pid`, `seconds`/`llm_seconds`, and absolute
paths of temporary folders. Nothing else.

## Risks

- **PDF text.** No Rust crate extracts text exactly like MuPDF. Chunks from PDFs may differ in whitespace and
  reading order, which changes graph prompts for PDF seeds (not for pasted text or Markdown). Milestone 2 measures
  this on real PDFs; if `pdf-extract` is poor, `pdfium-render` with a bundled pdfium (about 5 MB) is the fallback.
  Existing runs are not affected: they keep their chunks' results in `graph.json`.
- **Claude Code flags.** The CLI is updated often. The argv test pins what we pass, `doctor` checks the flags the
  installed version accepts, and a missing `--json-schema` falls back to JSON mode as the SDK path does today.
- **Template differences.** minijinja and jinja2 differ in a few filters and in whitespace control. Golden HTML
  diffs catch them.
- **SQLite versions.** rusqlite bundles its own SQLite, newer than or equal to macOS's. The schema uses nothing
  version-specific; old databases are covered by fixtures.
