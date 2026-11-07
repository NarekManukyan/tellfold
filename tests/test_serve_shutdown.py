"""`shoal serve` must not outlive the Mac app when a request thread is stuck in a blocking call."""

from __future__ import annotations

import subprocess
import sys
import time


def test_exit_later_ends_a_process_whose_main_thread_is_stuck() -> None:
    code = (
        "import threading\n"
        "from shoal.cli import _exit_later\n"
        "_exit_later(0.2)\n"
        "threading.Event().wait()\n"  # never returns on its own
    )
    start = time.monotonic()
    proc = subprocess.run([sys.executable, "-c", code], timeout=20)
    assert proc.returncode == 0
    assert time.monotonic() - start < 15


def test_fake_delay_parsing_and_slow_fake_llm(monkeypatch) -> None:
    import anyio

    from shoal import cli

    monkeypatch.setenv("SHOAL_FAKE_LLM", "1")
    for raw, want in [("", 0.0), ("abc", 0.0), ("-1", 0.0), ("0.05", 0.05), ("99", 10.0)]:
        monkeypatch.setenv("SHOAL_FAKE_DELAY", raw)
        assert cli.fake_delay() == want
    monkeypatch.setenv("SHOAL_FAKE_DELAY", "0.05")
    llm = cli.make_llm(None, 1)

    async def call() -> float:
        start = time.monotonic()
        await llm.text("hi", system="s", model="haiku", purpose="chat")
        return time.monotonic() - start

    assert anyio.run(call) >= 0.04


def test_force_exit_kills_running_agent_cli_groups(tmp_path) -> None:
    """The force-exit runs when the event loop is stuck, so the cancellation path that kills agent CLIs never
    runs; their process groups must be killed before os._exit or they keep running (and spending tokens)."""
    import os
    import signal

    pid_file = tmp_path / "child.pid"
    child = tmp_path / "agent"
    child.write_text(f"#!{sys.executable}\nimport os, time\nopen({str(pid_file)!r}, 'w').write(str(os.getpid()))\n"
                     "time.sleep(60)\n", encoding="utf-8")
    child.chmod(0o755)
    code = (
        "import asyncio, os, time\n"
        "from shoal.cli import _exit_later\n"
        "from shoal.runtimes.cli_tools import run_process\n"
        "async def main():\n"
        f"    task = asyncio.ensure_future(run_process([{str(child)!r}], stdin=None, cwd='/', env=dict(os.environ)))\n"
        f"    while not os.path.exists({str(pid_file)!r}):\n"
        "        await asyncio.sleep(0.02)\n"
        "    _exit_later(0.2)\n"
        "    time.sleep(60)\n"  # a stuck event loop: cancellation never gets to run
        "asyncio.run(main())\n"
    )
    proc = subprocess.run([sys.executable, "-c", code], timeout=30)
    assert proc.returncode == 0
    pid = int(pid_file.read_text())
    deadline = time.monotonic() + 5
    alive = True
    while alive and time.monotonic() < deadline:
        try:
            os.kill(pid, 0)
            time.sleep(0.05)
        except ProcessLookupError:
            alive = False
    if alive:
        os.kill(pid, signal.SIGKILL)
    assert not alive, "the agent CLI outlived the engine"
