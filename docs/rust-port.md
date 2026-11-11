# Porting the Tellfold engine to Rust

Status: November 2026. Python 0.3.0 is the reference. Milestones 1 to 9 left what ships unchanged; milestone 10
switches the Mac app to the Rust engine (see "The switch").

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
| pydantic | `serde`, `serde_json` (`preserve_order`), `indexmap` | output must match `model_dump_json` byte for byte; schemas are pydantic's, checked in; see "JSON" below |
| FastAPI, uvicorn, python-multipart | `axum` (with `multipart`), `tokio` | SSE written as a plain streamed body; see "The server" below |
| typer, rich | own click-compatible parser and a port of the rich subset the CLI prints (`tellfold-cli`) | see "The command line" below; help, errors, tables and panels match byte for byte |
| jinja2 | `minijinja` | `templates/report.html` is ported as it is; filters re-implemented where minijinja differs |
| pymupdf | `pdf-extract` on `lopdf`, with `pdfium-render` as fallback if text quality falls short | see "Risks" |
| charset-normalizer | `chardetng` + `encoding_rs` | |
| sqlite3 | `rusqlite` (bundled SQLite, `backup` feature) | |
| httpx | `reqwest` (rustls) | |
| anyio, asyncio subprocess | `tokio` (`process`, `sync::Semaphore`, `time`) | each child leads its own process group (`process_group(0)`), killed with `libc::killpg` |
| claude-agent-sdk | the `claude` CLI directly | see below |
| `random.Random` | own MT19937 port | see "RNG" |
| `hashlib.blake2b(digest_size=8)` | `blake2::Blake2bVar` with an 8-byte output | feed tie-breaks |

## Calling the AI tools without the Python SDK

**Claude Code.** The SDK is a wrapper around the `claude` binary, and Rust drives that binary the same way the SDK
does (checked against claude-agent-sdk 0.2.179 and Claude Code 2.1.283):

```
claude --output-format stream-json --verbose --system-prompt <system> --tools "" --max-turns <1|3>
       --model <alias> --strict-mcp-config --setting-sources= [--thinking disabled] [--json-schema <schema>]
       --input-format stream-json --no-session-persistence -p
```

The argv up to `--input-format stream-json` is the SDK's own `_build_command()` output for the same options, pinned
by a golden test. `--no-session-persistence` keeps simulation calls out of the user's Claude Code history, and `-p`
replaces the SDK's initialize handshake. The prompt goes in on stdin as one stream-json user message marked
`client_composed: true`, which is what the SDK's `verbatim_prompts=True` sends: Claude Code then delivers the text as
written, with no `@path` expansion and no slash commands. `--setting-sources=` keeps the user's CLAUDE.md, hooks and
plugins out of the simulation, as `setting_sources=[]` does; `--tools ""` matches `tools=[]`. Thinking is turned off
with `--thinking disabled` (the SDK's `thinking={"type": "disabled"}`) except for the think purposes. The result
message carries `result`, `structured_output`, `is_error`, `subtype`, `api_error_status` and `terminal_reason`, which
map onto the same `Transient` / `BadOutput` / not-logged-in classification `llm.py` uses (golden-tested). The process
runs in its own process group in `$TMPDIR/tellfold-runtime-cwd`, with the engine's environment and every secret
blanked, as `IsolatedClaudeLocal` does. `calls.jsonl` lines keep `ClaudeLocal`'s shape (`turns`, `api_seconds`,
`startup_seconds`, no `runtime` key). A test runs a fake `claude` script and checks its argv, stdin, working
directory and environment.

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
  uses `", "` and `": "` separators and Python's float repr. `tellfold_models::json::py_dumps` reproduces it.
- **Reading** follows pydantic's lax mode where models (or older files) need it: integers from integral floats and
  numeric strings, floats from integers and numeric strings, booleans from `"true"`/`"false"`/`0`/`1`; unknown
  fields ignored; `null` refused where the field is not optional.
