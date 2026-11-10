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

## Golden fixtures

`golden/fixtures/` holds what the Python engine returns for the same inputs. Regenerate them from the repository
root when the Python behaviour they pin changes, in the same pull request:

```bash
uv run python engine-rs/golden/make.py
```
