# Tellfold engine in Rust

The Rust port of the Python engine in `tellfold/`. The plan, the milestones and the parity rules are in
[docs/rust-port.md](../docs/rust-port.md). The Mac app bundles this engine (`mac/scripts/bundle-engine.sh`).

```bash
cd engine-rs
cargo test --workspace
```

Cargo's target directory is `engine-rs/target`. Do not point `CARGO_TARGET_DIR` at a folder shared with other
checkouts.

## Crates so far

- `tellfold-models`: the shared types of `tellfold/models.py`, pydantic-compatible JSON in and out.
- `tellfold-store`: run folders, `world.db` (`World`, `RunReader`), studies on disk and the call estimate.
- `tellfold-llm`: the `Llm` trait, `FakeLlm` with purpose routing, `extract_json`, `AdapterLlm` (retries, repair,
  `calls.jsonl`), and seed loading and chunking.
- `tellfold-runtimes`: `settings.json`, detection of the installed AI tools, Claude Code through the `claude` CLI,
  the other CLI tools, the HTTP runtimes, and agent/planner routing (`build_llm`).
- `tellfold-engine`: CPython's `random.Random`, graph extraction and merge, persona generation, the round loop and
  `run_pipeline` with `status.json` tracking and stopping. The report stage is passed in.
- `tellfold-report`: the evidence digest, the report call with its citation audit and repair, `report.md` and
  `report.html` (the Python Jinja template, rendered with minijinja), interviews, report Q&A and the rewrite
  suggestion. `ReportWriter` is the pipeline's report stage.
- `tellfold-study`: the cited comparison of a study's runs, the "Check it" agreement across repeats, branching a
  run from a round, and packing a finished run as a built-in example.
- `tellfold-server`: the Tellfold Studio HTTP API (runs, studies, SSE, settings and runtimes), the run manager on
  its own thread, the watcher for runs started elsewhere, uploads, the local-only and token guards, and the
  embedded frontend.
- `tellfold-cli`: the `tellfold` binary with every command, including `serve` (the Mac app's engine) and `ui`, the
  live run view, and typer's help and error screens, plus `mcp`, the MCP server over the HTTP API (only Rust has
  it, so the typer help fixtures leave it out). `TELLFOLD_FAKE_LLM=1` runs it on the canned model.

No test starts a real AI tool. For a manual check against one that is installed and logged in:

```bash
cargo run -p tellfold-cli -- doctor
cargo run -p tellfold-cli -- run ../examples/mossbook_pricing.md -q "Will buyers accept the subscription?" --agents 4 --rounds 1
cargo run -p tellfold-runtimes --example ping -- claude-code
cargo run -p tellfold-runtimes --example ping -- cursor-agent
TELLFOLD_SERVE_TOKEN=0123456789abcdef cargo run -p tellfold-cli -- serve --runs /tmp/tf-runs
cargo run -p tellfold-cli -- ui --runs /tmp/tf-runs
AGENTS=4 ROUNDS=1 cargo run -p tellfold-report --example run -- ../examples/mossbook_pricing.md "Will buyers accept the subscription?"
```

The second example runs a whole simulation with the report and prints every event and the run folder
(`cargo run -p tellfold-engine --example run` does the same without the report).
`cargo run -p tellfold-study --example compare -- RUN_A RUN_B` makes a real comparison and Check it of two finished
run folders.

Every Mac app build compiles this engine for `aarch64-apple-darwin` with `MACOSX_DEPLOYMENT_TARGET=15.0` and puts
it in `Resources/engine/tellfold` (the `release` profile, or `app-debug` without link-time optimisation for Debug).
Setting an engine path in the app's Settings runs the Python package through `uv` instead. To point the app at a
local build without rebuilding it, run `cargo build --release -p tellfold-cli`, set the engine path to the
repository and the uv path to `scripts/uv-shim.sh`. The shim takes the `uv run --project <path> tellfold ...`
command and starts `target/release/tellfold` with the same arguments (see "Parity soak" in the plan).

`schemas/` holds the JSON Schemas pydantic generates for the replies the engine asks models for. They are part of
the prompts, so Rust embeds them as they are.

## Golden fixtures

`golden/fixtures/` holds what the Python engine returns for the same inputs. Regenerate them from the repository
root when the Python behaviour they pin changes, in the same pull request (this also rewrites `schemas/`):

```bash
uv run python engine-rs/golden/make.py
uv run python engine-rs/golden/make_cli_help.py
uv run python engine-rs/golden/make_cli_runs.py
uv run python engine-rs/golden/make_api.py
```
