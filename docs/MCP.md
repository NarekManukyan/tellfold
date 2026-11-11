# Tellfold over MCP

`tellfold mcp` is a [Model Context Protocol](https://modelcontextprotocol.io) server on stdin/stdout. It lets an AI
assistant such as Claude Code, Claude Desktop or Cursor test a text before it ships: start a simulation of how a crowd
reacts to a changelog, a pricing change or a launch post, wait for it, read the cited report, ask the crowd follow-up
questions and compare two drafts.

## How it works

The MCP server is a subcommand of the same `tellfold` binary the Mac app bundles. When a client starts it, it starts
the engine's [HTTP API](API.md) inside the same process, on a free port of `127.0.0.1` with a random token, and its
tools call that API. Runs go to the runs folder in the app's settings, by default
`~/Library/Application Support/Tellfold/runs`, so they show up in the app too. If you moved the folder in the app and
it does not show your runs, pass the same folder with `--runs`. When the client closes the server, runs that are still
going are stopped and marked `stopped`.

The simulated people run on the AI tool chosen in the app's Settings › Agents (or in
`~/Library/Application Support/Tellfold/settings.json`), for example the Claude Code CLI. That tool must be installed
and signed in; `tellfold runtimes` lists what Tellfold finds.

Runs take minutes, so no tool blocks on one. `start_simulation` answers with a run id at once, and `wait_for_run`
waits at most 120 seconds per call, then answers with the progress so far.

## The binary

The `mcp` command ships in Tellfold 0.5.0 and later, inside the app:

```
/Applications/Tellfold.app/Contents/Resources/engine/tellfold
```

To build it from source instead (also on Linux):

```sh
cd engine-rs
cargo build --release -p tellfold-cli
# engine-rs/target/release/tellfold
```

The examples below use the app's path. Replace it with your own build's path if you built one.

## Setup

### Claude Code

```sh
claude mcp add --scope user tellfold -- /Applications/Tellfold.app/Contents/Resources/engine/tellfold mcp
claude mcp list
```

Then add the `tellfold-simulate` skill, which tells Claude when and how to use the tools: see
[integrations/claude-code](../integrations/claude-code/README.md).

### Claude Desktop

Open Settings › Developer › Edit Config, or edit `~/Library/Application Support/Claude/claude_desktop_config.json`,
and add:

```json
{
  "mcpServers": {
    "tellfold": {
      "command": "/Applications/Tellfold.app/Contents/Resources/engine/tellfold",
      "args": ["mcp"]
    }
  }
}
```

Restart Claude Desktop. Claude Desktop starts servers outside any project folder, so give `start_simulation` the text
itself, or absolute file paths.

### Cursor

Add the server to `~/.cursor/mcp.json` (every project) or `.cursor/mcp.json` (one project):

```json
{
  "mcpServers": {
    "tellfold": {
      "command": "/Applications/Tellfold.app/Contents/Resources/engine/tellfold",
      "args": ["mcp"]
    }
  }
}
```

Then turn it on under Cursor Settings › MCP.

### Other clients

Any client that starts stdio servers works the same way: the command is the binary, the argument is `mcp`.

## Options

| option | default | |
|---|---|---|
| `--runs DIR` | settings `runs_dir`, else `~/Library/Application Support/Tellfold/runs` | where runs are saved |
| `--url URL` (`TELLFOLD_URL`) | none | use an engine that is already running, e.g. `tellfold ui` or `tellfold serve`, instead of starting one |
| `--token TOKEN` (`TELLFOLD_SERVE_TOKEN`) | random | the bearer token of the `--url` server, if it has one (`tellfold ui` has none), or of the server started here; at least 16 characters |
| `--examples DIR` | none | a folder of read-only example runs to list next to the runs |
| `--log-level LEVEL` | `warning` | `debug`, `info`, `warning` or `error`; logs go to stderr |

Stdout carries only MCP messages. `TELLFOLD_FAKE_LLM=1` swaps in a canned offline model, which is handy for trying a
client's setup: runs finish in seconds and use no AI tool.

With `--url`, runs belong to that server: they keep going when the MCP client closes, and only that server can inject
events into them. A draft B that `compare_drafts` could not start at once still starts from the MCP server, so the
session must stay open until it has (see below).

## Tools