- **`extract_json`** keeps `JSONDecoder(strict=False)`: raw line breaks and tabs inside strings are accepted, the
  fenced blocks are tried before the whole text, and the preferred opener (`{` or `[`) wins.
- **Schemas** sent to models must be the ones pydantic produces today, because their text is part of the prompt.
  pydantic's output carries docstrings, field titles and defaults that no derive macro reproduces exactly, so the
  schemas are checked in: `golden/make.py` writes `engine-rs/schemas/<Class>.json` from `model_json_schema()`, the
  crates embed them, and a test checks each Rust type writes exactly the schema's properties. When a Python model
  changes, its schema file is regenerated in the same pull request.
- **Validation messages** differ. Python's repair prompt quotes pydantic's `ValidationError` text after
  `Problem:`; Rust quotes serde's message (`missing field \`n\``). The rest of the repair prompt is identical. Only
  the second attempt of an already failing call sees this text.

## RNG

`engine.py` seeds `random.Random` with strings (`"shoal:active:{seed}:{round}"`) and uses `random()`, `shuffle`,
`choices` and `_weighted_sample`; `personas.py` shuffles with `random.Random(f"{question}|{n}")`. The port
(`tellfold_engine::rng::PyRandom`) implements Python's string seeding (bytes plus their SHA-512, `init_by_array`),
MT19937, `random()` (53-bit from two outputs), `getrandbits`, `_randbelow`, `shuffle` and `choices` exactly, and is
tested against sequences dumped from CPython. Float operations keep Python's order (`x ** y` is `powf`, not `powi`;
`round()` is half-even; float sums use CPython 3.12's compensated `sum`), so the same seed picks the same agents,
follows and opening authors.

## The engine (graph, personas, rounds, pipeline)

- **Unicode.** `slugify` and `select_chunks` use `re`'s `\w` and `unicodedata.combining`. Their tables are generated
  from Python by `golden/make.py` into `tellfold-engine/src/unicode_tables.rs`, so slugs and word matches follow
  Python's Unicode version rather than Rust's.
- **Concurrency.** Python starts every batch of a round as a task before awaiting the first one, then applies them
  in batch order. The port builds every prompt first, runs the calls concurrently and applies the replies in batch
  order, polling every unfinished call before each apply (`in_order` in `engine.rs`). Events from calls that finish
  early (`batch_failed`) therefore come in the same order as in Python; the golden run checks the whole event stream.
- **Threads.** `World` is not `Sync`, so the engine's futures are not `Send`: a run lives on one thread
  (a current-thread runtime or a `LocalSet`). The model calls of a round still run concurrently.
- **Report stage.** `run_pipeline` takes a `ReportStage` (`build_report` plus `write_report` in Python), so the
  engine does not depend on the report crate. `tellfold_report::ReportWriter` is the real one.
- **Cancellation.** Dropping the `run_pipeline` future marks the run `stopped` and emits `run_stopped`, like the
  Python cancellation branch. A listener that panics is caught and logged, like a listener that raises.
- **Canned responder.** `cli.fake_responder` (what `TELLFOLD_FAKE_LLM=1` runs on) is ported as
  `tellfold_llm::canned::fake_responder` and tested reply for reply against Python on every engine schema.

Known differences, none visible in a normal run:

- `select_chunks` sums a chunk's relevance over a set of words. Python sums in hash order, which changes between
  processes; Rust sums in sorted order. Both use compensated summation, so the result differs at most in the last
  bit, and then only the order of two chunks with equal relevance could change.
- A `target_post_id` given as a string of non-ASCII digits (`"٣"`) becomes null instead of a number.
- A branch run whose `graph.json` is missing fails with the same `[Errno 2] ...` text; a malformed one fails with
  serde's message instead of pydantic's.

## The report (digest, audit, HTML, chat, rewrite)

