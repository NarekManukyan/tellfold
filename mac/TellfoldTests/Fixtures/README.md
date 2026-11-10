# Test fixtures

Captured from a real engine run in offline mode (`TELLFOLD_FAKE_LLM=1 uv run tellfold ui --runs /private/tmp/tellfold-mac-fixtures --port 8796 --no-open`),
with one small run created over HTTP (6 agents, 3 rounds) and one larger run (120 agents, 4 rounds) for the live
detail and the full event stream:

- `health.json`, `runs.json`, `run_detail.json`, `posts.json`, `actions.json`, `agent_detail.json`, `chat.json`,
  `ask.json`: responses of the matching endpoints for the finished run.
- `run_detail_live.json`: `GET /api/runs/{id}` while the larger run was live (seeding stage).
- `stream_full.sse`: the larger run's `/stream` from connect to `run_done`.
- `stream_tail.sse`: the end of the small run's stream.
- `stop_conflict.json`, `error_validation.json`: FastAPI error bodies (409 detail string, 422 detail list).

`runtimes.json`, `settings.json`, `runtime_test.json` and `unauthorized.json` come from `tellfold serve` (offline mode,
scratch `TELLFOLD_HOME`, bearer token): `GET /api/runtimes`, `GET /api/settings`, `POST /api/runtimes/claude-code/test`
and an `/api` request without a token. Home-folder paths were replaced with `/Users/example`.

`runtimes_handwritten.json` is hand-written from the `RuntimeInfo` shape in MAC_CONTRACT.md (covers `auth: missing`,
an `openai-compatible:` id and an unknown extra field).

`study_detail.json` and `studies.json` come from the offline engine through FastAPI's test client: a 4-agent, 2-round
run, then a second run with `cast_from` set to it (its study), captured once the comparison was written:
`GET /api/studies/{id}` and `GET /api/studies`.
