# Tellfold HTTP API

The Tellfold engine serves a local JSON API. The Mac app, Tellfold Studio (`tellfold ui`) and the MCP server
(`tellfold mcp`, see [MCP.md](MCP.md)) all use it. This page lists every route, what it takes and what it returns,
and the events of the live stream.

The API is for programs on the same machine. It is not a public web service and has no versioning yet: the shapes
below are those of the current engine (`engine-rs/crates/tellfold-server`).

## Starting a server

```sh
# headless, as the Mac app runs it: prints {"port": N} on stdout, then logs to stderr
TELLFOLD_SERVE_TOKEN=$(openssl rand -hex 16) tellfold serve --port 0 --runs ~/tellfold-runs

# Tellfold Studio in the browser, on http://127.0.0.1:8765 (no token)
tellfold ui --runs ~/tellfold-runs
```

`TELLFOLD_FAKE_LLM=1` makes either one use a canned offline model, which is handy for trying the API: runs finish in
seconds and cost nothing.

## Access rules

- The server binds `127.0.0.1` only.
- Every request must carry a local `Host` header (`127.0.0.1`, `localhost` or `::1`, any port), or it gets
  `403 forbidden host`. This stops DNS rebinding.
- A `POST`, `PUT`, `PATCH` or `DELETE` with an `Origin` header from another site (or `Origin: null`) gets
  `403 cross-origin request refused`.
- With a token (`tellfold serve` always has one), every path under `/api` needs `Authorization: Bearer <token>`,
  or the answer is `401 unauthorized` with `WWW-Authenticate: Bearer`.

```sh
curl -H "Authorization: Bearer $TELLFOLD_SERVE_TOKEN" http://127.0.0.1:$PORT/api/health
```

## Errors

Errors are JSON in FastAPI's shape:

```json
{"detail": "run not found"}
```

| status | meaning |
|---|---|
| 400 | a malformed id or field, or a request that does not make sense (`"add at least one seed file ..."`) |
| 401 | missing or wrong token |
| 403 | foreign host, cross-origin write, or a change to a read-only run (a built-in example or an imported run) |
| 404 | no such run, study, agent, runtime, seed or report |
| 409 | not possible in the run's current state (`"round 3 has already started (current round is 4)"`) |
| 413 | an upload over the limits |
| 422 | the JSON body failed validation; `detail` is a list of `{type, loc, msg, input}` items |
| 502 | the AI tool's call failed (`"the model call failed: ..."`) |
| 503 | the server is shutting down |

An unexpected failure is a plain-text `500 Internal Server Error`.

Built-in examples and runs opened from a `.tellfold` bundle are read-only. Every route that would change one (events,
stop, report rebuild, the cast routes) answers 403 with `"this is a built-in example run; it cannot be changed. Use Run
again to make your own copy"` or `"this run was imported from a .tellfold bundle; it cannot be changed. Use Run again
to make your own copy"`. They can still be read, interviewed and asked (with an AI tool set up on this machine), and
branched or checked into new runs of your own.

## Shared shapes

The model shapes are serialised from `engine-rs/crates/tellfold-models`. The JSON Schemas in
[`engine-rs/schemas/`](../engine-rs/schemas) describe the parts the AI writes: [`Report.json`](../engine-rs/schemas/Report.json),
[`CompareReport.json`](../engine-rs/schemas/CompareReport.json) and
[`RewriteSuggestion.json`](../engine-rs/schemas/RewriteSuggestion.json). The engine adds a `citation_audit` object to
each of those after checking the citations.

**RunStatus** (`status.json`, and the `status` stream event)

