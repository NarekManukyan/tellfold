#!/bin/bash
# Builds the engine into an app bundle's Resources/engine, so Tellfold runs without uv, Python or the repository:
#
#   engine/tellfold   the Rust engine (engine-rs/, the `tellfold-cli` package), one Mach-O for Apple silicon
#
# The app starts it as `engine/tellfold serve ...`. It links only system libraries, so nothing else goes next to it.
#
# Usage: bundle-engine.sh <repo root> <destination engine folder> [codesign identity]
# Cargo keeps its build in <repo root>/engine-rs/target, so a build with unchanged sources does no work. Xcode's
# Debug configuration uses the `app-debug` profile (no link-time optimisation); everything else uses `release`.
# The engine, like the Release app, is for Apple silicon only.
set -euo pipefail

REPO=$(cd "$1" && pwd)
DEST=$2
IDENTITY=${3:--}
TARGET=aarch64-apple-darwin
# The app's own minimum (LSMinimumSystemVersion in mac/project.yml).
export MACOSX_DEPLOYMENT_TARGET=15.0

CARGO=$(command -v cargo || true)
for candidate in "$HOME/.cargo/bin/cargo" /opt/homebrew/bin/cargo /usr/local/bin/cargo; do
    [ -n "$CARGO" ] && break
    [ -x "$candidate" ] && CARGO=$candidate
done
if [ -z "$CARGO" ]; then
    echo "error: Rust is needed to build the bundled engine (https://rustup.rs)." >&2
    exit 1
fi
# rustup's cargo finds rustc next to itself; Xcode's PATH for build scripts does not have ~/.cargo/bin.
PATH="$(dirname "$CARGO"):$PATH"
export PATH

# The target directory stays inside this checkout: another worktree's build must never land in this app.
unset CARGO_TARGET_DIR
if [ "${CONFIGURATION:-}" = "Debug" ]; then PROFILE=app-debug; else PROFILE=release; fi
"$CARGO" build --manifest-path "$REPO/engine-rs/Cargo.toml" --profile "$PROFILE" --locked --target "$TARGET" \
    -p tellfold-cli
BINARY="$REPO/engine-rs/target/$TARGET/$PROFILE/tellfold"

MINOS=$(vtool -show-build "$BINARY" | awk '$1 == "minos" { print $2 }')
if [ "$MINOS" != "$MACOSX_DEPLOYMENT_TARGET" ]; then
    echo "error: $BINARY is built for macOS $MINOS, not $MACOSX_DEPLOYMENT_TARGET." >&2
    exit 1
fi

rm -rf "${DEST:?}"
mkdir -p "$DEST"
cp "$BINARY" "$DEST/tellfold"

# The app's own signature then seals the folder. Outside Debug, a real identity gets the hardened runtime and a
# timestamp, which notarization requires; the engine needs no entitlements under it. Debug builds skip both, so
# they never wait on Apple's timestamp server.
if [ "$IDENTITY" = "-" ] || [ "${CONFIGURATION:-}" = "Debug" ]; then
    codesign --force --timestamp=none --sign "$IDENTITY" "$DEST/tellfold"
else
    codesign --force --timestamp --options runtime --sign "$IDENTITY" "$DEST/tellfold"
fi
"$DEST/tellfold" serve --help >/dev/null
echo "Bundled engine: $(du -sh "$DEST" | cut -f1) in $DEST"
