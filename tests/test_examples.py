"""Built-in example runs: packed read-only, listed next to normal runs, never changed by the server.

Every model call goes through FakeLLM. The example is packed from a fake run and its folder made read-only, like
the signed app bundle it ships in.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import sqlite3
import stat
from collections.abc import Iterator
from pathlib import Path

import anyio
import pytest
from fastapi.testclient import TestClient

from tellfold.cli import fake_responder
from tellfold.examples import ExampleError, pack_example
from tellfold.llm import FakeLLM
from tellfold.models import SimConfig
from tellfold.runner import copy_seeds, create_run_dir, run_pipeline
from tellfold.server import create_app

SEED_TEXT = (
    "# Harbour Town votes on a new ferry fare\n\n"
    "The council proposes raising the ferry fare from 2 to 3 euros to fund an electric ferry. Commuters oppose it; "
    "shops support the new boat. The mayor decides after a consultation.\n"
)
BUNDLED = Path(__file__).resolve().parent.parent / "mac" / "Tellfold" / "Resources" / "Examples"


def fake_run(out: Path, question: str = "Will Harbour Town accept the ferry fare rise?") -> Path:
    seed = out.parent / "harbour.md"
    seed.write_text(SEED_TEXT, encoding="utf-8")
    run_dir = create_run_dir(out, question)
    seeds = copy_seeds(run_dir, [seed])
    config = SimConfig(question=question, seed_files=[str(s) for s in seeds], num_agents=4, rounds=2, batch_size=2)
    anyio.run(lambda: run_pipeline(FakeLLM(fake_responder), run_dir, config, seeds))
    return run_dir


def freeze(path: Path) -> None:
    for p in [path, *path.rglob("*")]:
        p.chmod(stat.S_IRUSR | stat.S_IXUSR if p.is_dir() else stat.S_IRUSR)


def thaw(path: Path) -> None:
    for p in [path, *path.rglob("*")]:
        p.chmod(stat.S_IRWXU)


def snapshot(path: Path) -> dict[str, str]:
    return {str(p.relative_to(path)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(path.rglob("*")) if p.is_file()}


@pytest.fixture(scope="module")
def packed(tmp_path_factory: pytest.TempPathFactory) -> Iterator[Path]:
    """`examples/cafe`, packed from a finished fake run and frozen read-only."""
    root = tmp_path_factory.mktemp("examples-src")
    run_dir = fake_run(root / "runs")
    examples = root / "examples"
    pack_example(run_dir, examples / "cafe")
    freeze(examples)
    yield examples
    thaw(examples)


@pytest.fixture
def runs_dir(tmp_path: Path) -> Path:
    path = tmp_path / "runs"
    path.mkdir()
    return path


class Ready:
    def __init__(self, value: bool) -> None:
        self.value = value

    async def __call__(self) -> bool:
        return self.value


@pytest.fixture
def ready() -> Ready:
    return Ready(True)


@pytest.fixture
def client(runs_dir: Path, packed: Path, ready: Ready) -> Iterator[TestClient]:
    app = create_app(runs_dir, lambda run_dir, concurrency, think: FakeLLM(fake_responder), fake_llm=True,
                     poll_interval=0.05, shutdown_grace=0.5, examples_dir=packed, runtime_ready=ready)
    with TestClient(app) as c:
        yield c


def test_pack_example_ships_a_self_contained_read_only_world(packed: Path) -> None:
    folder = packed / "cafe"
    names = {p.name for p in folder.iterdir()}
    assert {"config.json", "personas.json", "world.db", "report.json", "report.html", "status.json", "seeds"} <= names
    assert not {"world.db-wal", "world.db-shm"} & names
    config = json.loads((folder / "config.json").read_text())
    assert config["seed_files"] == ["seeds/harbour.md"]
    assert json.loads((folder / "status.json").read_text())["pid"] is None
    html = (folder / "report.html").read_text()
    assert "Seed files: seeds/harbour.md." in html and str(packed.parent) not in html
    conn = sqlite3.connect(f"{(folder / 'world.db').as_uri()}?mode=ro&immutable=1", uri=True)
    assert conn.execute("PRAGMA journal_mode").fetchone()[0] == "delete"
    conn.close()


def test_pack_example_refuses_an_unfinished_run_or_prompt_text(tmp_path: Path) -> None:
    run_dir = fake_run(tmp_path / "runs")
    with (run_dir / "calls.jsonl").open("a") as f:
        f.write(json.dumps({"purpose": "x", "prompt": "secret"}) + "\n")
    with pytest.raises(ExampleError, match="prompt"):
        pack_example(run_dir, tmp_path / "out")
    (run_dir / "report.json").unlink()
    with pytest.raises(ExampleError, match="not a finished run"):
        pack_example(run_dir, tmp_path / "out2")


def test_examples_are_listed_next_to_normal_runs(client: TestClient, runs_dir: Path) -> None:
    normal = fake_run(runs_dir)
    runs = {r["id"]: r for r in client.get("/api/runs").json()}
    assert runs["example-cafe"]["example"] is True
    assert runs["example-cafe"]["state"] == "done" and runs["example-cafe"]["has_report"]
    assert runs[normal.name]["example"] is False

    d = client.get("/api/runs/example-cafe").json()
    assert d["example"] is True and d["report"] is not None and d["personas"] and not d["live"]
    posts = client.get("/api/runs/example-cafe/posts").json()
    assert posts
    agent = d["personas"][0]["id"]
    assert client.get(f"/api/runs/example-cafe/agents/{agent}").status_code == 200
    assert client.get("/api/runs/example-cafe/report.html").status_code == 200
    assert [s["name"] for s in client.get("/api/runs/example-cafe/seeds").json()] == ["harbour.md"]
    assert client.get("/api/runs/example-cafe/seeds/harbour.md").text == SEED_TEXT


def test_mutating_routes_refuse_examples(client: TestClient, packed: Path) -> None:
    before = snapshot(packed)
    for path, body in [("events", {"round": 2, "content": "news"}), ("stop", None), ("report", None)]:
        res = client.post(f"/api/runs/example-cafe/{path}", json=body)
        assert res.status_code == 403, (path, res.text)
        assert "example" in res.json()["detail"]
    assert snapshot(packed) == before


def test_interviews_and_questions_leave_the_example_untouched(client: TestClient, packed: Path) -> None:
    before = snapshot(packed)
    agent = client.get("/api/runs/example-cafe").json()["personas"][0]["id"]
    res = client.post("/api/runs/example-cafe/chat", json={"agent_id": agent, "message": "Why?"})
    assert res.status_code == 200, res.text and res.json()["reply"]
    res = client.post("/api/runs/example-cafe/ask", json={"question": "Who changed their mind?"})
    assert res.status_code == 200, res.text
    assert snapshot(packed) == before


def test_interviews_need_an_ai_tool(client: TestClient, ready: Ready) -> None:
    ready.value = False
    agent = client.get("/api/runs/example-cafe").json()["personas"][0]["id"]
    res = client.post("/api/runs/example-cafe/chat", json={"agent_id": agent, "message": "Why?"})
    assert res.status_code == 409 and "AI tool" in res.json()["detail"]
    res = client.post("/api/runs/example-cafe/ask", json={"question": "Why?"})
    assert res.status_code == 409 and "AI tool" in res.json()["detail"]


def test_a_normal_run_with_the_same_id_prefix_does_not_collide(client: TestClient, runs_dir: Path) -> None:
    lookalike = fake_run(runs_dir)
    lookalike = lookalike.rename(runs_dir / "example-cafe-2")
    same_folder = fake_run(runs_dir, "Another question").rename(runs_dir / "cafe")
    shadow = fake_run(runs_dir, "Third question")
    shutil.copytree(shadow, runs_dir / "example-cafe")  # cannot take the example's id

    runs = client.get("/api/runs").json()
    ids = [r["id"] for r in runs]
    assert ids.count("example-cafe") == 1
    flags = {r["id"]: r["example"] for r in runs}
    assert flags["example-cafe"] is True
    assert flags[lookalike.name] is False and flags[same_folder.name] is False

    assert client.get("/api/runs/example-cafe").json()["config"]["question"] != "Third question"
    d = client.get(f"/api/runs/{lookalike.name}").json()
    assert d["example"] is False
    assert client.post(f"/api/runs/{lookalike.name}/stop").status_code == 409  # normal rules, not 403
    assert client.post(f"/api/runs/{same_folder.name}/report").status_code == 200


@pytest.mark.skipif(not BUNDLED.is_dir(), reason="no bundled examples in this checkout")
def test_bundled_examples_are_shippable(tmp_path: Path) -> None:
    folders = [p for p in sorted(BUNDLED.iterdir()) if p.is_dir()]
    assert folders
    app = create_app(tmp_path / "runs", lambda *a: FakeLLM(fake_responder), fake_llm=True, examples_dir=BUNDLED)
    with TestClient(app) as c:
        runs = [r for r in c.get("/api/runs").json() if r["example"]]
        assert len(runs) == len(folders)
        for run in runs:
            d = c.get(f"/api/runs/{run['id']}").json()
            assert run["state"] == "done" and d["report"]["summary"] and d["personas"]
            assert c.get(f"/api/runs/{run['id']}/posts").json()
    for folder in folders:
        assert not (folder / "world.db-wal").exists() and not (folder / "world.db-shm").exists()
        assert all(not os.path.isabs(s) for s in json.loads((folder / "config.json").read_text())["seed_files"])
