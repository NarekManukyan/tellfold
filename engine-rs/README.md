# Tellfold engine in Rust

The Tellfold engine: the `tellfold` command-line tool and the binary the Mac app bundles
(`mac/scripts/bundle-engine.sh`). It began as a port of a Python engine, which was retired after 0.7.0; the port's
plan and parity rules are in [docs/rust-port.md](../docs/rust-port.md).

```bash
cd engine-rs
cargo test --workspace
```

Cargo's target directory is `engine-rs/target`. Do not point `CARGO_TARGET_DIR` at a folder shared with other
checkouts.

## Crates

- `tellfold-models`: the shared types, with JSON in and out exactly as the Python engine wrote it (pydantic's
  layout), so run folders from any version still read.
- `tellfold-store`: run folders, `world.db` (`World`, `RunReader`), studies on disk and the call estimate.
- `tellfold-llm`: the `Llm` trait, `FakeLlm` with purpose routing, `extract_json`, `AdapterLlm` (retries, repair,
  `calls.jsonl`), and seed loading and chunking.
- `tellfold-runtimes`: `settings.json`, detection of the installed AI tools, Claude Code through the `claude` CLI,
  the other CLI tools, the HTTP runtimes, and agent/planner routing (`build_llm`).
- `tellfold-engine`: CPython's `random.Random`, graph extraction and merge, persona generation, the round loop and
  `run_pipeline` with `status.json` tracking and stopping. The report stage is passed in.
- `tellfold-report`: the evidence digest, the report call with its citation audit and repair, `report.md` and
  `report.html` (a Jinja template, rendered with minijinja), interviews, report Q&A and the rewrite
  suggestion. `ReportWriter` is the pipeline's report stage.
- `tellfold-study`: the cited comparison of a study's runs, the "Check it" agreement across repeats, branching a
  run from a round, and packing a finished run as a built-in example.
- `tellfold-server`: the Tellfold Studio HTTP API (runs, studies, SSE, settings and runtimes), the run manager on
  its own thread, the watcher for runs started elsewhere, uploads, the local-only and token guards, and the
  embedded frontend (`web/`).
- `tellfold-cli`: the `tellfold` binary with every command, including `serve` (the Mac app's engine) and `ui`, the
  live run view, and typer's help and error screens, plus `mcp`, the MCP server over the HTTP API (only Rust has
  it, so the typer help fixtures leave it out). `TELLFOLD_FAKE_LLM=1` runs it on the canned model.
- `tellfold-bench`: the accuracy benchmark harness (`bench/`, see [bench/README.md](../bench/README.md)). Not in the
  app.
- `tellfold-brand`: writes every logo SVG in `brand/` from one geometry (`cargo run -p tellfold-brand`, with
  `-- --appicon` for the Mac app icon PNGs). Not in the app.

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
To point the app at a local build without rebuilding it, run `cargo build --release -p tellfold-cli` and set
Settings > Advanced > Engine path to the repository (the app runs `engine-rs/target/release/tellfold`), to a
folder holding a `tellfold` binary, or to the binary itself.

`schemas/` holds the JSON Schemas of the replies the engine asks models for. They are part of the prompts, so the
engine embeds them as they are; edit them by hand, together with the types they describe.

## Golden fixtures

`golden/fixtures/` holds what the Python engine returned for the same inputs, generated one last time at the commit
that retired it (every generator reproduced the committed files byte for byte). They are frozen now: the Rust tests
compare against them, so a change that shows up in a fixture is a change to what run folders, the HTTP API or the CLI
look like. When that change is intended, edit the fixture in the same pull request and say why.
