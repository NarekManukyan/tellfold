"""Find agent CLIs on disk and read their versions, quickly and without a login shell.

The Mac app starts the engine with a minimal PATH, so lookup also searches the usual install folders:
~/.local/bin, Homebrew, /usr/local/bin, and npm / bun / volta / nvm global bins. Child processes get the same
extended PATH, because most of these CLIs are Node scripts that run `#!/usr/bin/env node`.
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
from functools import lru_cache
from pathlib import Path

VERSION_TIMEOUT_S = 3.0
AUTH_TIMEOUT_S = 5.0
_ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07]*\x07")
_VERSION = re.compile(r"\d+\.\d+(?:\.\d+)?(?:[-+.][0-9A-Za-z.-]+)?")


def strip_ansi(text: str) -> str:
    return _ANSI.sub("", text).replace("\r", "")


def extra_bin_dirs() -> list[Path]:
    home = Path.home()
    dirs = [
        home / ".local" / "bin",
        Path("/opt/homebrew/bin"),
        Path("/usr/local/bin"),
        home / ".npm-global" / "bin",
        home / ".bun" / "bin",
        home / ".volta" / "bin",
        home / ".opencode" / "bin",
        home / ".cargo" / "bin",
        home / "bin",
    ]
    prefix = os.environ.get("NPM_CONFIG_PREFIX") or os.environ.get("npm_config_prefix")
    if prefix:
        dirs.insert(1, Path(prefix) / "bin")
    nvm = home / ".nvm" / "versions" / "node"
    try:
        versions = sorted((p for p in nvm.iterdir() if p.is_dir()), key=_node_version_key, reverse=True)
        dirs.extend(v / "bin" for v in versions)
    except OSError:
        pass
    return dirs


def _node_version_key(path: Path) -> tuple[int, ...]:
    return tuple(int(x) for x in re.findall(r"\d+", path.name)[:3]) or (0,)


def search_path() -> str:
    """os.environ PATH followed by the extra install folders (deduplicated)."""
    parts = [p for p in os.environ.get("PATH", "").split(os.pathsep) if p]
    for d in extra_bin_dirs():
        if str(d) not in parts:
            parts.append(str(d))
    for d in ("/usr/bin", "/bin", "/usr/sbin", "/sbin"):
        if d not in parts:
            parts.append(d)
    return os.pathsep.join(parts)


def find_binary(*names: str) -> str | None:
    path = search_path()
    for name in names:
        found = shutil.which(name, path=path)
        if found:
            return found
    return None


# Variables a CLI child may inherit from the engine. Everything else is dropped: the Mac app puts every Keychain
# secret (ANTHROPIC_API_KEY, OPENAI_API_KEY, SHOAL_KEY_*) and the engine's own bearer token (SHOAL_SERVE_TOKEN) in
# the engine's environment, and none of them may reach a third-party agent CLI. A runtime that needs one of its
# own variables (GEMINI_API_KEY, CURSOR_API_KEY, ...) names it in `passthrough`.
CHILD_ENV_ALLOW = frozenset({
    "HOME", "USER", "LOGNAME", "SHELL", "LANG", "LANGUAGE", "TMPDIR", "TZ", "TERM", "COLORTERM",
    "__CF_USER_TEXT_ENCODING", "SSH_AUTH_SOCK",
    "HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "no_proxy", "all_proxy",
    "SSL_CERT_FILE", "SSL_CERT_DIR", "NODE_EXTRA_CA_CERTS", "REQUESTS_CA_BUNDLE",
    "NVM_DIR", "NVM_BIN", "VOLTA_HOME", "BUN_INSTALL", "NPM_CONFIG_PREFIX", "npm_config_prefix",
})
CHILD_ENV_ALLOW_PREFIXES = ("LC_", "XDG_")
# Never passed to a child, even when a runtime asks for it.
CHILD_ENV_DENY = frozenset({"SHOAL_SERVE_TOKEN", "ANTHROPIC_API_KEY", "OPENAI_API_KEY"})
CHILD_ENV_DENY_PREFIXES = ("SHOAL_KEY_",)


def is_secret_env(name: str) -> bool:
    """True for the engine's own secrets, which no child process may see."""
    return name in CHILD_ENV_DENY or name.startswith(CHILD_ENV_DENY_PREFIXES)


def child_env(binary: str | None = None, extra: dict[str, str] | None = None,
              passthrough: tuple[str, ...] | frozenset[str] = ()) -> dict[str, str]:
    """Environment for a CLI child: an allowlist of ours (plus `passthrough`), with the extended PATH (binary's
    own folder first). Engine secrets are always removed."""
    env = {k: v for k, v in os.environ.items()
           if (k in CHILD_ENV_ALLOW or k.startswith(CHILD_ENV_ALLOW_PREFIXES) or k in passthrough)
           and not is_secret_env(k)}
    path = search_path()
    if binary:
        folder = str(Path(binary).parent)
        path = os.pathsep.join([folder, *[p for p in path.split(os.pathsep) if p != folder]])
    env["PATH"] = path
    env.setdefault("NO_COLOR", "1")
    env.setdefault("TERM", "dumb")
    env.setdefault("CI", "1")  # fewer interactive niceties (spinners, update prompts) in most Node CLIs
    if extra:
        env.update(extra)
    return env


def run_quick(argv: list[str], *, timeout: float) -> tuple[int | None, str]:
    """Run a short command, return (exit code or None on timeout/error, combined output stripped of ANSI)."""
    try:
        done = subprocess.run(argv, capture_output=True, text=True, timeout=timeout, check=False,
                              stdin=subprocess.DEVNULL, env=child_env(argv[0]), cwd=_neutral_cwd())
    except (OSError, subprocess.TimeoutExpired, ValueError):
        return None, ""
    return done.returncode, strip_ansi((done.stdout or "") + "\n" + (done.stderr or "")).strip()


def _neutral_cwd() -> str:
    tmp = os.environ.get("TMPDIR") or "/tmp"
    return tmp if os.path.isdir(tmp) else "/"


def parse_version(output: str) -> str | None:
    for line in output.splitlines():
        match = _VERSION.search(line)
        if match:
            return match.group(0)
    return output.strip().splitlines()[0][:60] if output.strip() else None


def binary_version(path: str, args: tuple[str, ...] = ("--version",)) -> str | None:
    """`<binary> --version` with a 3 s timeout, cached per (path, mtime)."""
    try:
        mtime = os.stat(path).st_mtime
    except OSError:
        return None
    return _cached_version(path, mtime, args)


@lru_cache(maxsize=64)
def _cached_version(path: str, mtime: float, args: tuple[str, ...]) -> str | None:
    code, out = run_quick([path, *args], timeout=VERSION_TIMEOUT_S)
    if code is None:
        return None
    return parse_version(out)
