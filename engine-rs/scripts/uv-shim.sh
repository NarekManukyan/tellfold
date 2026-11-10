#!/bin/sh
# Stands in for `uv` so the Mac app's debug launch (`uv run --project <path> tellfold serve ...`) starts the Rust
# binary with the same arguments. Point the app at it with the uv path setting, or in the Mac tests with
# TEST_RUNNER_TELLFOLD_UV_PATH. TELLFOLD_RUST_BINARY overrides the binary (default: this workspace's release build).
if [ "$1" != "run" ] || [ "$2" != "--project" ] || [ "$4" != "tellfold" ]; then
    echo "uv-shim: expected 'run --project <path> tellfold ...', got: $*" >&2
    exit 2
fi
shift 4
here=$(cd "$(dirname "$0")" && pwd)
exec "${TELLFOLD_RUST_BINARY:-$here/../target/release/tellfold}" "$@"
