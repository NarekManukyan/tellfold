# Developing Tellfold

This page is for people who want to build Tellfold from source, use the command-line engine, or cut a release. If you just want to use the app, see the [README](../README.md).

The engine is a Python package (`tellfold/`) with a Rust port in `engine-rs/` that behaves the same. The Mac app (`mac/`) bundles the Rust build. The logo and brand files live in `brand/` (see `brand/README.md`).

> **Clean-room note.** Tellfold takes its general idea (seed documents in, simulated crowd reaction and forecast out) from public descriptions of swarm-simulation tools such as MiroFish. Its code, prompts, schemas, data layout and UI text were written from scratch against this project's own `ARCHITECTURE.md`, without looking at MiroFish, its forks or the OASIS / camel-ai source.

## Requirements

- **Claude Code CLI**, installed and logged in. Run `claude` once and use `/login` if you have not already. Tellfold uses your normal subscription through the CLI.
- **[uv](https://docs.astral.sh/uv/)** for Python and dependencies. Python 3.11 or newer; uv installs it for you if needed.
- **[Rust](https://rustup.rs)** (stable) to build the Mac app, which bundles the Rust engine, or to work on `engine-rs/`.
- macOS or Linux. Windows should work under WSL, but nobody has tested it.

## Install

```sh
git clone https://github.com/NarekManukyan/tellfold.git && cd tellfold
uv sync
uv run tellfold doctor
```

`doctor` checks that `claude` is on your PATH, that the Python SDK imports, and that a one-word haiku call succeeds. If all four rows say `ok`, you are ready.

To get a global `tellfold` command instead of `uv run tellfold`:

```sh
uv tool install .
```

## Quick start

The repository includes a fictional seed document: a plant-care app called Mossbook switching from a one-time purchase to a subscription.

```sh
uv run tellfold run examples/mossbook_pricing.md \
  -q "How will existing Mossbook buyers react to the new subscription pricing over the first two weeks?" \
  --agents 12 --rounds 4 --open
```

You will see each stage tick off, followed by a table that grows one row per round with the number of active agents, new posts, the mix of actions and the average sentiment. When it finishes, `--open` opens `report.html` in your browser.

`examples/run_mossbook.sh` runs a fuller version that also injects two breaking-news events partway through.

There are four more fictional seeds that have nothing to do with software: `examples/corner_cafe_prices.md` (a café raising its prices), `examples/school_phone_policy.md` (a school banning phones during the day), `examples/high_street_car_free.md` (a town closing its main street to cars) and `examples/team_office_return.md` (a company bringing staff back to the office). The Mac app bundles all of them for "Start from an example" on the New simulation sheet.

## Commands

The command is `tellfold` (`run`, `compare`, `check`, `branch`, `rewrite`, `report`, `ui`, `serve`, `mcp`, `chat`,
`ask`, `agents`, `doctor`, `runtimes`). The old `shoal` command still works as a deprecated alias for the same app.

### `tellfold run SEED... -q QUESTION [options]`

Runs the whole pipeline and creates `runs/<YYYYMMDD-HHMMSS>-<slug>/`.

| option | default | meaning |
|---|---|---|
| `-q, --question` | required | What you want predicted. The report answers this directly. |
| `--agents` | 20 | Number of simulated people and organisations. |
| `--rounds` | 6 | Simulation rounds. Each round is one beat of activity on the platform. |
| `--batch` | 6 | Agents decided per model call. Bigger batches mean fewer calls but less attention per agent. |
| `--agent-model` | haiku | Model for the per-round decisions. This is where most calls go. |
| `--planner-model` | sonnet | Model for reading the documents, writing personas and writing the report. |
| `--concurrency` | 4 | How many Claude CLI processes run at once. |
| `--event "ROUND:TEXT"` | none | Breaking news shown to everyone at the start of that round. Can be repeated. Rounds count from 1. |
| `--language` | English | Language the agents post in and the report is written in. |
| `--seed` | 7 | Random seed for who is active each round and feed tie-breaks. |
| `--think` | off | Turn on extended thinking for the graph, persona and report calls. More thorough, but each of those calls gets several times slower. By default only the final report call thinks. |
| `--out` | runs | Folder that holds run directories. |
| `--open` | off | Open `report.html` when the run finishes. |

`SEED` can be `.pdf`, `.md`, `.markdown` or `.txt` files, or a folder containing them.

### `tellfold report RUN_DIR [--model M] [--open]`

Rebuilds `report.json`, `report.html` and `report.md` from what is already in the run folder. Use it after an interrupted run, or to try a different model for the write-up.

### `tellfold chat RUN_DIR --agent a03`

Opens an interview with one simulated agent. The agent answers in character, using its persona, its memory notes and what it posted during the run. You can pass an id (`a03`) or a handle (`@maria_dev`). Type `exit` or press Ctrl-D to leave. Run `tellfold agents` to see who is there.

### `tellfold ask RUN_DIR "question"`

Asks one follow-up question about the outcome ("Which group changed its mind the most, and why?"). The answer draws on the report and the simulation record.

### `tellfold agents RUN_DIR`

Prints a table of every persona with followers, posts, activity, influence and starting stance.

### `tellfold doctor`

Runs the setup checks described under Install. Exits with status 1 if anything fails.

### `tellfold mcp`

An MCP server on stdin/stdout for Claude Code, Claude Desktop and Cursor. Only the Rust engine has it
(`cargo run -p tellfold-cli -- mcp`). See [MCP.md](MCP.md) for the setup and the tools, and [API.md](API.md) for the
HTTP API behind it, which `serve` and `ui` also expose.

## What a run leaves behind

```
runs/20260303-091500-how-will-existing-mossbook-buyers/
  config.json     the exact settings used
  graph.json      people, organisations and relations extracted from the seeds
  personas.json   the simulated cast
  world.db        SQLite: posts, actions, follows, memories, round stats, events
  calls.jsonl     one line per model call: purpose, model, duration, sizes, errors (never the prompt text)
  report.json     the structured report
  report.html     a self-contained page: report, sentiment chart, top threads, persona directory
  report.md       the same report as Markdown
```

Press Ctrl-C at any point and Tellfold stops cleanly and keeps whatever it has written. If the simulation had already started, `tellfold report RUN_DIR` builds a report from the rounds that finished.

## Cost and rate limits

Tellfold does not charge anything itself; it uses your Claude subscription through the CLI, so heavy runs count against your plan's usage limits. Right after reading the seeds it prints an estimate of the number of calls, assuming none need a retry. For a run with `A` agents, `R` rounds and batch size `B`, that is roughly:

```
graph calls  ≈ chunks / 4                     (planner model)
persona calls ≈ A / 10                         (planner model)
round calls  ≈ R × (A / B + 1)                 (agent model, plus one sentiment pass per round)
report       = 1                               (planner model)
```

The default 20 agents × 6 rounds with batch 6 comes to about 30 haiku calls for the rounds, plus a handful of sonnet calls for the graph, personas and report.

Tips:

- Start small (`--agents 8 --rounds 3`) to check the question and seeds give sensible results, then scale up.
- Extended thinking is off for almost every call. With it on, haiku spends 10-20k hidden tokens per call and a graph-extraction call goes from about 10s to about 2 minutes. Only the final report keeps it, unless you pass `--think`.
- If you hit rate limits, lower `--concurrency` to 2. Tellfold retries with backoff, and a round where a batch keeps failing carries on without those agents.
- `--planner-model opus` gives a noticeably better report at a higher usage cost. Keep the agent model on haiku unless you have headroom.
- `calls.jsonl` shows where the time went.

## How it works

1. **Ingest.** Text is pulled out of the seed files and cut into overlapping chunks on paragraph boundaries. The Python package reads PDFs with PyMuPDF and the Rust engine (the one in the app) with `pdf-extract`, so a PDF's chunks can differ in whitespace and reading order between the two (see "Risks" in `docs/rust-port.md`).
2. **Knowledge graph.** The planner model reads the chunks in small groups and lists the actors who would react to the question (people, companies, groups, media) and how they relate. Tellfold merges and deduplicates the results itself, without a model call.
3. **Personas.** The planner turns the most important actors into official accounts and public figures, then fills the rest of the cast with ordinary members of the affected public: different ages, jobs, incentives and starting views, including people who simply do not care.
   In the Mac app, "Review the cast before the run" pauses here: the run's status gets `pause: "personas_ready"`, and `GET`/`PUT /api/runs/{id}/personas` read and replace the cast, `POST /api/runs/{id}/personas/group` writes a group described in words (`{description, count}`), and `POST /api/runs/{id}/personas/confirm` lets the run go on. Edits stay with the waiting run until the confirm, when the run writes `personas.json`. The cast may be empty while the run waits (a group can still be added); the confirm needs at least one person. A cast holds at most 200 people: a group that would pass that is refused before the model is called, and its places are held while the model writes it. A run whose cast was never checked (still waiting, or stopped at the pause) cannot lend its people to another (`cast_from` answers 409), and a new version of a run (`cast_from`) keeps that run's `fair_turns`. A cast saved as an audience can start a later run (`audience` on `POST /api/runs`), which skips the graph and persona stages.
4. **World.** A small social platform lives in SQLite: posts, replies, reposts, likes, dislikes and follows. Each agent's feed ranks posts from people it follows and replies to its own posts first, then by engagement, author influence and recency.
5. **Rounds.** Each round some agents become active (weighted by their activity level). Runs started from the Mac app also use fair turns (`fair_turns`): a camp nobody in it has posted for yet always has someone in the round, and people who hold a view strongly are never idle for more than two rounds in a row. They are decided in batches: the model sees each agent's private background, its memory notes and its own feed, and picks one to three actions. Invalid actions, such as liking a post that does not exist, are turned into "do nothing" instead of crashing the run. Breaking-news events are shown to everyone in their round. A short pass then scores the round's sentiment.
6. **Report.** Tellfold computes the evidence itself (per-round numbers, the most engaged posts, the most followed accounts, how each agent's tone moved from first post to last) and gives that digest to the planner model, which writes a report that answers the question and cites posts and handles.

## Limitations

- These are language-model agents, not people. They are often more articulate, more reasonable and more on-topic than a real crowd. Treat the output as a structured way to think through reactions, not as a poll.
- Twenty agents over six rounds is a small town meeting, not the internet. Network effects, virality and bots are only roughly approximated.
- Results vary between runs. Run the same question two or three times with different `--seed` values before trusting a conclusion.
- The agents only know what is in your seed documents plus the model's general knowledge. If an important fact or a vocal group is missing from the seeds, it will be missing from the simulation.
- Sentiment is the model's judgement of the round's posts toward your question's subject, on a −1 to 1 scale. It is not a calibrated measurement.
- Scanned PDFs without a text layer are not OCR'd.

## Development

```sh
uv run pytest -q
```

Tests never call the real CLI. Setting `TELLFOLD_FAKE_LLM=1` makes every command use a built-in offline model that returns valid but meaningless placeholder output, which is handy for trying the CLI without spending usage. The other environment variables are `TELLFOLD_HOME`, `TELLFOLD_FAKE_DELAY`, `TELLFOLD_SERVE_TOKEN`, `TELLFOLD_ALLOW_UNSAFE_RUNTIMES`, `TELLFOLD_KEY_<ID>` and `TELLFOLD_REAL_RUNTIMES`; the old `SHOAL_*` names are still read as fallbacks. Settings and runs live in `~/Library/Application Support/Tellfold`; an old `.../Shoal` folder is moved there automatically on first launch:

```sh
TELLFOLD_FAKE_LLM=1 uv run tellfold run examples/mossbook_pricing.md -q "test" --agents 6 --rounds 2
```

### Mac app

The Mac app lives in `mac/` (XcodeGen). Every build is self-contained: `mac/scripts/bundle-engine.sh` builds the Rust engine (`engine-rs/`) for macOS 15 on Apple silicon and puts the one binary in `Tellfold.app/Contents/Resources/engine/tellfold`, so the app needs neither `uv` nor Python on the user's Mac. Building the app needs a Rust toolchain ([rustup](https://rustup.rs)). The app still needs an AI tool such as Claude Code installed and signed in, because every model call goes through the user's own CLI.

To run the app on the Python package instead, set Settings > Advanced > Engine path to this repository. The app then starts it with `uv run --project <path> tellfold serve`.

```sh
cd mac && xcodegen generate
xcodebuild build -scheme Tellfold -configuration Release -derivedDataPath build/dd
```

The first build compiles the engine (a few minutes). Later builds reuse `engine-rs/target` and recompile only the crates that changed. Release builds then redo the link-time optimisation, which takes a minute or two; Debug builds use the `app-debug` profile without it. The app builds and runs on Apple silicon only. The bundled engine is about 13 MB. A Developer ID signing identity signs it with the hardened runtime, ready for notarization.

### Releasing

`mac/scripts/release.sh <version>` builds a Developer ID signed and notarized `Tellfold.dmg`, signs it for Sparkle and adds it to `site/appcast.xml`, which the app reads for updates. Add `--publish` to also create the GitHub release and push the appcast; the Pages workflow (`.github/workflows/pages.yml`) publishes `site/` to GitHub Pages on every push to `main` that touches it. Write the release notes in `docs/releases/<version>.md` first, and commit the appcast change afterwards.

One-time setup on the release Mac:

1. Developer ID signing (needs a paid Apple Developer Program membership). Either a **Developer ID Application** certificate in the keychain (Xcode > Settings > Accounts > Manage Certificates > +), or the team's Account Holder signed in to Xcode and `TELLFOLD_TEAM_ID=<team id>` set, so Xcode signs the export with Apple's cloud-managed Developer ID certificate.
2. Notary credentials. Either an App Store Connect API key saved as an [`asc`](https://asccli.sh) profile, then run with `TELLFOLD_ASC_PROFILE=<profile>`; or an app-specific password from account.apple.com: `xcrun notarytool store-credentials tellfold-notary --apple-id <apple id> --team-id <team id>`.
3. The Sparkle update key is in the login keychain under the account `tellfold`. Back it up with `mac/build/sparkle-2.10.1/bin/generate_keys --account tellfold -x <file>` and keep the file somewhere safe: without it no installed copy can be updated.
4. `gh auth login` for GitHub releases.
