# Shoal

Shoal runs a small simulated social network on your own machine to help you guess how people will react to something. You give it one or more documents (a press release, a policy draft, a product announcement) and a question ("How will existing customers react to the new pricing?"). It reads the documents, builds a cast of people and organisations who would care, lets them post, reply, like and follow each other for a few rounds, and then writes a report that answers your question with the simulated posts as evidence.

Every model call goes through the Claude Code CLI you already have installed and logged in. Shoal needs no API key and no cloud service, and it keeps no data anywhere except the run folder it creates.

> **Clean-room note.** Shoal takes its general idea (seed documents in, simulated crowd reaction and forecast out) from public descriptions of swarm-simulation tools such as MiroFish. Its code, prompts, schemas, data layout and UI text were written from scratch against this project's own `ARCHITECTURE.md`, without looking at MiroFish, its forks or the OASIS / camel-ai source.

## Requirements

- **Claude Code CLI**, installed and logged in. Run `claude` once and use `/login` if you have not already. Shoal uses your normal subscription through the CLI.
- **[uv](https://docs.astral.sh/uv/)** for Python and dependencies. Python 3.11 or newer; uv installs it for you if needed.
- macOS or Linux. Windows should work under WSL, but nobody has tested it.

## Install

```sh
git clone <this repo> shoal && cd shoal
uv sync
uv run shoal doctor
```

`doctor` checks that `claude` is on your PATH, that the Python SDK imports, and that a one-word haiku call succeeds. If all four rows say `ok`, you are ready.

To get a global `shoal` command instead of `uv run shoal`:

```sh
uv tool install .
```

## Quick start

The repository includes a fictional seed document: a plant-care app called Mossbook switching from a one-time purchase to a subscription.

```sh
uv run shoal run examples/mossbook_pricing.md \
  -q "How will existing Mossbook buyers react to the new subscription pricing over the first two weeks?" \
  --agents 12 --rounds 4 --open
```

You will see each stage tick off, followed by a table that grows one row per round with the number of active agents, new posts, the mix of actions and the average sentiment. When it finishes, `--open` opens `report.html` in your browser.

`examples/run_mossbook.sh` runs a fuller version that also injects two breaking-news events partway through.

## Commands

### `shoal run SEED... -q QUESTION [options]`

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

### `shoal report RUN_DIR [--model M] [--open]`

Rebuilds `report.json`, `report.html` and `report.md` from what is already in the run folder. Use it after an interrupted run, or to try a different model for the write-up.

### `shoal chat RUN_DIR --agent a03`

Opens an interview with one simulated agent. The agent answers in character, using its persona, its memory notes and what it posted during the run. You can pass an id (`a03`) or a handle (`@maria_dev`). Type `exit` or press Ctrl-D to leave. Run `shoal agents` to see who is there.

### `shoal ask RUN_DIR "question"`

Asks one follow-up question about the outcome ("Which group changed its mind the most, and why?"). The answer draws on the report and the simulation record.

### `shoal agents RUN_DIR`

Prints a table of every persona with followers, posts, activity, influence and starting stance.

### `shoal doctor`

Runs the setup checks described under Install. Exits with status 1 if anything fails.

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

Press Ctrl-C at any point and Shoal stops cleanly and keeps whatever it has written. If the simulation had already started, `shoal report RUN_DIR` builds a report from the rounds that finished.

## Cost and rate limits

Shoal does not charge anything itself; it uses your Claude subscription through the CLI, so heavy runs count against your plan's usage limits. Right after reading the seeds it prints an estimate of the number of calls, assuming none need a retry. For a run with `A` agents, `R` rounds and batch size `B`, that is roughly:

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
- If you hit rate limits, lower `--concurrency` to 2. Shoal retries with backoff, and a round where a batch keeps failing carries on without those agents.
- `--planner-model opus` gives a noticeably better report at a higher usage cost. Keep the agent model on haiku unless you have headroom.
- `calls.jsonl` shows where the time went.

## How it works

1. **Ingest.** Text is pulled out of the seed files (PDFs via PyMuPDF) and cut into overlapping chunks on paragraph boundaries.
2. **Knowledge graph.** The planner model reads the chunks in small groups and lists the actors who would react to the question (people, companies, groups, media) and how they relate. Shoal merges and deduplicates the results in plain Python.
3. **Personas.** The planner turns the most important actors into official accounts and public figures, then fills the rest of the cast with ordinary members of the affected public: different ages, jobs, incentives and starting views, including people who simply do not care.
4. **World.** A small social platform lives in SQLite: posts, replies, reposts, likes, dislikes and follows. Each agent's feed ranks posts from people it follows and replies to its own posts first, then by engagement, author influence and recency.
5. **Rounds.** Each round some agents become active (weighted by their activity level). They are decided in batches: the model sees each agent's private background, its memory notes and its own feed, and picks one to three actions. Invalid actions, such as liking a post that does not exist, are turned into "do nothing" instead of crashing the run. Breaking-news events are shown to everyone in their round. A short pass then scores the round's sentiment.
6. **Report.** Shoal computes the evidence itself (per-round numbers, the most engaged posts, the most followed accounts, how each agent's tone moved from first post to last) and gives that digest to the planner model, which writes a report that answers the question and cites posts and handles.

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

Tests never call the real CLI. Setting `SHOAL_FAKE_LLM=1` makes every command use a built-in offline model that returns valid but meaningless placeholder output, which is handy for trying the CLI without spending usage:

```sh
SHOAL_FAKE_LLM=1 uv run shoal run examples/mossbook_pricing.md -q "test" --agents 6 --rounds 2
```