- **Template.** `tellfold-report/templates/report.html` is a copy of `tellfold/templates/report.html`, and a test
  fails when the two differ. minijinja renders it with Jinja2's settings (`trim_blocks`, `lstrip_blocks`, HTML
  autoescape). Where minijinja differs from Jinja2 the port supplies Jinja2's behaviour: values are escaped like
  `markupsafe.escape` (`&#34;` and `&#39;`, `/` left alone), floats print as Python's `str()`, and `truncate`,
  `format` (`"%.2f"`), `capitalize` and the report's own `signed` filter are re-implemented. `dict.get` and the
  other Python methods the template calls come from minijinja-contrib's `pycompat`.
- **Regular expressions.** The citation, quote, sentence and Markdown patterns use look-behinds, so they run on
  `fancy-regex`. `\w`, `\s` and `\d` are spelled out from Python's own tables (`WORD`, `DECIMAL` in
  `tellfold-engine/src/unicode_tables.rs`), so `#١` cites post 1 exactly as in Python.
- **Numbers.** `statistics.fmean` is CPython's `fsum` (exact, Shewchuk) divided by the count; plain sums use the
  compensated `sum` of CPython 3.12. Prompt templates are filled in one pass like `str.format`, so a `{digest}`
  inside a question stays as it is.
- **Errors.** `RewriteError` and the `KeyError` of an unknown interviewee are `ReportError::Rewrite` and
  `ReportError::UnknownAgent`, with Python's message text (including `repr()` quoting of the agent name).

Known differences, none visible in a normal run:

- A post id too large for an `i64` is read as unknown (`-1`) where Python keeps the big integer.
- An evidence entry made only of non-decimal digits such as `²` raises in Python (`int("²")`); the port treats it
  as text and drops it like any other entry without a citation.

## Studies (compare, Check it, branch, examples)

- **Crate.** `tellfold-study` ports `compare.py`, `agree.py`, `branch.py` and `examples.py`; the study files on
  disk (`study.py`) were already in `tellfold-store`. It builds on the report crate's evidence, audit and renderer.
- **Branching.** The world is copied with SQLite's backup API from a read-only `file:` URI (`mode=ro`, or
  `mode=ro&immutable=1` for a bundled example), so rows still in the source's WAL file come along, as in Python.
- **Examples.** `pack_example` copies files like `shutil.copy2` (bytes, permissions and modification time), backs
  the world up into a rollback-journal database and vacuums it. The packed `world.db` holds the same rows as
  Python's, but its bytes depend on the SQLite version, so the golden test compares rows, not bytes.
- **Parity.** `golden/make.py` (`study_fixtures`) makes canned runs of the mossbook seed and records the digests,
  three comparisons, a branch prepared and continued to its report, the branch errors, two Check it agreements and a
  packed example; the Rust test makes the same runs and compares every call, file, event and row.

## The command line

- **Crate.** `tellfold-cli` builds the `tellfold` binary from `cli.py` and `__main__.py`. Run under the name
  `shoal` (a symlink or a copy), it prints the old-name notice on stderr and carries on as `tellfold`.
- **Parsing.** typer 0.28 ships its own copy of click, and its rules and messages are the ones ported: options
  anywhere, `--opt=value`, `-qvalue`, `--`, parameters processed in the order given, eager `--help`/`--version`,
  close-match suggestions, `Got unexpected extra argument(s)`, control characters escaped in echoed values. One
  table (`spec.rs`) holds every command's parameters, help text, checks and defaults; the parser and `--help` both
  read it.
- **Rendering.** `term` ports the part of rich 15.0 the commands use (text wrapping and cell widths, markup, styles,
  tables, panels, live display) and typer's help and error layout. Plain output is the same byte for byte; on a
  terminal the same styles are written with rich's ANSI codes. Like rich, a live view or status line only animates
  on a terminal: elsewhere the run view prints its last frame once and a status line prints nothing.
- **Differences.** Rich highlights numbers, paths and quoted strings in coloured output on its own; the port does
  not (plain output is unaffected). Emoji codes such as `:smile:` in model text are not replaced, and a stray
  closing markup tag is skipped instead of raising. `doctor` shows a "Tellfold engine" row in place of the
  `claude-agent-sdk` import and Python rows, since the binary needs neither. Where Python lets an exception through
  with a traceback (an unknown runtime in `compare`, say), the port prints `error: ...` and exits 1.