| field | type | |
|---|---|---|
| `state` | string | `queued`, `running`, `done`, `failed` or `stopped` |
| `stage` | string | `reading`, `graph`, `personas`, `seeding`, `rounds`, `report` or `finished` |
| `round` | int | the last round started |
| `total_rounds` | int | |
| `detail` | string | what it is doing now, e.g. `"round 2 of 6: 3 batches deciding"` |
| `error` | string or null | why it failed |
| `pid` | int or null | the process running it |
| `started_at`, `updated_at` | string | ISO 8601 |
| `failed_batches` | list | `{round, batch, agents, error}` for model calls that failed and were skipped |
| `pause` | string, absent when none | why a running run waits for a client: `"personas_ready"` while its cast is up for review (see [Checking the cast](#checking-the-cast-before-the-run)). Only a running run has one |

**SimConfig** (`config.json`): `question`, `seed_files`, `num_agents`, `rounds`, `batch_size`, `feed_size`,
`memory_size`, `agent_model`, `planner_model`, `concurrency`, `language`, `events` (list of `{round, content}`),
`seed`, `think`, `agent_runtime`, `planner_runtime`, `cast_from`, `study_id`, `variant`, `branched_from`,
`branch_round`, and, only when set, `review_cast` (true), `fair_turns` (true: every opinion camp gets a turn and
strongly held views are not left idle), `audience` (the name of the saved audience the people came from) and
`material` (a **MaterialSummary** of the audience material the people were built from).

**MaterialSummary** describes what was sent: `items` (how many items), `counts` (those items by kind, e.g.
`{"reviews": 14, "survey": 3}`; kinds are `reviews`, `survey`, `emails`, `comments` and `notes`), `dropped` (distinct
items the cap left out before they were looked at), `redacted` (how many of each were hidden in the items sent:
`names`, `emails`, `phones`, `addresses`, `numbers`, `links`) and `files` (a neutral label per file, such as
`"reviews 1"`; file names are never stored). Only when the planner could read fewer items than were sent, `read`
holds the items it read, by kind: the people were built from those, and the report names them. A run whose material
could not be read at all has no `material`. The cleaned items themselves are in the run's `material.json`.

**Persona**: `id` (`"a03"`), `handle`, `name`, `bio`, `background`, `stance`, `traits`, `activity` (0-1),
`influence` (0-1), `entity_id`, and, only when known, `camp` (`strong_support`, `lean_support`, `undecided`,
`indifferent`, `lean_critical` or `strong_critical`) and `source` (`cast`: written by the engine, `group`: from a
group described in words, `edited`: changed before the run, `material`: built from the user's audience material).
A person built from audience material also has `grounding`: `{theme, counts, quotes}`, where `theme` is the
cluster of the material the person stands for, `counts` how many items of each kind that cluster holds, and `quotes`
up to six of those items, already redacted.

**Post**: `id`, `round`, `author_id`, `content`, `parent_id`, `repost_of`, `likes`, `dislikes`, `reposts`,
`replies`. The API adds `author_handle` and `sentiment` (-1 to 1, or null).

**RoundStats**: `round`, `actions` (counts by action type), `new_posts`, `active_agents`, `sentiment` (mean, or
null), `llm_calls`, `llm_seconds`.

**Report**: `title`, `summary`, `prediction`, `confidence` (`low`, `medium` or `high`), `sections`
(`{title, body_markdown}`), `key_voices`, `reasons`, `camps` (`{name, summary, members, sentiment}`), `risks`,
`caveats`, `recommendations` (`{action, title, why, addresses, evidence, expected_effect, confidence}`, where `action`
is `keep`, `change`, `add`, `drop` or `test`), `citation_audit`.

## Runs

### `GET /api/health`

```json
{"ok": true, "fake_llm": false, "version": "0.5.0"}
```

### `GET /api/runs`

Every run, newest first, with the built-in examples (when the server has an examples folder). Each item is a
**RunSummary**:

```json
{
  "id": "20261111-140210-will-existing-users-accept",
  "question": "Will existing users accept the subscription?",
  "state": "done", "stage": "finished", "round": 6, "total_rounds": 6, "agents": 20,
  "created_at": "2026-11-11T14:02:10", "has_report": true, "example": false,
  "study_id": null, "variant": null
}
```

A run waiting for its cast to be checked also has `"pause": "personas_ready"`. A run opened from a `.tellfold` bundle
also has `"imported": true`; the key is left out for every other run.

### `POST /api/runs`

Starts a run. The body is `multipart/form-data` (or `application/x-www-form-urlencoded` for the fields, though a
run needs at least one file). Answers `{"id": "<run id>"}` as soon as the run is queued.

| field | | |
|---|---|---|
| `files` or `files[]` | required, repeatable | seed documents: `.pdf`, `.md`, `.markdown` or `.txt`, at most 20 MB each, 100 files and 100 MB in all (audience material has its own budget) |
| `question` | required | up to 2000 characters |
| `agents` | 2-200, default 20 | |
| `rounds` | 1-50, default 6 | |
| `batch` | 1-20, default 6 | agents decided per model call |
| `concurrency` | 1-16, default 4 | parallel model calls |
| `agent_model` | default `haiku` | |
| `planner_model` | default `sonnet` | |
| `agent_runtime`, `planner_runtime` | default: settings | ids from `GET /api/runtimes` |
| `language` | default `English` | up to 40 characters |
| `think` | `1`/`true`/`yes`/`on` | extended thinking for the planner |
| `seed` | 0 to 2³¹-1, default 7 | random seed |
| `events` | JSON list | `[{"round": 3, "content": "A rival cuts its price."}]`, rounds 1 to `rounds`, 2000 characters each |
| `cast_from` | a run id | reuse that run's topic map and people (it must have them already); the two runs join a study |
| `study_id` | needs `cast_from` | the study to join (default: the source's study, else a new one) |
| `variant` | needs `cast_from` | label of this run in the study (default: the next letter) |
| `review_cast` | `1`/`true`/`yes`/`on` | pause once the people are written, for the cast to be checked (see [below](#checking-the-cast-before-the-run)) |
| `fair_turns` | `1`/`true`/`yes`/`on` | every opinion camp gets a turn to post, and strongly held views are never idle more than two rounds in a row |
| `audience` | JSON, not with `cast_from` | a saved cast, `{"name": "...", "personas": [Persona], "graph": {...}, "material": MaterialSummary}` (1-200 people; `material` optional, held to known kinds and categories and to the caps below): the run uses these people and topic map and skips the graph and persona stages |
| `material` or `material[]` | optional, repeatable, not with `cast_from` or `audience` | audience material to build the people from: `.txt`, `.md`, `.markdown`, `.csv`, `.tsv` or `.eml`, at most 10 MB each, 20 files and 200 MB in all (see [Audience material](#audience-material)) |
| `material_spec` | JSON list, needs `material` | one `{"kind": ..., "column": ...}` per material file, in the form's order (under either key); both keys optional |
| `material_hide` | text, needs `material` | other names or words to hide, one per line or separated by commas, at most 200 |

A whole form may hold 300 MB (the seeds' 100 MB and the material's 200 MB) and 120 files; over that the answer is
413, `"the upload is larger than 300 MB: the seed files may add up to 100 MB and the audience material to 200 MB"`.

With `cast_from`, `agents` and `fair_turns` are taken from the source run, so a new version differs only in the
text, and the source's `material` summary is kept. `cast_from` answers 409 when the source's cast was never checked:
a `review_cast` run that is still waiting, or was stopped there. With `audience`, `agents` is the number of people in
it.

### Audience material

Material is what real people said: customer reviews, survey answers, support emails, social comments, meeting notes.
It is separate from the seed documents. With it, the persona stage builds the crowd from that material instead of
from the seeds alone:

1. **Read and redact, on this computer.** Each file is split into items (paragraphs, bullets, or lines of a review
   or comment list; one message per `.eml`, quoted replies and signatures cut; one cell per row of a spreadsheet
   column). Common first names, names in greetings, sign-offs and introductions, @handles, email addresses, phone
   numbers, street addresses, order or account numbers and links are replaced by `[name]`, `[email]`, `[phone]`,
   `[address]`, `[number]` and `[link]` before anything is sent anywhere. This step is local and deterministic, and
   it is not complete: a name the rules do not know, mid-sentence or in lower case, stays as written. Check the
   preview, and add such names to `material_hide`. Only the redacted items are saved, in the run's `material.json`;
   the uploaded files are not kept, and the run names them by label only.
2. **Sort.** The planner reads the items in batches of 40 and gives each a camp and a short theme. The engine then
   groups the items into clusters locally, by camp and by shared theme words.
3. **Write people.** The camps of the crowd follow how the items spread across camps. Each person stands for one
   cluster and is written from up to three of its items; quotes are not repeated until a cluster runs out.

`kind` is one of `reviews`, `survey`, `emails`, `comments` or `notes`. It can be left out for `.eml` (emails) and
`.csv`/`.tsv` (survey answers); a `.txt` or `.md` file without one is refused with 400. For a spreadsheet, `column`
picks the column to read, by header or by 1-based number; without it the column with the most text is read. No other
column is ever read. A spreadsheet with a quote that never closes is refused with 400 naming its line, since the rest
of the file, other columns included, would otherwise be read as one answer.

Limits: 300 items at most, 600 characters per item, and items under 12 characters are skipped. Repeats are dropped,
then items are redacted in an even sample across the files, topped up from the rest while redaction turns some into
repeats (items that differ only in a hidden detail, such as two names on one complaint, count once), until 300
distinct ones are kept or 1200 were redacted. `dropped` counts the ones never looked at, and a warning says so.
Fewer than 30 items adds a warning (a few voices will shape the whole crowd), and so do fewer items than people (some
people share quotes).

```sh
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:$PORT/api/runs \
  -F files=@pricing.md -F question="How will customers take the new price?" \
  -F material=@reviews.txt -F material=@survey.csv \
  -F 'material_spec=[{"kind": "reviews"}, {"column": "What would make you cancel?"}]' \
  -F 'material_hide=Mossbook'
```

### `POST /api/material/preview`

Exactly what a run with this material would send to the model, without starting one. Takes the same `material`,
`material_spec` and `material_hide` fields as `POST /api/runs`, and optionally `agents` (to warn when the material is
thin for that many people). 400 without a material file, or for a file that cannot be read; the message says why.

```json
{
  "summary": {"items": 8, "counts": {"reviews": 5, "survey": 3}, "dropped": 0,
              "redacted": {"names": 2, "emails": 1, "phones": 1, "addresses": 0, "numbers": 1, "links": 0},
              "files": ["reviews 1", "survey 1"]},
  "files": [{"name": "reviews.txt", "label": "reviews 1", "kind": "reviews", "items": 5},
            {"name": "survey.csv", "label": "survey 1", "kind": "survey", "items": 3,
             "columns": ["Name", "Email", "What would make you cancel?"], "column": "What would make you cancel?"}],
  "items": [{"id": "m002", "kind": "reviews", "file": "reviews 1",
             "text": "The price doubled overnight and nobody warned us. My order #[number] was charged twice."}],
  "warnings": ["Only 5 reviews and 3 survey answers: a few voices will shape the whole crowd. ..."],
  "thin": true,
  "estimated_calls": 1
}
```

`files` lists the files in the form's order, with the name each was sent under and its `label`; items name their file
by label. `estimated_calls` is the number of extra planner calls the material costs (one per 40 items). The usage
estimate of a run counts them too.

```sh
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:$PORT/api/runs \
  -F files=@CHANGELOG.md -F question="How will users react to 3.0?" -F agents=8 -F rounds=3
```

### `GET /api/runs/{run_id}`

The whole run (**RunDetail**):

| field | |
|---|---|
| `id` | |
| `config` | SimConfig |
| `status` | RunStatus, or null before the run has one |
| `live` | whether this server is running it now |
| `personas` | list of Persona |
| `graph` | `{entities: [{id, name, kind, summary, source_chunk_ids}], relations: [{source, target, label, evidence}]}`, or null |
| `stats` | list of RoundStats |
| `events` | list of `{round, content}` |
| `follows` | list of `[follower_id, followee_id]` |
| `report` | Report, or null until it is written |
| `calls` | `{count, failed, seconds, agent: {count, failed, seconds}, planner: {...}}` |
| `report_rebuild` | `{running, error}` for `POST .../report` |
| `example` | a read-only built-in example |
| `imported` | `true` for a read-only run opened from a `.tellfold` bundle; left out otherwise |

### `GET /api/runs/{run_id}/posts?since_id=N`

Posts with an id above `since_id` (default 0), oldest first. Each is a Post with `author_handle` and `sentiment`.

### `GET /api/runs/{run_id}/actions?round=N`

What each agent did, optionally for one round: `{round, agent_id, type, target_post_id, target_agent_id, content,
reasoning}`.

### `GET /api/runs/{run_id}/agents/{agent_id}`

One agent, by id (`a03`) or handle (`@maria`):

```json
{"persona": {...}, "memory": ["..."], "posts": [...], "followers": ["a01"], "following": ["a07"],
 "sentiment_by_round": [[1, 0.2], [2, -0.1]]}
```

### `POST /api/runs/{run_id}/events`

Injects breaking news into a run this server is running. Body `{"round": 4, "content": "..."}` (1-2000
characters); the round must be one that has not started. Answers `{"ok": true}`, or 409 when the run is not live
here or the round has started.

### `POST /api/runs/{run_id}/stop`

Asks a live run to stop after the current step. Answers `{"ok": true}`; the run ends with state `stopped`.

### Checking the cast before the run

A run started with `review_cast` writes its people, sets `status.pause` to `"personas_ready"`, sends the
`personas_ready` stream event and waits. Nothing is simulated until the cast is confirmed, so changing it costs only
the calls that write added groups. Stopping the run (`POST .../stop`) while it waits ends it before any round.

Edits stay with the waiting run; it writes `personas.json` once, when it goes on. Every answer with a cast is
`{"waiting": bool, "personas": [Persona]}`, tidied by the engine: text trimmed, handles made unique, unknown camps and
sources dropped, and ids renumbered `a01`..`aNN` in order (so an id can name another person after any change).

- `GET /api/runs/{run_id}/personas`: the cast under review while the run waits (`waiting: true`), else the run's
  people as written (`waiting: false`).
- `PUT /api/runs/{run_id}/personas`: replaces the cast. Body `{"personas": [Persona]}`, at most 200; an empty cast is
  kept while the run waits, so a group can still be added. 422 for a body that is not that shape or holds more than
  200 people; 409 when the run is not waiting, or when a group being written holds the places it would take.
- `POST /api/runs/{run_id}/personas/group`: the planner writes people for a group described in words and adds them.
  Body `{"description": "...", "count": 3}`: `description` 1-500 characters (not only whitespace), `count` 1-20,
  default 3; 422 otherwise. 400 when the cast has no room for `count` more (the model is not called); the group's
  places are held while the model writes it. One model call; 502 when it fails.
- `POST /api/runs/{run_id}/personas/confirm`: the run goes on with the cast. Answers `{"ok": true}`; 400 when the
  cast is empty, 409 while a group is still being written or when the run is not waiting.

### `POST /api/runs/{run_id}/chat`

Interviews one agent in character. Body:

```json
{"agent_id": "a03", "message": "Would you pay $6 a month?", "history": [["earlier question", "their answer"]]}
```

`message` is 1-4000 characters, `history` at most 50 pairs (the last 20 are used). Answers `{"reply": "..."}`.

### `POST /api/runs/{run_id}/ask`

Asks the report agent a question about the outcome. Body `{"question": "..."}` (1-4000 characters). Answers
`{"reply": "..."}` with citations of posts.

### `POST /api/runs/{run_id}/report`

Rebuilds the report from what the run has (also for an interrupted run). Answers `{"ok": true}` at once; the
`report_rebuilt` stream event and `report_rebuild` in the run detail say when it is done.

### `GET /api/runs/{run_id}/report.md` and `GET /api/runs/{run_id}/report.html`

The report as Markdown (`text/markdown`) or as the standalone HTML page. 404 until the run has one.

### `GET /api/runs/{run_id}/bundle`

The finished run as one `.tellfold` file to send to someone else: a zip of a cleaned copy of the run folder (no
prompt text, no process id, relative seed paths) plus `bundle.json` (`{format, run_id, question, exported_at,
engine_version}`). Sent as `application/vnd.tellfold.run+zip` with `Content-Disposition: attachment;
filename="<run_id>.tellfold"`. 400 when the run is not finished with a report, its calls log holds prompt text or is
damaged (`"the run's calls log is damaged"`), the run unpacks to more than 2048 MB (`"... a bundle may unpack to at
most 2048 MB"`), or the bundle would be over 512 MB; 409 while the run is live or its report is being rebuilt. While a bundle is being written
the run's report cannot be rebuilt (409).

A run built from audience material keeps it out of the bundle (`material.json` stays behind, and its files were only
ever named by label), but its people carry the redacted quotes they were written from (`grounding.quotes` in
`personas.json`), so whoever opens the bundle can read them.

### `POST /api/runs/import`

Opens a `.tellfold` file, sent as the raw request body (any content type, at most 512 MB), as a new read-only run.
Answers `{"id": "<new run id>"}`: the exported run's id, or that id with `-2`, `-3`, ... when it is taken. 400 for an
empty body or a file that is not a valid bundle (unsafe or unexpected names, links, wrong sizes, a damaged or unsafe
database, an unfinished run, a newer format); 413 over 512 MB. Nothing is left in the runs folder when a bundle is
refused. The new run has `imported: true` and is read-only (see [Errors](#errors)).

### `GET /api/runs/{run_id}/seeds` and `GET /api/runs/{run_id}/seeds/{name}`

The run's seed documents: a list of `{name, size}`, and one file as it was uploaded.

### `POST /api/runs/{run_id}/rewrite` and `GET /api/runs/{run_id}/rewrite`

`POST` asks for a better version of the run's text, based on its report, and answers a **RewriteSuggestion**:
`{revised_text, changes: [{what, why, cites}], kept_on_purpose, source_name, created_at, citation_audit}`. `GET`
answers the last one saved, or 404. Test the new text with `POST /api/runs` and `cast_from`.

### `POST /api/runs/{run_id}/branch`

"What if...?": copies the run up to a round and continues it as a new run with other events. Body:

```json
{"round": 3, "events": [{"round": 4, "content": "The CEO apologises."}]}
```

`round` keeps rounds 0 to `round` as they happened; `events` (at most 50) happen after it. Answers
`{"id": "<new run id>", "study_id": "<study>"}`.

### `POST /api/runs/{run_id}/check`

"Check it": runs a finished run again with the same people and text but other random seeds, one after another, then
writes what held. Body `{"runs": 3, "agent_model": null}` (`runs` 2-5, this one included). Answers
`{"study_id": "..."}`; follow it with `GET /api/studies/{study_id}`.

### `GET /api/runs/{run_id}/stream`

The live event stream of a run (see [below](#the-event-stream)).

## Studies

A study groups runs of one crowd: versions of a text (`kind: "compare"`, made with `cast_from` or `branch`) or
repeats of one run (`kind: "repeat"`, made with `check`).

### `GET /api/studies`

Newest first:

```json
[{"id": "s-20261111-140501", "kind": "compare", "created_at": "...", "question": "...",
  "runs": [{"id": "...", "variant": "Draft A", "letter": "A"}, {"id": "...", "variant": "Plus plan", "letter": "B"}],
  "has_compare": true, "comparing": false, "planned": 0}]
```

### `GET /api/studies/{study_id}`

| field | |
|---|---|
| `id`, `kind`, `created_at`, `planned` | |
| `runs` | `{id, variant, letter, question, state, has_report, example, digest}` per run, plus `imported: true` for a run opened from a bundle; `digest` is the per-run summary the comparison is built from |
| `compare` | for `compare` studies: `{study_id, created_at, runs: [digest], report: {headline, better_variant, why, per_variant: [{variant, summary, objections}], objections_only_in: [{variant, objections}], caveats, citation_audit}}`, or null |
| `compare_state` | `{running, error}` |
| `repeating` | a "Check it" series is still running |
| `agreement` | for `repeat` studies: `{study_id, created_at, runs, final_low, final_high, spread, bands, camps, held, split, verdict, summary}` where `verdict` is `consistent`, `mixed` or `unstable`; or null |

The comparison is written by itself once every run of the study has finished with a report.

### `POST /api/studies/{study_id}/compare`

Writes (or rewrites) the comparison in the background. Answers `{"ok": true}`; 409 while it is being written or
while fewer than two runs have finished with a report.

## Runtimes and settings

### `GET /api/runtimes?check_auth=0|1`

The AI tools found on this machine:

```json
[{"id": "claude-code", "kind": "cli", "name": "Claude Code", "installed": true, "version": "2.1.0",
  "path": "/usr/local/bin/claude", "auth": "ok", "auth_hint": null, "models": ["haiku", "sonnet", "opus"],
  "default_models": {"agent": "haiku", "planner": "sonnet"}, "tested": true, "notes": null}]
```

`auth` is `ok`, `missing` or `unknown`; `check_auth=1` checks logins (slower; may call the tools).

### `POST /api/runtimes/{runtime_id}/test`

Makes one tiny call. Optional body `{"model": "haiku"}`. Answers `{ok, seconds, reply, error, model}`.

### `GET /api/settings` and `PUT /api/settings`

The settings in `~/Library/Application Support/Tellfold/settings.json` (or `$TELLFOLD_HOME/settings.json`):

```json
{"runs_dir": null, "agent_runtime": "claude-code", "agent_model": "haiku", "planner_runtime": "claude-code",
 "planner_model": "sonnet", "ollama_base_url": "http://127.0.0.1:11434",
 "openai_endpoints": [{"id": "local", "name": "LM Studio", "base_url": "http://127.0.0.1:1234/v1",
                       "models": ["qwen3"], "key_env": null}],
 "concurrency": 4, "think": false}
```

`PUT` takes the whole object, validates it (422 with the field at fault) and answers the saved settings. No secrets
are stored: API keys come only from environment variables.

## The event stream

`GET /api/runs/{run_id}/stream` is a [server-sent events](https://html.spec.whatwg.org/multipage/server-sent-events.html)
stream (`text/event-stream`). It opens with the comment `: connected`, then sends the run's current `status`, then
every change. After 15 seconds of silence it sends a `: ping` comment. A run this server is not executing (started by
the command line or another server) is followed by watching its folder, so the same events arrive, a little later.

```
event: round_done
data: {"round":2,"actions":{"post":3,"reply":5,"like":9},"new_posts":8,"active_agents":7,"sentiment":0.12,"llm_calls":3,"llm_seconds":14.2,"total_rounds":6}
```

| event | data |
|---|---|
| `status` | RunStatus, whenever it changes |
| `stage` | `{stage, detail, ...}`; seeding steps add `step` (`follow_graph`, `opening_posts`, ...), reused people add `reused: true` |
| `round_start` | `{round, active, total_rounds, active_agents, batches, events}` |
| `posts_added` | `{round, post_ids}`; fetch them with `GET .../posts?since_id=` |
| `round_done` | RoundStats plus `total_rounds` |
| `event_injected` | `{round, content}` when an injected or planned event takes effect |
| `batch_failed` | `{round, batch, agents, error}`; the run goes on without that batch |
| `personas_ready` | `{personas}`: the cast is written and the run waits for it to be checked; a client that connects during the pause gets it too |
| `run_done` | `{}` |
| `run_failed` | `{error}` |
| `run_stopped` | `{}` |
| `report_rebuilt` | `{ok}`, or `{ok: false, error}`, after `POST .../report` |

## The frontend

Any other `GET` outside `/api` answers Tellfold Studio's single-page app, and `/static/...` its files.