| tool | what it does |
|---|---|
| `start_simulation` | Starts a run from `text` (with an optional `title`; `.md` is added unless it ends in .md, .markdown or .txt) and/or `files` (paths of .md, .markdown, .txt or .pdf files or folders; folders are read recursively, symlinked folders are skipped, and relative paths start at the server's working folder). Takes `question` (required), `agents` (2-200), `rounds` (1-50), `language`, `events` (`[{round, content}]`), `agent_runtime`, `planner_runtime`, `agent_model` and `planner_model`. Answers `run_id` at once, and `documents`: the names the engine saved the seeds under. |
| `get_run_status` | `state`, `stage`, `round`, `total_rounds`, a rough `progress` (0-1), `finished`, `has_report`, `people`, and `error` when it failed. `pause` is set while the run waits for its cast to be checked in the Tellfold app. `example` or `imported` is true for a read-only run: it can be read and asked about, not changed. |
| `wait_for_run` | Waits until the run finishes or `timeout_seconds` (default 30, at most 120) pass, then answers like `get_run_status`. Call it again while `finished` is false. A run paused for its cast check is waited for like any other, with `pause` in the answer. |
| `get_report` | The report: `format` `markdown` (default, also `md`), `json` (also as structured content) or `both`. |
| `list_runs` | Runs, newest first (`limit`, default 20), with `example` or `imported` set on read-only ones. |
| `list_people` | The simulated people of a run: `id`, `handle`, `name`, `bio`, `stance`. |
| `ask_crowd` | Asks the report agent a question about the outcome of a run; the answer cites posts. |
| `interview_person` | Asks one person (`person`: id, or handle with or without `@`, in any case) a question; they answer in character. |
| `inject_event` | Breaking news at a round that has not started, in a running run. |
| `stop_run` | Stops a running run; what it has so far is kept. |
| `compare_drafts` | Tests two versions of a text on the same crowd: `question`, `draft_a` and `draft_b` (plus the run options of `start_simulation` for draft A), or `baseline_run_id` and `draft_b`. Draft B is shown to draft A's people, with draft A's settings and events, as soon as draft A has them. Answers `run_a`, `run_b` and `study_id`, or only `run_a` while draft A is still making its people: then draft B starts later from this MCP session, which must stay open until `get_study` returns a `study_id`. `pause` is set when draft A's cast waits to be checked in the Tellfold app. If it closed first, call `compare_drafts` again with `baseline_run_id` set to `run_a`. |
| `get_study` | A study by `study_id` or `run_id`: its runs, and the cited comparison (`compare_drafts`) or what held across repeats (`check_run`). `ready` is true once every run has ended and the result covers them all; until then `comparison` is null, even when an older comparison exists. `error` says when no result will be written: fewer than two runs finished with a report, writing it failed, or the engine stopped before it was written. `wait_seconds` (at most 120) waits for either, and for a draft B that has not started yet. |
| `check_run` | Runs a finished run again with the same people and text but other random seeds (`runs` in all, 2-5) and writes what held. Answers a `study_id`. |

A tool that cannot do what was asked (an unknown run, a run with no report yet, a round that has started) answers
with a tool error whose text says why, so the assistant can correct itself.

### A typical session

1. `start_simulation` with `question: "How will existing users react to per-seat pricing?"`, `files: ["CHANGELOG.md"]`,
   `agents: 8`, `rounds: 3`.
2. `wait_for_run` with the `run_id`, again while `finished` is false.
3. `get_report`.
4. `ask_crowd` with `"What would make the team admins accept it?"`.
5. `compare_drafts` with `baseline_run_id` set to the run and `draft_b` set to a rewrite, then `wait_for_run` on
   `run_b` and `get_study` with `wait_seconds: 60` until `ready` is true.

## Troubleshooting

- **The client says the server failed to start.** Run the command yourself
  (`/Applications/Tellfold.app/Contents/Resources/engine/tellfold mcp`) and read stderr. An older binary answers
  `No such command 'mcp'`.
- **Runs fail at once.** The AI tool in Settings › Agents is missing or signed out. `tellfold runtimes` shows what is
  found; the run's `error` says which call failed.
- **Runs take long.** Every person decides in every round. Fewer `agents` and `rounds` are faster and use less of
  your plan.
