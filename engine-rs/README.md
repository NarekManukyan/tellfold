# Tellfold engine in Rust

The Rust port of the Python engine in `tellfold/`. The plan, the milestones and the parity rules are in
[docs/rust-port.md](../docs/rust-port.md). The Python engine keeps shipping until the port reaches parity.

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

No test starts a real AI tool. For a manual check against one that is installed and logged in:

```bash
cargo run -p tellfold-runtimes --example ping -- claude-code
cargo run -p tellfold-runtimes --example ping -- cursor-agent
AGENTS=4 ROUNDS=1 cargo run -p tellfold-report --example run -- ../examples/mossbook_pricing.md "Will buyers accept the subscription?"
```

The second example runs a whole simulation with the report and prints every event and the run folder
(`cargo run -p tellfold-engine --example run` does the same without the report).

`schemas/` holds the JSON Schemas pydantic generates for the replies the engine asks models for. They are part of
the prompts, so Rust embeds them as they are.

## Golden fixtures

`golden/fixtures/` holds what the Python engine returns for the same inputs. Regenerate them from the repository
root when the Python behaviour they pin changes, in the same pull request (this also rewrites `schemas/`):

```bash
uv run python engine-rs/golden/make.py
```
