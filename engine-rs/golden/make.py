"""Write golden fixtures from the Python engine for the Rust port's parity tests.

Run from the repository root:  uv run python engine-rs/golden/make.py

The Rust tests read these files, so `cargo test` needs neither Python nor network. Regenerate them in the same pull
request as any change to the Python behaviour they pin.
"""

from __future__ import annotations

import json
import shutil
import sqlite3
import tempfile
from pathlib import Path

from tellfold import runner
from tellfold.models import ActionType, Action, AgentMemory, GodEvent, Persona, RoundStats, SimConfig
from tellfold.server.app import ActionOut, AgentDetail, PostOut, RunDetail, RunSummary
from tellfold.server.reader import RunReader
from tellfold.study import default_variant, letter
from tellfold.world import World

ROOT = Path(__file__).resolve().parents[2]
EXAMPLES = ROOT / "mac" / "Tellfold" / "Resources" / "Examples"
OUT = Path(__file__).resolve().parent / "fixtures"

TABLES = {
    "posts": "SELECT id, round, author_id, content, parent_id, repost_of, likes, dislikes, reposts, replies "
             "FROM posts ORDER BY id",
    "reactions": "SELECT agent_id, post_id, kind FROM reactions ORDER BY agent_id, post_id",
    "follows": "SELECT follower, followee FROM follows ORDER BY follower, followee",
    "actions": "SELECT seq, round, data FROM actions ORDER BY seq",
    "round_stats": "SELECT round, data FROM round_stats ORDER BY round",
    "events": "SELECT seq, round, content FROM events ORDER BY seq",
    "memories": "SELECT agent_id, notes FROM memories ORDER BY agent_id",
    "memory_history": "SELECT agent_id, round, notes FROM memory_history ORDER BY round, agent_id",
    "post_sentiment": "SELECT post_id, score FROM post_sentiment ORDER BY post_id",
    "sqlite_sequence": "SELECT name, seq FROM sqlite_sequence ORDER BY name",
}


def dump(name: str, value: object) -> None:
    path = OUT / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=1, sort_keys=False) + "\n", encoding="utf-8")


def tables(db: Path) -> dict[str, list[list[object]]]:
    conn = sqlite3.connect(db)
    try:
        return {name: [list(r) for r in conn.execute(sql)] for name, sql in TABLES.items()}
    finally:
        conn.close()


def reader_fixture(name: str, run_dir: Path) -> None:
    with RunReader(run_dir, immutable=True) as r:
        summary = RunSummary.model_validate({**r.summary(live=False), "example": True}).model_dump(mode="json")
        detail = RunDetail.model_validate({**r.detail(live=False), "example": True}).model_dump(mode="json")
        posts = [PostOut.model_validate(p).model_dump(mode="json") for p in r.posts()]
        actions = [ActionOut.model_validate(a).model_dump(mode="json") for a in r.actions()]
        agents = {p.id: AgentDetail.model_validate(r.agent(p.id)).model_dump(mode="json") for p in r.personas()}
        last = r.round_stats()[-1].round
        actions_last = [ActionOut.model_validate(a).model_dump(mode="json") for a in r.actions(last)]
        new_ids = r.new_post_ids(5)
    dump(f"examples/{name}/summary.json", summary)
    dump(f"examples/{name}/detail.json", detail)
    dump(f"examples/{name}/posts.json", posts)
    dump(f"examples/{name}/actions.json", actions)
    dump(f"examples/{name}/actions_last_round.json", actions_last)
    dump(f"examples/{name}/agents.json", agents)
    dump(f"examples/{name}/new_post_ids_after_5.json", new_ids)


def rewritten_fixture(name: str, run_dir: Path) -> None:
    """Each JSON file of the run read and written again by pydantic, as the Python engine would rewrite it."""
    from tellfold.models import KnowledgeGraph, Persona, Report, RunStatus

    out = {}
    with tempfile.TemporaryDirectory() as tmp:
        for file, model in (("config.json", SimConfig), ("graph.json", KnowledgeGraph), ("report.json", Report),
                            ("status.json", RunStatus), ("personas.json", list[Persona])):
            raw = (run_dir / file).read_text(encoding="utf-8")
            if file == "personas.json":
                value = [Persona.model_validate(p) for p in json.loads(raw)]
            else:
                value = model.model_validate_json(raw)
            runner.write_json(Path(tmp) / file, value)
            out[file] = (Path(tmp) / file).read_text(encoding="utf-8")
    dump(f"examples/{name}/rewritten.json", out)