- **Parity.** `golden/make_cli_help.py` records typer's `--help` for the group and all 13 commands at five widths,
  usage errors, and rich's tables, panels and prints (plain and coloured). `golden/make_cli_runs.py` records a
  56-step session of the Python command on the canned model (runs with events, report, agents, ask, chat, a study
  made with `--cast-from` and compared, Check it, a branch, rewrite, a folder of seeds, doctor, and 35 usage and
  run errors); the Rust test replays it with the binary and compares exit codes, stdout, stderr and every file
  written. `tests/cli.rs` ports the helper tests of `test_cli.py` and the command-line parts of
  `test_rename_compat.py`.

## The server

- **Crate.** `tellfold-server` ports `tellfold/server/` with axum: the JSON API, the SSE stream, the run manager
  and its watcher, uploads, the local-only and token guards, and the Studio frontend. `tellfold serve` and
  `tellfold ui` in `tellfold-cli` start it with the options, messages and stdout line of `cli.py`.
- **One thread for runs.** The engine's futures hold SQLite connections and are not `Send`, and the Python server
  does everything on one event loop. The run manager therefore lives on its own thread with a single-threaded tokio
  runtime; handlers send it closures (`Handle::call`) and await the reply, while connections and SSE streams are
  served on a multi-threaded runtime. Live runs, report rebuilds, comparisons and "Check it" series are local
  tasks of the manager, so a run outlives every client. Each SSE client has a bounded queue that drops its oldest
  events, as in Python.
- **FastAPI's answers.** Errors are `{"detail": "..."}` with the same status codes and texts. JSON bodies and query
  parameters are checked before a handler looks at the run, and a failure is a 422 with pydantic's error list
  (`type`, `loc`, `msg`, `input`, `ctx`), which the Mac app shows as "field: message". Responses are pydantic's
  JSON; SSE data is `json.dumps` with compact separators. Unknown `/api` paths and methods give
  `{"detail": "not found"}`; other GETs serve the frontend, which is embedded in the binary at build time.
