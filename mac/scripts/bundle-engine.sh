#!/bin/bash
# Builds a self-contained engine into an app bundle's Resources/engine, so Tellfold runs without uv, a system
# Python or the repository:
#
#   engine/python/   a relocatable CPython (python-build-standalone, via uv) with the locked dependencies
#   engine/app/      the tellfold package source, found through a .pth file in site-packages
#
# The app starts it as `engine/python/bin/python3 -I -B -m tellfold serve ...`. Everything is bytecode-compiled
# here, because the signed bundle must not be written to at run time.
#
# Usage: bundle-engine.sh <repo root> <destination engine folder> [codesign identity]
# The Python runtime with its dependencies is cached in <repo root>/mac/build/engine-cache, keyed by uv.lock,
# so only the first build after a dependency change downloads anything.
set -euo pipefail

REPO=$(cd "$1" && pwd)
DEST=$2
IDENTITY=${3:--}
PY_VERSION=3.13
ENTITLEMENTS="$REPO/mac/scripts/engine.entitlements"

UV=$(command -v uv || true)
for candidate in "$HOME/.local/bin/uv" "$HOME/.cargo/bin/uv" /opt/homebrew/bin/uv /usr/local/bin/uv; do
    [ -n "$UV" ] && break
    [ -x "$candidate" ] && UV=$candidate
done
if [ -z "$UV" ]; then
    echo "error: uv is needed to build the bundled engine (https://docs.astral.sh/uv/)." >&2
    exit 1
fi

ARCH=$(uname -m)
KEY=$( (cat "$REPO/uv.lock" "$REPO/pyproject.toml" "$0"; echo "$PY_VERSION $ARCH") | shasum -a 256 | cut -c1-16)
CACHE="$REPO/mac/build/engine-cache"
RUNTIME="$CACHE/runtime-$KEY"

if [ ! -x "$RUNTIME/bin/python3" ]; then
    echo "Building the engine runtime ($KEY)…"
    rm -rf "$CACHE"
    mkdir -p "$CACHE/pythons"
    UV_PYTHON_INSTALL_DIR="$CACHE/pythons" "$UV" python install --no-bin "$PY_VERSION"
    SRC_PY=$(UV_PYTHON_INSTALL_DIR="$CACHE/pythons" "$UV" python find --managed-python "$PY_VERSION")
    SRC_ROOT=$(cd "$(dirname "$SRC_PY")/.." && pwd)
    STAGE="$CACHE/stage"
    cp -R "$SRC_ROOT" "$STAGE"
    # A managed Python refuses `pip install` into itself; this runtime is ours to fill.
    rm -f "$STAGE"/lib/python3.*/EXTERNALLY-MANAGED

    # pymupdf 2 pulls in an ML page-layout stack (onnxruntime, numpy, networkx: ~200 MB). The engine only calls
    # `page.get_text()`, which works without it, so those packages and their own dependencies stay out.
    REQS="$CACHE/requirements.txt"
    (cd "$REPO" && "$UV" export --frozen --no-dev --no-emit-project --no-hashes --format requirements-txt \
        --no-emit-package pymupdf-layout --no-emit-package pymupdf4llm --no-emit-package onnxruntime \
        --no-emit-package numpy --no-emit-package networkx --no-emit-package flatbuffers \
        --no-emit-package protobuf) > "$REQS"
    "$UV" pip install --python "$STAGE/bin/python3" --no-deps --link-mode copy --no-cache -r "$REQS"

    # Trim what the engine never uses. The Claude Code binary that ships inside claude-agent-sdk (~300 MB) goes
    # too: Tellfold runs the user's own `claude`, which is the one that is signed in.
    SITE=$(echo "$STAGE"/lib/python3.*/site-packages)
    rm -rf "$SITE"/claude_agent_sdk/_bundled/claude "$SITE"/pip "$SITE"/pip-*.dist-info \
        "$SITE"/pymupdf/layout "$SITE"/pymupdf/mupdf-devel \
        "$STAGE"/lib/python3.*/test "$STAGE"/lib/python3.*/idlelib "$STAGE"/lib/python3.*/tkinter \
        "$STAGE"/lib/python3.*/turtledemo "$STAGE"/lib/python3.*/ensurepip "$STAGE"/lib/python3.*/lib2to3 \
        "$STAGE"/lib/tcl* "$STAGE"/lib/tk* "$STAGE"/lib/libtcl* "$STAGE"/lib/libtk* "$STAGE"/lib/itcl* "$STAGE"/lib/thread* \
        "$STAGE"/lib/python3.*/lib-dynload/_tkinter* "$STAGE"/include "$STAGE"/share \
        "$STAGE"/bin/idle* "$STAGE"/bin/pip* "$STAGE"/bin/pydoc* "$STAGE"/bin/2to3*
    find "$STAGE" -name __pycache__ -type d -prune -exec rm -rf {} +
    # The package source lives next to the runtime; site-packages points at it (paths in .pth files are
    # relative to site-packages): site-packages -> python3.x -> lib -> python -> engine.
    echo "../../../../app" > "$SITE/tellfold-engine.pth"
    "$STAGE/bin/python3" -I -m compileall -q -j 0 --invalidation-mode unchecked-hash "$STAGE/lib" >/dev/null
    mv "$STAGE" "$RUNTIME"
fi

mkdir -p "$DEST"
rsync -a --delete "$RUNTIME/" "$DEST/python/"
mkdir -p "$DEST/app"
rsync -a --delete --exclude "__pycache__" --exclude "*.pyc" "$REPO/tellfold" "$DEST/app/"
"$DEST/python/bin/python3" -I -m compileall -q -j 0 --invalidation-mode unchecked-hash "$DEST/app" >/dev/null

# Sign every Mach-O inside, innermost first; the app's own signature then seals the folder. A Developer ID
# identity gets the hardened runtime, which notarization requires.
if [ "$IDENTITY" = "-" ]; then
    SIGN_FLAGS="--force --sign -"
else
    SIGN_FLAGS="--force --timestamp --options runtime --entitlements $ENTITLEMENTS --sign $IDENTITY"
fi
quiet() { "$@" 2>&1 | { grep -v "replacing existing signature" || true; }; }
quiet sh -c 'find "$1" -type f \( -name "*.so" -o -name "*.dylib" \) -print0 | xargs -0 -n 50 codesign '"$SIGN_FLAGS" _ "$DEST/python"
find "$DEST/python/bin" -type f -perm -u+x -print0 | while IFS= read -r -d '' f; do
    if file -b "$f" | grep -q "Mach-O"; then quiet codesign $SIGN_FLAGS "$f"; fi
done
"$DEST/python/bin/python3" -I -B -c "import tellfold.cli, tellfold.server.app, tellfold.ingest, pymupdf, claude_agent_sdk"
echo "Bundled engine:$(du -sh "$DEST" | cut -f1) in $DEST"