def world_fixture(name: str, run_dir: Path) -> None:
    config = SimConfig.model_validate_json((run_dir / "config.json").read_text(encoding="utf-8"))
    world = World(run_dir / "world.db", seed=config.seed, read_only=True)
    try:
        last = world.last_round()
        feeds = {}
        for p in world.personas():
            for r in sorted({0, 1, last}):
                feeds[f"{p.id}@{r}"] = [post.id for post in world.feed_for(p.id, r, config.feed_size)]
        engagement = {p.id: {str(k): v for k, v in world.engagement_by(p.id).items()} for p in world.personas()}
        try:
            memory_rounds: list[int] | None = world.memory_rounds()
        except sqlite3.OperationalError:  # examples predate memory_history and are opened read-only
            memory_rounds = None
        out = {
            "seed": config.seed,
            "feed_size": config.feed_size,
            "last_round": last,
            "memory_rounds": memory_rounds,
            "feeds": feeds,
            "engagement": engagement,
            "follow_edges": [list(e) for e in world.follow_edges()],
            "memories": {p.id: world.memory(p.id).notes for p in world.personas()},
        }
    finally:
        world.close()
    dump(f"examples/{name}/world.json", out)

    # truncate_after on a writable copy, for every round it can branch from
    truncated = {}
    with tempfile.TemporaryDirectory() as tmp:
        for r in sorted(set(out["memory_rounds"] or []) | {0, last}):
            copy = Path(tmp) / f"r{r}.db"
            shutil.copyfile(run_dir / "world.db", copy)
            w = World(copy)
            w.truncate_after(r)
            w.close()
            truncated[str(r)] = tables(copy)
    dump(f"examples/{name}/truncated.json", truncated)


def scripted_world() -> None:
    """A small world built through the public API, dumped table by table (byte layout of the JSON columns)."""
    with tempfile.TemporaryDirectory() as tmp:
        db = Path(tmp) / "world.db"
        w = World(db, seed=3)
        w.add_personas([Persona(id=f"a{i:02d}", handle=f"user{i}", name=f"User {i}", bio="bio", background="bg",
                                stance="neutral", traits=["calm", "ünïcode"], activity=0.25 * i, influence=0.1 * i,
                                entity_id="acme" if i == 1 else None) for i in range(1, 6)])
        w.add_follow("a01", "a02")
        p1 = w.apply(0, Action(agent_id="a01", type=ActionType.POST, content="  Hello \"world\"\n line\t2  ",
                               reasoning="opening"))
        w.apply(1, Action(agent_id="a02", type=ActionType.REPLY, content="re", target_post_id=p1))
        w.apply(1, Action(agent_id="a03", type=ActionType.LIKE, target_post_id=p1))
        w.apply(1, Action(agent_id="a03", type=ActionType.LIKE, target_post_id=p1, reasoning="again"))
        w.apply(1, Action(agent_id="a04", type=ActionType.REPOST, target_post_id=p1, content="x" * 700))
        w.apply(1, Action(agent_id="a05", type=ActionType.FOLLOW, target_agent_id="a01"))
        w.apply(1, Action(agent_id="a05", type=ActionType.FOLLOW, target_agent_id="zz"))
        w.save_memory(AgentMemory(agent_id="a01", notes=["saw a post", "änderte Meinung", "quote \" and \\"]))
        w.snapshot_memories(1)
        w.save_round_stats(RoundStats(round=1, actions={"post": 2, "like": 1}, new_posts=2, active_agents=3,
                                      sentiment=0.25, llm_calls=2, llm_seconds=1.5))
        w.save_post_sentiment({p1: 0.4, 2: -3.0})
        w.add_event(GodEvent(round=2, content="Breaking"))
        w.close()
        dump("scripted_world.json", tables(db))


def files_fixture() -> None:
    """How write_json lays out a model and a list of models, byte for byte."""
    with tempfile.TemporaryDirectory() as tmp:
        d = Path(tmp)
        runner.write_json(d / "empty.json", [])
        runner.write_json(d / "one.json", [GodEvent(round=1, content="x")])
        dump("write_json.json", {n: (d / n).read_text(encoding="utf-8") for n in ("empty.json", "one.json")})