- **Shutdown.** On Ctrl-C, SIGTERM or (for `serve`) stdin closing, the server stops accepting, gives open
  connections 3 seconds, then stops live runs (5 seconds' grace, then they are cancelled and marked stopped) and
  ends every SSE stream. As in Python, a timer ends the process 15 seconds after stdin closes, or 5 seconds after
  the server stops, killing any agent CLI still running, if something is stuck.
- **Differences.** File responses carry `last-modified` and `accept-ranges` but no `etag`, and do not serve
  `Range` requests. A malformed multipart body is reported with the parser's own message after "could not read the
  form:". A JSON body that is not JSON gets pydantic's `json_invalid` error with "Expecting value" whatever the
  problem was. `ui` exits like Python after Ctrl-C ("Aborted.", exit 1); `serve` exits 0 after a signal instead of
  re-raising SIGTERM.
- **Parity.** `golden/make_api.py` drives the Python app through 116 steps on the canned model: uploads and their
  errors, a run with events, every read route, chat, ask, rewrite, report rebuild, an example copied in and refused
  writes, a study made with `cast_from` and compared, a branch, Check it, a branch of the example, settings, the
  runtime test, the frontend routes and the guards. `tests/api.rs` sends the same requests to the Rust server and
  compares status codes, content types, headers and bodies after masking ids, timestamps, pids and timings.
  `tests/live.rs` and `tests/studies.rs` port the tests that need a run held mid-round, an SSE client or a run
  written by another process (`test_server.py`, `test_check.py`, `test_examples.py`, `test_runtimes_server.py`), and
  `tellfold-cli/tests/serve.rs` starts the binary the way the Mac app does.

## world.db

The schema is unchanged (tables `personas`, `follows`, `posts`, `reactions`, `actions`, `memories`,
`memory_history`, `round_stats`, `events`, `post_sentiment`), created with `IF NOT EXISTS`, WAL with
`synchronous=NORMAL` for writers, `mode=ro` for readers and `mode=ro&immutable=1` for bundled examples. JSON columns
hold compact pydantic JSON (`personas.data`, `actions.data`, `round_stats.data`) and `json.dumps` lists
(`memories.notes`, `memory_history.notes`). Older databases without `post_sentiment` or `memory_history` gain them
on open, as today.

## Milestones

Each milestone is one pull request against `main`, reviewed and merged only with the maintainer's OK. Python
shipped until milestone 10.

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
| 9 | Parity soak: the Mac app's test suite against the Rust binary, real end-to-end runs with Claude Code and Cursor Agent, size and start-up numbers | Mac tests green against the Rust binary (through `engine-rs/scripts/uv-shim.sh`); side-by-side real runs reviewed by hand; see "Parity soak" |
| 10 | Switch (after approval): `bundle-engine.sh` builds and signs the Rust binary for every build, `EngineCommand` starts `Resources/engine/tellfold serve ...`; an engine path in Settings keeps the `uv` path as an option until Python is retired | a Release build of the app, notarisation dry run, Mac tests |

Milestones 2 and 3 can overlap, as can 5 and 6 once 4 has landed.

## Golden tests

`engine-rs/golden/make.py` runs with `uv run python engine-rs/golden/make.py` and writes fixtures from the Python
engine into `engine-rs/golden/fixtures/`: reader output for the example runs, schemas, prompts, runtime command
lines, RNG sequences, the engine's seeded helpers on the example casts, and a whole run on the canned responder
(every model call, every event, the run folder and every `world.db` row), and the report side: the evidence digest,
every `build_report` scenario of `test_report.py` (calls, `report.json`, `report.md`, `report.html`), the citation
audit, interview and ask prompts, rewrites, and the rendered reports of both examples and of the golden run, and study folders (comparisons, Check it,
branches, packed examples). `golden/make_cli_help.py` and `golden/make_cli_runs.py`
record the command line (help screens, errors, rendering, and a session of commands), and `golden/make_api.py` a
session of HTTP requests. The Rust tests read the fixtures, so `cargo test` needs neither Python nor network. Changing Python behaviour means regenerating the fixtures in the same pull request,
which makes every parity change visible in review.

Normalised before comparing: `started_at`, `updated_at`, `created_at`, `pid`, `seconds`/`llm_seconds`, and absolute
paths of temporary folders. Nothing else.

## Parity soak (milestone 9)

Measured on 11 November 2026, on an Apple silicon Mac with macOS 27, against the installed app (0.2.1, Python 3.13
engine) and a release build of `tellfold-cli`.

**Mac tests.** The app's unit suite (230 tests in 31 suites) passes unchanged. `EngineIntegrationTests` gained
`everyRouteTheAppUsesAnswers`, which starts the engine through `EngineController` and calls every `TellfoldAPI`
method the app uses: a run with an event, its reads, seeds, the report HTML, chat, ask, refused writes on a finished
run, a report rebuild, rewrite, a branch and its comparison, Check it, runtimes, the runtime test and settings. Both
engine tests pass against Python (`uv run`) and against the Rust binary. For the latter,
`engine-rs/scripts/uv-shim.sh` stands in for `uv` and starts the binary with the arguments the app passes:

    TEST_RUNNER_TELLFOLD_REAL_ENGINE=1 TEST_RUNNER_TELLFOLD_HOME=/private/tmp/tellfold-mac-home \
    TEST_RUNNER_TELLFOLD_UV_PATH=$PWD/engine-rs/scripts/uv-shim.sh xcodebuild test ...

**Real runs.** The same small run (the corner cafe example, 3 people, 1 round, agents and planner on one runtime)
went through `tellfold serve` and the HTTP API, followed by the runtime test, chat and ask:

| engine | runtime | calls (failed) | citations checked (marked) | run time |
|---|---|---|---|---|
| Rust | Claude Code (haiku / sonnet) | 8 (0) | 47 (0) | 178 s |
| Rust | Cursor Agent (default) | 9 (0), report repaired once | 62 (0) | 308 s |
| Python | Claude Code (haiku / sonnet) | 8 (0) | 57 (0) | 202 s |

Both engines wrote the same set of files. The answers differ run to run, as they do between two Python runs. No
agent CLI processes were left behind after the servers exited.

**Size and start-up.** Median of 7 starts each, `serve --port 0` on the canned model, measured from spawn:

| | Python engine (bundled) | Rust binary |
|---|---|---|
| on disk in the app | 186 MB, 5,070 files | 13 MB, 1 file (6.3 MB gzipped) |
| app bundle total | 195 MB | about 22 MB |
| to the `{"port": N}` line | 411 ms (383 to 579) | 6 ms (5 to 7) |
| resident memory when idle | 93 MB | 9 MB |
| exit after stdin closes | 294 ms | 1 ms |

The binary links only system libraries (`libSystem`, `libiconv`, CoreFoundation). The first launch of a newly
built binary takes several seconds on macOS while the system scans it; later launches take the times above.

## The switch (milestone 10)

`mac/scripts/bundle-engine.sh` now runs `cargo build --release --locked --target aarch64-apple-darwin -p
tellfold-cli` with `MACOSX_DEPLOYMENT_TARGET=15.0`, checks the binary's `minos`, copies it to
`Resources/engine/tellfold` and signs it: ad hoc for local builds, and with the hardened runtime and a timestamp for a
Developer ID identity. The engine needs no entitlements, so `engine.entitlements` (which the bundled Python needed
for its extension modules) is gone. The script then starts `tellfold serve --help` from the bundle as a smoke test.

The build phase runs for every configuration, so Debug builds run the same engine as releases. `EngineCommand`
starts `Resources/engine/tellfold serve --port 0 --runs DIR [--examples DIR] --log-level LEVEL`. An engine path in
Settings > Advanced still runs the Python package with `uv run --project <path> tellfold serve`, and the Mac engine
tests do the same with `TEST_RUNNER_TELLFOLD_ENGINE_PATH`. Building the app now needs a Rust toolchain instead of
`uv`.

Checked on 11 November 2026: a Release build is 28 MB (the engine 13 MB), passes `codesign --verify --deep
--strict`, and its engine is arm64 with `minos 15.0`, linking only system libraries. The same binary signed with the
hardened runtime starts, answers `/api/health` and exits when stdin closes. The Mac unit suite passes, and the
engine tests pass against the bundled binary and, with an engine path, against the Python package. Notarisation
itself needs the release Mac's Developer ID and runs in `mac/scripts/release.sh`.

## Risks

- **PDF text.** No Rust crate extracts text exactly like MuPDF. Measured in milestone 2 on a generated PDF:
  `pdf-extract` returns the same words in the same order, but puts a blank line between lines that MuPDF keeps on
  consecutive lines, so a PDF's paragraphs (and so its chunks) can differ. Chunks from PDFs may differ in whitespace and
  reading order, which changes graph prompts for PDF seeds (not for pasted text or Markdown). Milestone 2 measures
  this on real PDFs; if `pdf-extract` is poor, `pdfium-render` with a bundled pdfium (about 5 MB) is the fallback.
  Existing runs are not affected: they keep their chunks' results in `graph.json`.
- **Claude Code flags.** The CLI is updated often. The argv test pins what we pass, `doctor` checks the flags the
  installed version accepts, and a missing `--json-schema` falls back to JSON mode as the SDK path does today.
- **Template differences.** minijinja and jinja2 differ in a few filters and in whitespace control. Golden HTML
  diffs catch them.
- **SQLite versions.** rusqlite bundles its own SQLite, newer than or equal to macOS's. The schema uses nothing
  version-specific; old databases are covered by fixtures.
