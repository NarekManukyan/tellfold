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


SCHEMAS = Path(__file__).resolve().parents[1] / "schemas"


def schema_classes() -> dict[str, type]:
    """Every pydantic model the engine sends to `llm.json()`, by class name."""
    from tellfold import engine, graph, personas
    from tellfold.models import AgreementCall, CampMatches, CompareReport, Report, RewriteSuggestion

    classes = [Report, CompareReport, RewriteSuggestion, CampMatches, AgreementCall, graph.Extraction,
               personas.PersonaBatch, personas.VoicePick, engine.RoundDecisions, engine.OpeningPosts,
               engine.SentimentScores]
    return {c.__name__: c for c in classes}


def schemas() -> None:
    """The JSON Schemas sent to models, as pydantic generates them. The Rust engine embeds these files: their text
    is part of the prompt, so they are the source of truth, not something Rust derives."""
    from tellfold.runtimes.base import json_system

    if SCHEMAS.exists():
        shutil.rmtree(SCHEMAS)
    SCHEMAS.mkdir()
    systems = {}
    for name, cls in schema_classes().items():
        schema = cls.model_json_schema()
        (SCHEMAS / f"{name}.json").write_text(json.dumps(schema, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        systems[name] = json_system("You are a careful analyst.  \n", schema)
    dump("prompts/json_system.json", systems)


def prompts() -> None:
    from tellfold.runtimes.base import classify_failure, repair_prompt, root_opener

    previous = ["", "   ", "x" * 2000, "é" * 1600 + "tail", '{"color": "teal"}']
    repairs = [{"prompt": "Pick one\n", "problem": problem, "previous": prev,
                "out": repair_prompt("Pick one\n", problem, prev)}
               for problem in ("1 validation error", "p" * 900) for prev in previous]
    messages = ["Error: Rate limit reached", "HTTP 429", "unknown model 'x'", "Not logged in. Run login",
                "  segfault\n\n core dumped ", "", "403 Forbidden", "quota exceeded for model", "\u00e9" * 500]
    classified = []
    for m in messages:
        for hint in (None, "Run `tool login`."):
            err = classify_failure(m, auth_hint=hint)
            classified.append({"message": m, "hint": hint, "kind": type(err).__name__, "text": str(err),
                               "rate_limited": getattr(err, "rate_limited", False)})
    dump("prompts/repair.json", repairs)
    dump("prompts/classify.json", classified)
    dump("prompts/root_opener.json", {"array": root_opener({"type": "array"}), "object": root_opener({})})
    texts = ["plain", "é€😀", "a\u2028b", 'q"\\']
    dump("prompts/ascii_dumps.json", [{"value": t, "dumps": json.dumps(t), "dumps_list": json.dumps([t, 1.5, None])}
                                     for t in texts])


EXTRACT_CASES = [
    '{"a": 1}', 'Here you go: {"a": {"b": [1, 2]}} done', '```json\n{"a": 2}\n```', 'noise {not json} then {"a": 3}',
    '[1, 2] and {"a": 4}', '{"a": 1} [1, 2]', 'just words', '```\n[3]\n```', '```JSON {"z": true}```',
    '{"revised_text": "Dear neighbours,\n\nWe opened\tnine years ago.", "changes": [{"what": "x", "cites": [1]}]}',
    '{"a": "\\u00e9\\ud83d\\ude00", "b": 1e3, "c": -0.0, "d": 12345678901234567890}',
    '{"a": [1, 2,]} {"b": 2}', '{"dup": 1, "dup": 2, "x": null}', '{"a": "unterminated', '{"a" 1} {"ok": [true, false]}',
    'text ```json\n{"bad": }\n``` then {"good": 1}', '{"a": "\\x"} {"b": 1}', '   [ ]   ', '{"n": 01}', '{"n": .5}',
    '{"a": {"b": {"c": {"d": [[[]]]}}}}', '{"key": "val\u0001ue"}', '{"\u00e9": "\u4e2d"}', '[{"a": 1}, {"b": 2}]',
]


def extract() -> None:
    from tellfold.llm import extract_json

    out = []
    for text in EXTRACT_CASES:
        for prefer in ("{", "["):
            try:
                value = extract_json(text, prefer=prefer)
                out.append({"text": text, "prefer": prefer, "value": value})
            except ValueError as e:
                out.append({"text": text, "prefer": prefer, "error": str(e)})
    dump("extract_json.json", out)


def ingest() -> None:
    from tellfold.ingest import _normalize, chunk_text, load_documents

    corpus = {p.name: p.read_text(encoding="utf-8") for p in sorted((ROOT / "examples").glob("*.md"))}
    corpus["cjk.txt"] = "第一句话很长。" * 120 + "\n\n" + "これは文です！" * 80
    corpus["spaces.txt"] = "  a\t\tb \f c\v\r\n\r\n\n\n\nnext\x00 para\x1c  \n" + ("word " * 700) + "\n\n" + "x" * 2500
    corpus["sentences.txt"] = " ".join(f"Sentence number {i} is here." for i in range(80)) + " " + "x" * 900
    corpus["unicode_ws.txt"] = "one\u2003two\u00a0three\n\u2028\nfour\n \u3000 \nfive " + "lorem ipsum " * 300
    normalized = {name: _normalize(text) for name, text in corpus.items()}
    docs = [(name, text) for name, text in normalized.items() if text]
    chunked = {f"{size}/{overlap}": [c.model_dump() for c in chunk_text(docs, size=size, overlap=overlap)]
               for size, overlap in ((1800, 200), (400, 60), (300, 40), (120, 0), (90, 44))}
    dump("ingest/corpus.json", corpus)
    dump("ingest/normalized.json", normalized)
    dump("ingest/chunks.json", chunked)
    errors = {}
    for size, overlap in ((100, 80), (0, 0), (10, 5), (-5, 0), (10, -1)):
        try:
            chunk_text([("a", "b")], size=size, overlap=overlap)
            errors[f"{size}/{overlap}"] = None
        except ValueError as e:
            errors[f"{size}/{overlap}"] = str(e)
    dump("ingest/chunk_errors.json", errors)

    # encodings: the bytes on disk and what load_documents reads from them
    text = "Café owners on Pier Street are à bout de patience. Œuvre «quoted» – 20 €. " * 12
    encoded = {"utf8.txt": text.encode("utf-8"), "utf8sig.txt": text.encode("utf-8-sig"),
               "latin1.txt": text.replace("Œ", "O").replace("–", "-").replace("€", "E").encode("latin-1"),
               "cp1252.txt": text.encode("cp1252"), "utf16.txt": text.encode("utf-16")}
    files = {}
    with tempfile.TemporaryDirectory() as tmp:
        for name, raw in encoded.items():
            (Path(tmp) / name).write_bytes(raw)
            [(_, read)] = load_documents([Path(tmp) / name])
            files[name] = {"hex": raw.hex(), "text": read}
    dump("ingest/encodings.json", files)
    pdf()


def pdf() -> None:
    import pymupdf

    from tellfold.ingest import load_documents

    out = OUT / "ingest" / "brief.pdf"
    with pymupdf.open() as doc:
        page = doc.new_page()
        page.insert_text((72, 72), "Harbor Town shuttle pilot starts in May.")
        page.insert_text((72, 100), "The Riders Union calls it a quiet service cut.")
        page.insert_text((72, 140), "Café owners on Pier Street worry about fewer stops.")
        page = doc.new_page()
        page.insert_text((72, 72), "Page two: the council votes on Monday.")
        doc.save(out)
    [(name, text)] = load_documents([out])
    dump("ingest/pdf.json", {"name": name, "text": text})


# ---------- runtimes ----------

CLI_CLASSES = ("CursorAgentLLM", "CodexLLM", "GeminiLLM", "QwenLLM", "OpenCodeLLM", "CopilotLLM", "GrokLLM",
               "DroidLLM")


def _outcome(fn) -> dict[str, object]:
    from tellfold.runtimes.base import BadOutput, Fatal, Transient

    try:
        reply = fn()
    except Transient as e:
        return {"error": "transient", "message": str(e), "rate_limited": e.rate_limited}
    except Fatal as e:
        return {"error": "fatal", "message": str(e)}
    except BadOutput as e:
        return {"error": "bad_output", "message": str(e), "reply_text": e.reply_text}
    return {"text": reply.text, "model": reply.model}


def runtimes() -> None:
    import httpx
    from claude_agent_sdk._internal.transport.subprocess_cli import SubprocessCLITransport
    from pydantic import BaseModel

    from tellfold import llm as llm_module
    from tellfold.runtimes import cli_tools, detect, http_api, registry
    from tellfold.runtimes.base import AdapterRequest
    from tellfold.settings import OpenAIEndpoint, Settings, key_env_for

    cli_tools.opencode_major_version.cache_clear()
    argv = []
    for name in CLI_CLASSES:
        cls = getattr(cli_tools, name)
        tool = cls("/opt/tools/tool")
        for model in ("gpt-5", "default", ""):
            request = AdapterRequest(prompt="the prompt", system="  be brief\n", model=model, purpose="engine.round")
            built, stdin = tool.build(request, "/W")
            argv.append({"class": name, "runtime_id": cls.runtime_id, "model": model, "argv": built, "stdin": stdin,
                         "extra_env": tool.extra_env(), "passthrough": list(cls.passthrough_env),
                         "json_output": cls.json_output, "unsafe": cls.unsafe, "auth_hint": cls.auth_hint})
    dump("runtimes/cli_argv.json", argv)

    stdouts = [
        "", "   ", "plain words\n", "\x1b[1mbold\x1b[0m answer\r\n",
        json.dumps({"type": "result", "is_error": False, "result": "fine", "model": "m"}),
        json.dumps({"response": "gem", "stats": {}}),
        json.dumps({"is_error": True, "result": "Please log in"}),
        json.dumps({"error": {"message": "quota exceeded"}}),
        json.dumps({"error": "bad model: unknown model zz"}),
        json.dumps({"error": {"code": 7}}),
        json.dumps({"error": "x", "result": "but answered"}),
        json.dumps({"subtype": "error"}),
        json.dumps({"stats": {}}),
        json.dumps({"result": "   ", "text": "from text"}),
        json.dumps({"content": [{"text": "a"}, {"text": ""}, {"content": "b"}]}),
        json.dumps([{"type": "system"}, {"type": "result", "result": "qw"}]),
        json.dumps([{"type": "result", "is_error": True, "error": "rate limit hit"}]),
        json.dumps([{"type": "message", "content": "one"}, {"type": "assistant", "message": {"text": "two"}},
                    {"type": "message", "role": "user", "content": "skip"}]),
        '{"type":"step_start"}\n{"type":"text","part":{"type":"text","text":"op"}}\n'
        '{"type":"text","part":{"type":"text","text":"en"}}\n',
        '{"type":"step_start"}\n{"type":"error","error":{"message":"not logged in"}}\n',
        '{"type":"step_start"}\nnoise\n{"type":"done"}\n',
        "progress\n{\"result\": \"last\"}\n",
        "42", '"just a string"', "true", "null", "[]", "[1, 2]",
    ]
    replies = [{"stdout": out, **_outcome(lambda out=out: cli_tools.parse_json_reply(out, auth_hint="HINT"))}
               for out in stdouts]
    dump("runtimes/parse_json_reply.json", replies)

    bodies = ['{"error": {"message": "invalid key"}}', '{"error": {"type": "auth"}}', '{"error": {"code": 1}}',
              '{"error": "plain"}', '{"error": "", "message": "msg"}', '{"detail": [1, "two", null, true, 1.5]}',
              '[1, 2]', '"text"', "not   json\n at all", "", '{"message": {"nested": "x"}}', '{"error": {}}']
    excerpts = [{"body": b, "excerpt": http_api._excerpt(httpx.Response(400, content=b.encode()))} for b in bodies]
    dump("runtimes/excerpt.json", excerpts)

    dump("runtimes/misc.json", {
        "ollama_context": [[n, http_api.ollama_context_length("x" * n)] for n in
                           (0, 5, 12287, 12288, 12289, 36863, 36865, 60_000, 200_000, 10_000_000)],
        "key_env_for": [[i, key_env_for(i)] for i in ("open-router.ai", "lmstudio", "LM Studio", "__x__", "ß-1")],
        "parse_version": [[o, detect.parse_version(o)] for o in
                          ("codex-cli 0.9.1\n", "2026.01.23-916f423", "2.1.283 (Claude Code)", "no digits here",
                           "", "  \n", "v1.2", "x" * 80, "first\n1.2.3-beta+build", "1.2.3.4.5")],
        "strip_ansi": [[t, detect.strip_ansi(t)] for t in ("\x1b[1mbold\x1b[0m", "a\r\nb", "\x1b]0;title\x07x")],
        "opencode_lockdown": cli_tools.OPENCODE_LOCKDOWN_CONFIG,
        "anthropic_model_id": [[m, http_api.AnthropicAPILLM.model_id(m)] for m in (None, "", "haiku", "sonnet",
                                                                                  "opus", "claude-x")],
    })

    settings_cases = [
        {}, {"concurrency": "8", "think": "yes", "agent_runtime": " ollama "},
        {"concurrency": 0}, {"concurrency": 17}, {"concurrency": True}, {"think": 2},
        {"agent_runtime": "Bad Runtime"}, {"agent_model": ""}, {"agent_model": "bad model"},
        {"planner_runtime": "openai-compatible:lm"}, {"ollama_base_url": "http://h:1/"},
        {"ollama_base_url": "ftp://h"}, {"ollama_base_url": "http://u:p@h"}, {"runs_dir": 5},
        {"openai_endpoints": [{"id": "LMStudio", "name": "LM Studio", "base_url": "http://127.0.0.1:1234/v1/",
                               "models": [" a ", "a", "", "qwen/qwen3-8b"], "key_env": "  "}]},
        {"openai_endpoints": [{"id": "a", "name": "A", "base_url": "http://h"},
                              {"id": "A", "name": "B", "base_url": "http://h"}]},
        {"openai_endpoints": [{"id": "x", "name": "", "base_url": "http://h"}]},
        {"openai_endpoints": [{"id": "x", "name": "X", "base_url": "http://h", "key_env": "sk-123!"}]},
        {"openai_endpoints": [{"id": "x", "name": "X", "base_url": "http://h", "models": ["bad model"]}]},
        {"openai_endpoints": [{"id": "-x", "name": "X", "base_url": "http://h"}]},
        {"unknown_field": 1, "agent_model": "llama3.2:3b"},
    ]
    out = []
    for case in settings_cases:
        try:
            settings = Settings.model_validate(case)
        except ValueError:
            out.append({"input": case, "ok": False})
            continue
        out.append({"input": case, "ok": True, "json": settings.model_dump_json(indent=2),
                    "endpoints": [{"runtime_id": e.runtime_id, "api_key_env": e.api_key_env}
                                  for e in settings.openai_endpoints]})
    dump("runtimes/settings.json", out)

    class Pick(BaseModel):
        color: str
        n: int

    claude = registry.IsolatedClaudeLocal(cli_path="/opt/bin/claude")
    commands = []
    for purpose, schema, think in (("engine.round", None, False), ("report.write", None, False),
                                   ("graph.extract", Pick.model_json_schema(), False),
                                   ("graph.extract", Pick.model_json_schema(), True)):
        client = claude if not think else registry.IsolatedClaudeLocal(
            cli_path="/opt/bin/claude", think_purposes=llm_module.PLANNER_THINK_PURPOSES)
        system = "Be terse. ünïcode" if schema is None else llm_module._json_system("Sys", schema, structured=True)
        options = client._options(system, "haiku", schema, purpose)
        commands.append({"purpose": purpose, "think": think, "schema": schema, "system": system,
                         "argv": SubprocessCLITransport(prompt="p", options=options)._build_command()})
    dump("runtimes/claude_argv.json", commands)

    failures = [("API Error: 429", 429, "api_error"), ("Overloaded", 529, "api_error"),
                ("Failed to authenticate: OAuth session expired", None, "api_error"),
                ("forbidden", 403, None), ("structured output retries exhausted", None, None), ("", None, None),
                ("usage limit reached", None, None), ("  boom  ", 500, None), ("odd", None, "api_error"),
                ("please /login", 400, None)]
    classified = []
    for message, status, reason in failures:
        err = llm_module._classify_api_failure(message, status, reason)
        classified.append({"message": message, "status": status, "terminal_reason": reason,
                           "kind": type(err).__name__, "text": str(err),
                           "rate_limited": getattr(err, "rate_limited", None)})
    dump("runtimes/claude_failures.json", classified)
    dump("runtimes/claude_system.json", {
        "structured": llm_module._json_system("Sys \n", Pick.model_json_schema(), structured=True),
        "plain": llm_module._json_system("Sys \n", Pick.model_json_schema(), structured=False),
        "schema": Pick.model_json_schema(),
    })


def unicode_tables() -> None:
    """Python's own Unicode tables for the two places the engine leans on them: the `\\w` class of `re` and
    `unicodedata.combining`. Written as Rust source so slugs and word matches use Python's Unicode version."""
    import sys
    import unicodedata

    def ranges(pred) -> list[tuple[int, int]]:
        out, start = [], None
        for cp in range(0x110000):
            ok = pred(chr(cp))
            if ok and start is None:
                start = cp
            elif not ok and start is not None:
                out.append((start, cp - 1))
                start = None
        if start is not None:
            out.append((start, 0x10FFFF))
        return out

    def table(name: str, rs: list[tuple[int, int]]) -> str:
        rows = "".join(f"    (0x{a:04X}, 0x{b:04X}),\n" for a, b in rs)
        return f"pub(crate) const {name}: &[(u32, u32)] = &[\n{rows}];\n"

    version = f"{sys.version_info.major}.{sys.version_info.minor}"
    text = (f"//! Generated by `engine-rs/golden/make.py` from Python {version} (Unicode {unicodedata.unidata_version}). "
            "Do not edit.\n\n"
            "/// Code points `re`'s `\\w` matches: `str.isalnum()` or `_`.\n"
            + table("WORD", ranges(lambda c: c.isalnum() or c == "_"))
            + "\n/// Code points with a non-zero `unicodedata.combining()`.\n"
            + table("COMBINING", ranges(lambda c: unicodedata.combining(c) != 0)))
    (Path(__file__).resolve().parents[1] / "crates/tellfold-engine/src/unicode_tables.rs").write_text(text)


def rng() -> None:
    """CPython's `random.Random` seeded from strings, as the engine seeds it."""
    import random

    out = []
    for seed in ["shoal:active:7:1", "q|20", "", "é€😀 and more", "shoal:follows:42", "x" * 300]:
        r = random.Random(seed)
        floats = [r.random() for _ in range(12)]
        bits = [r.getrandbits(k) for k in (1, 5, 31, 32, 1, 17)]
        below = [r.randrange(n) for n in (1, 2, 3, 7, 100, 1000003)]
        items = list(range(13))
        r.shuffle(items)
        picks = [r.choices(range(4), weights=[0.6, 0.01, 2.5, 0.3], k=1)[0] for _ in range(8)]
        out.append({"seed": seed, "random": floats, "bits": bits, "below": below, "shuffle": items, "choices": picks})
    dump("engine/rng.json", out)


GOLDEN_QUESTION = "Will the Acme widget subscription win over small landlords?"


def golden_seed_text() -> str:
    """A seed document long enough for two extraction groups."""
    paragraphs = []
    for i in range(1, 15):
        paragraphs.append(
            f"Section {i}. Acme Corporation sells widgets to small landlords across the region. Jane Doe, its chief "
            f"executive, says the new subscription saves money. The City Council worries about tenants, and "
            f"landlord groups ask whether paragraph {i} of the contract can change after signing. "
            + "Prices, support hours and repair times are listed in detail. " * 8
        )
    return "\n\n".join(paragraphs) + "\n"


def golden_responder(purpose: str, prompt: str, schema):
    """The canned responder, plus a hand-made graph (so entity voices exist) and one failing round batch.
    `tests/golden.rs` in tellfold-engine builds the same answers."""
    import re as _re

    from tellfold.cli import fake_responder
    from tellfold.llm import LLMError

    if purpose == "graph.extract":
        first = int(_re.search(r'<chunk id="(\d+)"', prompt).group(1))
        return {
            "entities": [
                {"name": "Acme Corporation", "kind": "company", "summary": "Acme makes widgets.",
                 "aliases": ["Acme"], "chunk_ids": [first]},
                {"name": "Jane Doe", "kind": "person", "summary": "Chief executive of Acme.", "aliases": [],
                 "chunk_ids": [first, 999]},
                {"name": f"Topic {first}", "kind": "concept", "summary": "", "aliases": [], "chunk_ids": []},
                {"name": "the City Council", "kind": "government", "summary": "Local regulator.",
                 "aliases": ["Council"], "chunk_ids": [first]},
                {"name": "Small landlords", "kind": "community", "summary": "Owners of a few flats.",
                 "aliases": []},
            ],
            "relations": [
                {"source": "Acme", "target": "Jane Doe", "label": "Employs ", "evidence": "Jane runs it."},
                {"source": "Council", "target": "Acme Corporation", "label": "regulates", "evidence": ""},
                {"source": "Jane Doe", "target": "nobody", "label": "x"},
            ],
        }
    if purpose == "personas.cast":
        ids = _re.findall(r"^- (\S+) \[", prompt, _re.MULTILINE)
        return {"entity_ids": [*reversed(ids), "nope"]}
    if purpose == "engine.round" and "Round 2 of" in prompt and "=== a04 " in prompt:
        return LLMError("rate limited (fake)")
    return fake_responder(purpose, prompt, schema)


def normalized_event(name: str, payload: dict) -> list:
    if name == "status":
        payload = {**payload, "pid": 0, "started_at": "T", "updated_at": "T"}
    return [name, payload]


def golden_run() -> None:
    """A whole run on the canned responder: the run folder, every model call and every event."""
    import anyio

    from tellfold.llm import FakeLLM
    from tellfold.models import GodEvent

    out = OUT / "engine" / "run"
    out.mkdir(parents=True, exist_ok=True)
    seed = out / "seed.md"
    seed.write_text(golden_seed_text(), encoding="utf-8")
    config = SimConfig(question=GOLDEN_QUESTION, seed_files=["seed.md"], num_agents=9, rounds=3, batch_size=3,
                       feed_size=6, memory_size=2, seed=11,
                       events=[GodEvent(round=2, content="Acme doubles the price."),
                               GodEvent(round=2, content="Acme doubles the price.")])
    llm = FakeLLM(golden_responder)
    events: list = []
    with tempfile.TemporaryDirectory() as tmp:
        run_dir = Path(tmp) / "run"
        anyio.run(lambda: runner.run_pipeline(llm, run_dir, config, [seed],
                                               on_event=lambda n, p: events.append(normalized_event(n, p))))
        status = json.loads((run_dir / "status.json").read_text())
        status.update(pid=0, started_at="T", updated_at="T")
        dump("engine/run/expected.json", {
            "config": config.model_dump(mode="json"),
            "config_json": (run_dir / "config.json").read_text(),
            "graph_json": (run_dir / "graph.json").read_text(),
            "personas_json": (run_dir / "personas.json").read_text(),
            "status": status,
            "tables": tables(run_dir / "world.db"),
            "calls": [list(c) for c in llm.calls if not c[0].startswith("report.")],
            "events": events,
        })


def engine_helpers() -> None:
    """Pure, seeded helpers of graph.py, personas.py and engine.py on the example runs and on edge cases."""
    from tellfold import engine, graph, personas
    from tellfold.ingest import chunk_text, load_documents
    from tellfold.models import KnowledgeGraph

    names = ["Acme Corp.", "The Acme Company Ltd", "Café Ünïcode", "  São Paulo & Rio  ", "O'Brien’s Pub",
             "東京 タワー", "Ελληνικά ΣΟΦΙΑ", "a_b__c", "---", "", "x²³ ½", "ﬁne ligature", "İstanbul", "Hindi हिन्दी",
             "the", "The A", "Ⓐcme", "Straße", "١٢٣ arabic digits", "tab\tnew\nline"]
    kinds = ["Company", " PERSON ", "orgs", "people", "cities", "unknown", "", "events", "Brands", "s"]
    long = ("First sentence here. Second sentence follows! Third one? " * 12).strip()
    caps = [(long, 480), (long, 100), ("word " * 120, 220), ("a" * 600, 480), ("x, y; z: " * 60, 50),
            ("short", 10), ("é" * 300 + " end", 200), ("One. " + "b" * 300, 120)]
    helpers: dict = {
        "slugify": [[n, graph.slugify(n)] for n in names],
        "match_key": [[n, graph.match_key(n)] for n in names],
        "normalize_kind": [[k, graph.normalize_kind(k)] for k in kinds],
        "cap": [[t, n, graph._cap(t, n)] for t, n in caps],
        "agent_id": [[i, t, personas.agent_id(i, t)] for i, t in [(0, 5), (8, 9), (9, 10), (99, 100), (0, 1000)]],
        "clip": [[t, n, engine._clip(t, n)] for t, n in [("  a   b  ", 10), ("word " * 40, 30), ("é" * 50, 10)]],
    }
    taken = {"jane", "jane_2", "user", "acme_corporation_te"}
    handles = [["@Jane", "x"], ["", "Jane Doe"], ["", ""], ["___", "!!!"], ["Ünïcode Name", ""],
               ["Acme Corporation Team Account", ""], ["a" * 30, ""], ["j.a.n.e", ""], ["jane", ""]]
    helpers["unique_handle"] = [[r, n, personas.unique_handle(r, n, set(taken))] for r, n in handles]

    examples = {}
    for run_dir in sorted(p for p in EXAMPLES.iterdir() if p.is_dir()):
        g = KnowledgeGraph.model_validate_json((run_dir / "graph.json").read_text(encoding="utf-8"))
        cast = [Persona.model_validate(p) for p in json.loads((run_dir / "personas.json").read_text())]
        config = SimConfig.model_validate_json((run_dir / "config.json").read_text(encoding="utf-8"))
        seeds = sorted((run_dir / "seeds").iterdir())
        chunks = chunk_text(load_documents(seeds))
        examples[run_dir.name] = {
            "question": config.question,
            "chunk_count": len(chunks),
            "select_chunks": {str(limit): [c.id for c in graph.select_chunks(chunks, config.question, limit)]
                              for limit in (2, 3, 5, 8)},
            "plan_slots": {str(n): [[s.label, s.brief, s.entity_id, list(s.activity_band)]
                                    for s in personas.plan_slots(g, question=config.question, n=n)]
                           for n in (1, 2, 7, 20, 101)},
            "plan_slots_no_voices": [[s.label, s.brief, s.entity_id, list(s.activity_band)]
                                     for s in personas.plan_slots(g, question=config.question, n=6, voices=[])],
            "persona_digest": personas.graph_digest(g),
            "engine_digest": engine._graph_digest(g),
            "follows": {str(seed): [list(e) for e in engine.build_follow_graph(cast, g, seed=seed)]
                        for seed in (7, 8, 12345)},
            "opening": {str(seed): [p.id for p in engine.pick_opening_authors(cast, seed=seed)] for seed in (7, 99)},
            "active": {f"{seed}:{r}:{news}": [p.id for p in engine.sample_active(cast, r, seed=seed, news=news)]
                       for seed in (7, 3) for r in (1, 2, 5) for news in (False, True)},
            "active_quiet": [p.id for p in engine.sample_active(
                [p.model_copy(update={"activity": 0.0}) for p in cast], 1, seed=7)],
            "persona_blocks": [engine._persona_block(p, AgentMemory(agent_id=p.id, notes=["R0: hi", "R1: yo"]))
                               for p in cast[:3]],
        }
    helpers["examples"] = examples

    merged_parts = [
        graph.Extraction.model_validate(golden_responder("graph.extract", '<chunk id="0">', None)),
        graph.Extraction.model_validate(golden_responder("graph.extract", '<chunk id="4">', None)),
        graph.Extraction.model_validate({"entities": [
            {"name": "ACME corp", "kind": "Company", "summary": "Acme   makes widgets.", "chunk_ids": [7, 2]},
            {"name": "Council", "kind": "org", "summary": "local regulator."},
            {"name": "Jane Doe Inc", "kind": "people", "summary": "x" * 600, "aliases": ["J", "JD"]},
            {"name": "!!!", "kind": "person"},
            {"name": "Acme Corporation", "kind": "person"},
        ], "relations": [{"source": "acme", "target": "council", "label": "lobbies", "evidence": "w " * 200},
                         {"source": "Council", "target": "Acme", "label": "regulates", "evidence": "late evidence"}]}),
    ]
    helpers["merge"] = {"parts": [p.model_dump(mode="json") for p in merged_parts],
                        "graph": graph.merge_extractions(merged_parts).model_dump(mode="json")}
    view = engine._AgentView(persona=Persona(id="a02", handle="jane", name="Jane", bio="", background="",
                                             stance=""), feed=[])
    view.allowed_posts.update({3, 5})
    view.visible_authors.update({"a01", "a03"})
    handles_index = {"a01": "a01", "a03": "a03", "bob": "a01", "carol": "a03", "dan": "a04"}
    proposals = [
        {"type": " Comment ", "target_post_id": "#3", "content": "  hi  ", "reasoning": "r"},
        {"type": "like", "target_post_id": 4, "reasoning": "nope"},
        {"type": "like"},
        {"type": "retweet", "target_post_id": 5, "content": "c"},
        {"type": "post", "target_post_id": 5, "content": " new "},
        {"type": "follow", "target_agent": "@Bob"},
        {"type": "follow", "target_agent": "dan"},
        {"type": "follow"},
        {"type": "dance", "reasoning": "why"},
        {"type": "skip"},
        {"type": "dislike", "target_post_id": "post 3"},
        {"type": "upvote", "target_post_id": 5.0},
    ]
    helpers["to_action"] = [
        [p, engine._to_action(view, engine.ProposedAction.model_validate(p), handles_index).model_dump(mode="json")]
        for p in proposals]
    dump("engine/helpers.json", helpers)

    canned = {}
    probe = ("Users to decide for: a01, a02 and a10. Feed: [#12] [#7] post 3, POST-ID: 44, #12 again.\n"
             "- item-1 [organization]: x")
    for name, cls in schema_classes().items():
        from tellfold.cli import fake_responder
        canned[name] = {"agents": fake_responder("engine.round", probe, cls),
                        "plain": fake_responder("report.write", "nothing here", cls)}
    canned["text"] = fake_responder("chat.interview", "x", None)
    dump("engine/canned.json", {"probe": probe, "replies": canned})


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
    schemas()
    prompts()
    extract()
    ingest()
    runtimes()
    unicode_tables()
    rng()
    engine_helpers()
    golden_run()
    dump("letters.json", {"letters": [letter(i) for i in range(30)],
                          "variants": [default_variant(i) for i in range(30)]})
    print(f"wrote {sum(1 for _ in OUT.rglob('*.json'))} fixtures to {OUT} and {len(schema_classes())} schemas")


if __name__ == "__main__":
    main()
