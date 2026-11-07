"""Runtime/settings API routes, runtime fields on POST /api/runs, the token guard, `shoal serve`, `shoal runtimes`."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request
from collections.abc import Iterator
from pathlib import Path

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient
from typer.testing import CliRunner

from shoal.cli import app as cli_app
from shoal.cli import fake_responder
from shoal.llm import FakeLLM
from shoal.runtimes import RuntimeInfo
from shoal.server import create_app
from shoal.server.auth import TokenAuthMiddleware
from shoal.settings import Settings, load_settings

TOKEN = "t0ken-" + "x" * 26
SEED = b"# Ferry fares\n\nThe council wants to raise the ferry fare. Commuters object; shops approve.\n"


@pytest.fixture(autouse=True)
def isolated(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    monkeypatch.setenv("SHOAL_HOME", str(tmp_path / "shoal-home"))
    return tmp_path


def fake_detect(settings: Settings, *, check_auth: bool = False) -> list[RuntimeInfo]:
    return [RuntimeInfo(id="claude-code", kind="cli", name="Claude Code", installed=True, version="9.9",
                        auth="ok" if check_auth else "unknown", models=["haiku"], tested=True),
            RuntimeInfo(id="ollama", kind="http", name="Ollama", installed=False)]


@pytest.fixture
def client(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Iterator[TestClient]:
    monkeypatch.setattr("shoal.server.runtime_routes.detect_all", fake_detect)
    factory = lambda run_dir, concurrency, think=False: FakeLLM(fake_responder)  # noqa: E731
    app = create_app(tmp_path / "runs", factory, fake_llm=True, poll_interval=0.05, shutdown_grace=0.5,
                     settings_path=tmp_path / "settings.json")
    with TestClient(app) as c:
        yield c


def test_runtimes_endpoint(client: TestClient) -> None:
    res = client.get("/api/runtimes")
    assert res.status_code == 200
    body = res.json()
    assert [r["id"] for r in body] == ["claude-code", "ollama"] and body[0]["auth"] == "unknown"
    assert set(body[0]) == set(RuntimeInfo.model_fields)
    assert client.get("/api/runtimes?check_auth=1").json()[0]["auth"] == "ok"
    assert client.get("/api/runtimes?check_auth=7").status_code == 422


def test_settings_get_put_roundtrip_and_validation(client: TestClient, tmp_path: Path) -> None:
    assert client.get("/api/settings").json() == Settings().model_dump()
    new = Settings(agent_runtime="openai-compatible:lms", agent_model="qwen/qwen3-8b", concurrency=6,
                   openai_endpoints=[{"id": "lms", "name": "LM Studio", "base_url": "http://127.0.0.1:1234/v1"}])
    res = client.put("/api/settings", json=new.model_dump())
    assert res.status_code == 200, res.text
    assert load_settings(tmp_path / "settings.json") == new
    assert client.get("/api/settings").json()["agent_runtime"] == "openai-compatible:lms"
    bad = new.model_dump() | {"planner_runtime": "openai-compatible:nope"}
    assert client.put("/api/settings", json=bad).status_code == 400
    assert client.put("/api/settings", json=new.model_dump() | {"concurrency": 99}).status_code == 422
    assert client.put("/api/settings", json=new.model_dump() | {"ollama_base_url": "file:///etc"}).status_code == 422


def test_runtime_test_endpoint(client: TestClient) -> None:
    res = client.post("/api/runtimes/claude-code/test")
    assert res.status_code == 200 and res.json()["ok"] is True  # fake mode: no real call
    assert client.post("/api/runtimes/claude-code/test", json={"model": "sonnet"}).json()["model"] == "sonnet"
    assert client.post("/api/runtimes/nope/test").status_code == 404
    assert client.post("/api/runtimes/BAD ID/test").status_code == 400
    assert client.post("/api/runtimes/claude-code/test", json={"model": "x; rm -rf /"}).status_code == 400


def test_runtime_test_endpoint_real_path_reports_failure(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("ANTHROPIC_API_KEY", raising=False)
    app = create_app(tmp_path / "runs", lambda *a: FakeLLM(fake_responder), fake_llm=False,
                     settings_path=tmp_path / "settings.json")
    with TestClient(app) as c:
        body = c.post("/api/runtimes/anthropic-api/test").json()
    assert body["ok"] is False and "ANTHROPIC_API_KEY" in body["error"]


def post_run(client: TestClient, **extra: str):
    data = {"question": "Will the fare rise pass?", "agents": "3", "rounds": "1", "batch": "3", **extra}
    return client.post("/api/runs", data=data, files=[("files[]", ("seed.md", SEED, "text/markdown"))])


def test_post_runs_accepts_runtime_fields(client: TestClient, tmp_path: Path) -> None:
    res = post_run(client, agent_runtime="ollama", agent_model="llama3.2:3b", planner_model="qwen/qwen3-8b")
    assert res.status_code == 200, res.text
    config = json.loads((tmp_path / "runs" / res.json()["id"] / "config.json").read_text())
    assert config["agent_runtime"] == "ollama" and config["planner_runtime"] == "claude-code"
    assert config["planner_model"] == "qwen/qwen3-8b"


def test_post_runs_defaults_from_settings_and_rejects_unknown(client: TestClient, tmp_path: Path) -> None:
    (tmp_path / "settings.json").write_text(Settings(planner_runtime="anthropic-api").model_dump_json())
    res = post_run(client)
    assert res.status_code == 200, res.text
    config = json.loads((tmp_path / "runs" / res.json()["id"] / "config.json").read_text())
    assert config["agent_runtime"] == "claude-code" and config["planner_runtime"] == "anthropic-api"
    before = set((tmp_path / "runs").iterdir())
    assert post_run(client, agent_runtime="skynet").status_code == 400
    assert post_run(client, planner_runtime="../../etc").status_code == 400
    assert set((tmp_path / "runs").iterdir()) == before  # no run directory left behind


def test_token_middleware() -> None:
    app = FastAPI()

    @app.get("/api/ping")
    async def ping() -> dict:
        return {"ok": True}

    @app.get("/")
    async def index() -> dict:
        return {"page": True}

    app.add_middleware(TokenAuthMiddleware, token=TOKEN)
    with TestClient(app) as c:
        assert c.get("/api/ping").status_code == 401
        assert c.get("/api/ping").headers["www-authenticate"] == "Bearer"
        assert c.get("/api/ping", headers={"Authorization": "Bearer wrong"}).status_code == 401
        assert c.get("/api/ping", headers={"Authorization": TOKEN}).status_code == 401
        assert c.get("/api/ping", headers={"Authorization": f"Bearer {TOKEN}"}).json() == {"ok": True}
        assert c.get("/").status_code == 200
    with pytest.raises(ValueError):
        TokenAuthMiddleware(app, token="")


# ---------- CLI ----------


def test_cli_runtimes_json_and_table(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("shoal.runtimes.detect_all", fake_detect)
    runner = CliRunner()
    res = runner.invoke(cli_app, ["runtimes", "--json"])
    assert res.exit_code == 0, res.output
    assert [r["id"] for r in json.loads(res.output)] == ["claude-code", "ollama"]
    res = runner.invoke(cli_app, ["runtimes", "--check-auth"])
    assert res.exit_code == 0 and "claude-code" in res.output


def test_cli_run_rejects_unknown_runtime(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("SHOAL_FAKE_LLM", "1")
    seed = tmp_path / "seed.md"
    seed.write_bytes(SEED)
    res = CliRunner().invoke(cli_app, ["run", str(seed), "-q", "q?", "--agent-runtime", "skynet",
                                       "--out", str(tmp_path / "runs")])
    assert res.exit_code == 1 and "unknown runtime" in res.output
    assert not (tmp_path / "runs").exists()


def test_cli_run_records_runtimes_in_config(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("SHOAL_FAKE_LLM", "1")
    seed = tmp_path / "seed.md"
    seed.write_bytes(SEED)
    res = CliRunner().invoke(cli_app, ["run", str(seed), "-q", "Will it pass?", "--agents", "3", "--rounds", "1",
                                       "--agent-runtime", "ollama", "--out", str(tmp_path / "runs")])
    assert res.exit_code == 0, res.output
    (run_dir,) = (tmp_path / "runs").iterdir()
    config = json.loads((run_dir / "config.json").read_text())
    assert config["agent_runtime"] == "ollama" and config["planner_runtime"] == "claude-code"


def test_serve_prints_port_requires_token_and_exits_on_stdin_eof(tmp_path: Path) -> None:
    env = dict(os.environ, SHOAL_FAKE_LLM="1", SHOAL_SERVE_TOKEN=TOKEN, SHOAL_HOME=str(tmp_path / "home"))
    proc = subprocess.Popen([sys.executable, "-m", "shoal", "serve", "--port", "0", "--runs", str(tmp_path / "runs")],
                            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
    try:
        line = proc.stdout.readline().decode()
        assert proc.poll() is None, proc.stderr.read().decode()
        port = json.loads(line)["port"]
        assert isinstance(port, int) and port > 0

        def get(path: str, token: str | None) -> int:
            headers = {"Authorization": f"Bearer {token}"} if token else {}
            try:
                with urllib.request.urlopen(urllib.request.Request(f"http://127.0.0.1:{port}{path}",
                                                                   headers=headers), timeout=5) as r:
                    return r.status
            except urllib.error.HTTPError as e:
                return e.code

        assert get("/api/health", None) == 401
        assert get("/api/health", "nope") == 401
        assert get("/api/health", TOKEN) == 200
        assert get("/api/settings", TOKEN) == 200
        proc.stdin.close()
        assert proc.wait(timeout=15) == 0
        assert proc.stdout.read() == b""  # exactly one stdout line; logs go to stderr
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait()


def test_serve_rejects_short_token(tmp_path: Path) -> None:
    res = CliRunner().invoke(cli_app, ["serve", "--token", "short", "--runs", str(tmp_path / "runs")])
    assert res.exit_code == 2


def test_serve_port_zero_gives_each_server_its_own_port(tmp_path: Path) -> None:
    env = dict(os.environ, SHOAL_FAKE_LLM="1", SHOAL_SERVE_TOKEN=TOKEN, SHOAL_HOME=str(tmp_path / "home"))
    procs = [subprocess.Popen([sys.executable, "-m", "shoal", "serve", "--runs", str(tmp_path / f"runs{i}")],
                              stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env)
             for i in range(2)]
    try:
        ports = [json.loads(p.stdout.readline())["port"] for p in procs]
        assert ports[0] != ports[1]
    finally:
        for p in procs:
            p.stdin.close()
        deadline = time.monotonic() + 15
        for p in procs:
            try:
                p.wait(timeout=max(0.1, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                p.kill()