def estimates() -> None:
    rows = []
    for agents in (1, 5, 6, 12, 20, 40):
        for rounds in (1, 3, 6, 10):
            for batch in (1, 4, 6, 10):
                for chunks in (0, 1, 7, 64, 200):
                    for extra in ({}, {"cast_from": "x"}, {"branched_from": "x", "branch_round": 2}):
                        if extra.get("branch_round", 0) >= rounds:
                            continue
                        config = SimConfig(question="q", seed_files=[], num_agents=agents, rounds=rounds,
                                           batch_size=batch, **extra)
                        rows.append({"agents": agents, "rounds": rounds, "batch": batch, "chunks": chunks,
                                     **extra, "split": list(runner.estimate_call_split(config, chunks))})
    dump("estimates.json", rows)


def numbers() -> None:
    """Float text as pydantic and json.dumps write it, CPython's compensated sum(), and round()."""
    import random

    from pydantic import TypeAdapter

    values = [0.0, -0.0, 1.0, 0.1, 0.30000000000000004, 1e-5, 1e-7, 1.5e-7, -2.5e-9, 1e15, 1e16, 1e17, 1.234e21,
              123456789012345680.0, 0.001, -0.125, 1 / 3, 2 / 3 * 1e-10, 5e-324, 1.7976931348623157e308, 0.0001]
    rng = random.Random("golden:numbers")
    sums = []
    for n in (1, 2, 3, 5, 8, 13, 40):
        for _ in range(6):
            xs = [round(rng.uniform(-1, 1), rng.choice((1, 2, 3, 17))) for _ in range(n)]
            sums.append({"values": [repr(x) for x in xs], "sum": repr(sum(xs)), "mean3": repr(round(sum(xs) / n, 3))})
    sums.append({"values": ["1e100", "1.0", "-1e100"], "sum": repr(sum([1e100, 1.0, -1e100])), "mean3": "0.0"})
    rounds = [{"x": repr(x), "n": n, "r": repr(round(x, n))}
              for x in (0.125, 0.375, 2.675, 1.005, 36.75, 4.25, 0.045, 1234.5678, -0.0049, 0.5, 2.5)
              for n in (0, 2, 3)]
    dump("numbers.json", {
        "values": [repr(v) for v in values],
        "pydantic": TypeAdapter(list[float]).dump_json(values).decode(),
        "python": json.dumps(values),
        "sums": sums,
        "rounds": rounds,
        "ties_even": {repr(x): round(x) for x in (0.5, 1.5, 2.5, 0.3 * 5, 0.3 * 15, 0.3 * 25, 4.4999999)},
    })


LAX_CASES = [
    '{"i": true}', '{"i": 3.0}', '{"i": 3.5}', '{"i": "3"}', '{"i": " 3 "}', '{"i": "3.0"}', '{"i": 1e3}',
    '{"i": "1_000"}', '{"i": "+3"}', '{"i": "-7"}', '{"i": "x"}', '{"i": null}', '{"i": [1]}', '{"i": "1__0"}',
    '{"f": true}', '{"f": 2}', '{"f": "0.5"}', '{"f": " 0.5"}', '{"f": "1_0.5"}', '{"f": "x"}', '{"f": null}',
    '{"f": "-1e-3"}', '{"b": 1}', '{"b": 0}', '{"b": 2}', '{"b": "YES"}', '{"b": "off"}', '{"b": 1.0}',
    '{"b": "1"}', '{"b": "maybe"}', '{"b": null}', '{"s": 3}', '{"s": null}', '{"s": "ok"}', '{"o": null}',
    '{"o": "2"}', '{"o": 2.0}', '{"extra": 1}', '{}',
]


def lax() -> None:
    """pydantic's lax-mode coercion of JSON input, which models' replies rely on."""
    from pydantic import BaseModel

    class M(BaseModel):
        i: int = 0
        f: float = 0.0
        b: bool = False
        s: str = ""
        o: int | None = None

    out = []
    for raw in LAX_CASES:
        try:
            out.append({"input": raw, "output": M.model_validate_json(raw).model_dump()})
        except ValueError:
            out.append({"input": raw, "output": None})
    dump("lax.json", out)


def main() -> None:
    if OUT.exists():
        shutil.rmtree(OUT)
    for run_dir in sorted(p for p in EXAMPLES.iterdir() if p.is_dir()):
        reader_fixture(run_dir.name, run_dir)
        rewritten_fixture(run_dir.name, run_dir)
        world_fixture(run_dir.name, run_dir)
    scripted_world()
    files_fixture()
    estimates()
    numbers()
    lax()
    dump("letters.json", {"letters": [letter(i) for i in range(30)],
                          "variants": [default_variant(i) for i in range(30)]})
    print(f"wrote {sum(1 for _ in OUT.rglob('*.json'))} fixtures to {OUT}")


if __name__ == "__main__":
    main()
