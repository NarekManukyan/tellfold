#!/bin/sh
# Stands in for `uv` so the Mac app's `uv run --project <path> tellfold serve ...` launch starts the Rust binary with
# the same arguments. The app only runs `uv` when an engine path is set, so set both: the engine path (this repository)
# and the uv path (this script), or TEST_RUNNER_TELLFOLD_ENGINE_PATH and TEST_RUNNER_TELLFOLD_UV_PATH in the Mac tests.
# TELLFOLD_RUST_BINARY overrides the binary (default: this workspace's release build, `cargo build --release -p
# tellfold-cli`).
if [ "$1" != "run" ] || [ "$2" != "--project" ] || [ "$4" != "tellfold" ]; then
    echo "uv-shim: expected 'run --project <path> tellfold ...', got: $*" >&2
    exit 2
fi
shift 4
here=$(cd "$(dirname "$0")" && pwd)
exec "${TELLFOLD_RUST_BINARY:-$here/../target/release/tellfold}" "$@"
