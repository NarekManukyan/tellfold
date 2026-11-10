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

`schemas/` holds the JSON Schemas pydantic generates for the replies the engine asks models for. They are part of
the prompts, so Rust embeds them as they are.

## Golden fixtures

`golden/fixtures/` holds what the Python engine returns for the same inputs. Regenerate them from the repository
root when the Python behaviour they pin changes, in the same pull request (this also rewrites `schemas/`):

```bash
uv run python engine-rs/golden/make.py
```
