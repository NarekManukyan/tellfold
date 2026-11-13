"""Record what the Python Tellfold Studio API answers to a session of requests on the canned model.

Run from the repository root:  uv run python engine-rs/golden/make_api.py

Writes golden/fixtures/api/session.json. The Rust test (crates/tellfold-server/tests/api.rs) sends the same requests
to the Rust server in a fresh folder and compares status codes, content types, the headers listed in HEADERS and
bodies, after the same normalisation: the folder becomes `<root>`, run and study ids become `{run:N}` / `{study:N}`
(numbered in the order they appear on disk), timestamps, pids and call durations are masked.

Steps marked `wait` are not compared: they poll until runs, report rebuilds or studies are finished. A `copy` step
copies a finished run into the examples folder, which makes it the read-only example `example-demo`.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
OUT = Path(__file__).resolve().parent / "fixtures" / "api"
SEED = (ROOT / "examples" / "mossbook_pricing.md").read_text(encoding="utf-8")
QUESTION = "Will existing buyers accept the subscription?"
HEADERS = ["cache-control", "content-disposition", "www-authenticate", "x-accel-buffering"]
TIMEOUT = 60.0

TIMING = re.compile(r'"seconds":\s?-?\d+(?:\.\d+)?(?:e[+-]?\d+)?')
LLM_TIMING = re.compile(r'"llm_seconds":\s?-?\d+(?:\.\d+)?(?:e[+-]?\d+)?')
PID = re.compile(r'"pid":\s?\d+')
STAMP = re.compile(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:\d\d)?")
GOOD_RUN = {"question": QUESTION, "agents": "4", "rounds": "2", "batch": "2", "seed": "7",
            "events": json.dumps([{"round": 2, "content": "A rival app says it will stay a one-time purchase"}])}


def form(fields: dict[str, str] | None = None, files: list[tuple[str, str, str]] | None = None) -> dict[str, Any]:
    return {"fields": [[k, v] for k, v in (fields or {}).items()],
            "files": [list(f) for f in (files if files is not None else [("files", "mossbook.md", SEED)])]}


def step(name: str, method: str, path: str, **extra: Any) -> dict[str, Any]:
    return {"name": name, "method": method, "path": path, **extra}


def steps() -> list[dict[str, Any]]:
    run0, run1 = "{run:0}", "{run:1}"
    return [
        step("health", "GET", "/api/health"),
        step("no runs yet", "GET", "/api/runs"),
        step("no studies yet", "GET", "/api/studies"),
        step("no seed file", "POST", "/api/runs", form=form({"question": QUESTION}, [])),
        step("not multipart", "POST", "/api/runs", raw="question=x", content_type="application/x-www-form-urlencoded"),
        step("wrong suffix", "POST", "/api/runs", form=form({"question": QUESTION}, [("files", "notes.png", "x")])),
        step("no question", "POST", "/api/runs", form=form({"question": "  "})),
        step("rounds not a number", "POST", "/api/runs", form=form({"question": QUESTION, "rounds": "abc"})),
        step("rounds too many", "POST", "/api/runs", form=form({"question": QUESTION, "rounds": "99"})),
        step("bad model", "POST", "/api/runs", form=form({"question": QUESTION, "agent_model": "bad model"})),
        step("bad events", "POST", "/api/runs", form=form({"question": QUESTION, "events": "{nope"})),
        step("event out of range", "POST", "/api/runs",
             form=form({"question": QUESTION, "rounds": "2", "events": '[{"round": 3, "content": "late"}]'})),
        step("unknown runtime", "POST", "/api/runs", form=form({"question": QUESTION, "agent_runtime": "nope"})),
        step("study without cast", "POST", "/api/runs", form=form({"question": QUESTION, "variant": "B"})),
        step("cast from nothing", "POST", "/api/runs", form=form({"question": QUESTION, "cast_from": "missing"})),
        step("empty seed", "POST", "/api/runs", form=form({"question": QUESTION}, [("files", "empty.md", "")])),
        step("create run", "POST", "/api/runs",
             form=form(GOOD_RUN, [("files[]", "Mossbook Pricing.md", SEED), ("files", "notes.txt", "Buyers liked it.\n")])),
        step("wait for run", "WAIT", "", wait={"run": run0}),
        step("runs", "GET", "/api/runs"),
        step("run detail", "GET", f"/api/runs/{run0}"),
        step("posts", "GET", f"/api/runs/{run0}/posts"),
        step("posts since 3", "GET", f"/api/runs/{run0}/posts?since_id=3"),
        step("posts since -1", "GET", f"/api/runs/{run0}/posts?since_id=-1"),
        step("posts since abc", "GET", f"/api/runs/{run0}/posts?since_id=abc"),
        step("actions", "GET", f"/api/runs/{run0}/actions"),
        step("actions round 1", "GET", f"/api/runs/{run0}/actions?round=1"),
        step("agent", "GET", f"/api/runs/{run0}/agents/a01"),
        step("agent by handle", "GET", f"/api/runs/{run0}/agents/@HANDLE"),
        step("agent unknown", "GET", f"/api/runs/{run0}/agents/nobody"),
        step("agent bad id", "GET", f"/api/runs/{run0}/agents/bad!id"),
        step("seeds", "GET", f"/api/runs/{run0}/seeds"),
        step("seed file", "GET", f"/api/runs/{run0}/seeds/Mossbook%20Pricing.md"),
        step("seed text", "GET", f"/api/runs/{run0}/seeds/notes.txt"),
        step("seed hidden", "GET", f"/api/runs/{run0}/seeds/.hidden"),
        step("seed missing", "GET", f"/api/runs/{run0}/seeds/nope.md"),
        step("report html", "GET", f"/api/runs/{run0}/report.html"),
        step("stream", "GET", f"/api/runs/{run0}/stream", stream=True),
        step("event into finished run", "POST", f"/api/runs/{run0}/events", json={"round": 2, "content": "x"}),
        step("event body invalid", "POST", f"/api/runs/{run0}/events", json={"round": "x", "content": ""}),
        step("event body missing", "POST", f"/api/runs/{run0}/events"),
        step("event body not json", "POST", f"/api/runs/{run0}/events", raw="nope",
             content_type="application/json"),
        step("event body a list", "POST", f"/api/runs/{run0}/events", json=[]),
        step("stop finished run", "POST", f"/api/runs/{run0}/stop"),
        step("chat", "POST", f"/api/runs/{run0}/chat", json={"agent_id": "a01", "message": " Why? ",
                                                            "history": [["Hi", "Hello"]]}),
        step("chat bad history", "POST", f"/api/runs/{run0}/chat", json={"agent_id": "a01", "message": "x",
                                                                        "history": [["only one"]]}),
        step("chat unknown agent", "POST", f"/api/runs/{run0}/chat", json={"agent_id": "nobody", "message": "x"}),
        step("chat bad agent id", "POST", f"/api/runs/{run0}/chat", json={"agent_id": "a b", "message": "x"}),
        step("chat invalid body", "POST", f"/api/runs/{run0}/chat", json={"agent_id": "", "message": "",
                                                                         "history": "x"}),
        step("ask", "POST", f"/api/runs/{run0}/ask", json={"question": "Who changed their mind?"}),
        step("ask empty", "POST", f"/api/runs/{run0}/ask", json={"question": ""}),
        step("no rewrite yet", "GET", f"/api/runs/{run0}/rewrite"),
        step("rewrite", "POST", f"/api/runs/{run0}/rewrite"),
        step("saved rewrite", "GET", f"/api/runs/{run0}/rewrite"),
        step("rebuild report", "POST", f"/api/runs/{run0}/report"),
        step("wait for rebuild", "WAIT", "", wait={"rebuild": run0}),
        step("run after rebuild", "GET", f"/api/runs/{run0}"),
        step("bad run id", "GET", "/api/runs/..x"),
        step("bad run id chars", "GET", "/api/runs/a%20b"),
        step("unknown run", "GET", "/api/runs/nope"),
        step("copy example", "COPY", "", copy={"from": run0, "to": "demo"}),
        step("runs with example", "GET", "/api/runs"),
        step("example detail", "GET", "/api/runs/example-demo"),
        step("example event", "POST", "/api/runs/example-demo/events", json={"round": 2, "content": "x"}),
        step("example stop", "POST", "/api/runs/example-demo/stop"),
        step("example rebuild", "POST", "/api/runs/example-demo/report"),
        step("example chat", "POST", "/api/runs/example-demo/chat", json={"agent_id": "a02", "message": "And you?"}),
        step("example rewrite", "POST", "/api/runs/example-demo/rewrite"),
        step("example saved rewrite", "GET", "/api/runs/example-demo/rewrite"),
        step("cast from run", "POST", "/api/runs",
             form=form({**GOOD_RUN, "cast_from": run0, "variant": "Shorter"})),
        step("wait for cast run", "WAIT", "", wait={"run": run1}),
        step("wait for comparison", "WAIT", "", wait={"study": "{study:0}"}),
        step("studies", "GET", "/api/studies"),
        step("study", "GET", "/api/studies/{study:0}"),
        step("compare again", "POST", "/api/studies/{study:0}/compare"),
        step("wait for comparison again", "WAIT", "", wait={"study": "{study:0}"}),
        step("study bad id", "GET", "/api/studies/..x"),
        step("study unknown", "GET", "/api/studies/nope"),
        step("compare unknown", "POST", "/api/studies/nope/compare"),
        step("branch", "POST", f"/api/runs/{run0}/branch",
             json={"round": 1, "events": [{"round": 2, "content": " The owner apologises "}, {"round": 2, "content": " "}]}),
        step("wait for branch", "WAIT", "", wait={"run": "{run:2}"}),
        step("branch too late", "POST", f"/api/runs/{run0}/branch", json={"round": 2}),
        step("branch invalid", "POST", f"/api/runs/{run0}/branch", json={"round": -1, "events": [1, {"round": "a"}]}),
        step("wait for branch study", "WAIT", "", wait={"study": "{study:1}"}),
        step("check", "POST", f"/api/runs/{run0}/check", json={"runs": 2}),
        step("wait for check", "WAIT", "", wait={"study": "{study:2}"}),
        step("check study", "GET", "/api/studies/{study:2}"),
        step("check invalid", "POST", f"/api/runs/{run0}/check", json={"runs": 9, "agent_model": "a b"}),
        step("branch an example", "POST", "/api/runs/example-demo/branch", json={"round": 1}),
        step("wait for example branch", "WAIT", "", wait={"run": "{run:4}"}),
        step("wait for example branch study", "WAIT", "", wait={"study": "{study:3}"}),
        step("all runs", "GET", "/api/runs"),
        step("all studies", "GET", "/api/studies"),
        step("settings", "GET", "/api/settings"),
        step("save settings", "PUT", "/api/settings", json={"agent_model": " sonnet ", "concurrency": 3}),
        step("settings bad runtime", "PUT", "/api/settings", json={"agent_runtime": "Bad Runtime"}),
        step("settings unknown runtime", "PUT", "/api/settings", json={"agent_runtime": "nope"}),
        step("settings bad concurrency", "PUT", "/api/settings", json={"concurrency": 99}),
        step("settings saved", "GET", "/api/settings"),
        step("runtime test", "POST", "/api/runtimes/claude-code/test", json={"model": " haiku "}),
        step("runtime test no body", "POST", "/api/runtimes/claude-code/test"),
        step("runtime test bad id", "POST", "/api/runtimes/BAD/test"),
        step("runtime test unknown", "POST", "/api/runtimes/nope/test"),
        step("runtime test bad model", "POST", "/api/runtimes/claude-code/test", json={"model": "a b"}),
        step("runtimes bad query", "GET", "/api/runtimes?check_auth=5"),
        step("api unknown", "GET", "/api/nope"),
        step("api wrong method", "PUT", "/api/runs"),
        step("api delete", "DELETE", f"/api/runs/{run0}"),
        step("index", "GET", "/", body=False),
        step("deep link", "GET", "/runs/whatever", body=False),
        step("static css", "GET", "/static/css/studio.css", body=False),
        step("static missing", "GET", "/static/nope.js"),
        step("post to page", "POST", "/whatever"),
        step("foreign host", "GET", "/api/health", headers={"host": "evil.example"}),
        step("cross-origin write", "POST", f"/api/runs/{run0}/stop", headers={"origin": "http://evil.example"}),
        step("null origin", "POST", f"/api/runs/{run0}/stop", headers={"origin": "null"}),
        step("same-origin read", "GET", "/api/health", headers={"origin": "http://evil.example"}),
    ]


RUN_NAME = re.compile(r"\d{8}-\d{6}-(.+?)(?:-\d+)?")
STUDY_NAME = re.compile(r"\d{8}-\d{6}-[0-9a-f]{4}")


def masked_run(index: int, name: str) -> str:
    """The run's id without what depends on the clock (its timestamp, and the `-2` suffix a run started in the same
    second gets), so the slug made from the question is still compared."""
    match = RUN_NAME.fullmatch(name)
    return f"{{run:{index}}}-{match.group(1)}" if match else name


def masked_study(index: int, name: str) -> str:
    """The study id, when it has the timestamp-and-random-hex shape: an id of any other shape is left to differ."""
    return f"{{study:{index}}}" if STUDY_NAME.fullmatch(name) else name


def normalise(text: str, root: Path, runs: list[str], studies: list[str]) -> str:
    for folder in sorted({str(root.resolve()), str(root)}, key=len, reverse=True):
        text = text.replace(folder, "<root>")
    names = [(name, masked_run(i, name)) for i, name in enumerate(runs)]
    names += [(name, masked_study(i, name)) for i, name in enumerate(studies)]
    for name, placeholder in sorted(names, key=lambda n: -len(n[0])):
        text = text.replace(name, placeholder)
    text = TIMING.sub('"seconds":0', PID.sub('"pid":0', STAMP.sub("<time>", text)))
    return LLM_TIMING.sub('"llm_seconds":0.0', text)


def new_names(folder: Path, known: list[str]) -> list[str]:
    if not folder.is_dir():
        return []
    return sorted(p.name for p in folder.iterdir() if p.is_dir() and p.name != "studies" and p.name not in known)


def main() -> None:
    from fastapi.testclient import TestClient

    from make import freeze_engine_clock  # this folder is on sys.path when the script runs

    freeze_engine_clock()

    from tellfold.cli import fake_responder
    from tellfold.llm import FakeLLM
    from tellfold.server import create_app

    with tempfile.TemporaryDirectory(prefix="tellfold-api-golden-") as tmp:
        root = Path(tmp)
        runs_dir, examples_dir = root / "runs", root / "examples"
        examples_dir.mkdir()
        os.environ["TELLFOLD_HOME"] = str(root / "home")
        app = create_app(runs_dir, lambda run_dir, concurrency, think=False: FakeLLM(fake_responder), fake_llm=True,
                         poll_interval=0.1, settings_path=root / "settings.json", examples_dir=examples_dir)
        runs: list[str] = []
        studies: list[str] = []
        out = []

        def resolve(text: str) -> str:
            text = re.sub(r"\{run:(\d+)\}", lambda m: runs[int(m.group(1))], text)
            return re.sub(r"\{study:(\d+)\}", lambda m: studies[int(m.group(1))], text)

        with TestClient(app) as client:
            for s in steps():
                if s["method"] == "WAIT":
                    wait(client, {k: resolve(v) for k, v in s["wait"].items()})
                elif s["method"] == "COPY":
                    shutil.copytree(runs_dir / resolve(s["copy"]["from"]), examples_dir / s["copy"]["to"])
                else:
                    if "@HANDLE" in s["path"]:
                        detail = client.get(resolve(s["path"].split("/agents/")[0])).json()
                        s["path"] = s["path"].replace("HANDLE", detail["personas"][1]["handle"].upper())
                    s.update(send(client, s, resolve))
                runs += new_names(runs_dir, runs)
                studies += new_names(runs_dir / "studies", studies)
                out.append(s)
        for s in out:
            if "body" in s and isinstance(s["body"], str):
                s["body"] = normalise(s["body"], root, runs, studies)
                if s["name"] in LISTS:
                    # ids are random, so the server's id order is too; stored in placeholder order (api.rs sorts both)
                    items = sorted(json.loads(s["body"]), key=lambda item: item.get("id", ""))
                    s["body"] = json.dumps(items, ensure_ascii=False, separators=(",", ":"))
            if "headers_out" in s:
                s["headers_out"] = {k: normalise(v, root, runs, studies) for k, v in s["headers_out"].items()}
    OUT.mkdir(parents=True, exist_ok=True)
    session = {"runs": len(runs), "studies": len(studies), "headers": HEADERS, "steps": out}
    (OUT / "session.json").write_text(json.dumps(session, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {OUT / 'session.json'}: {len(out)} steps, {len(runs)} runs, {len(studies)} studies")


# Steps whose body is a list of runs or studies (LISTS in crates/tellfold-server/tests/api.rs).
LISTS = {"runs", "runs with example", "all runs", "studies", "all studies", "no runs yet"}


def send(client: Any, s: dict[str, Any], resolve: Any) -> dict[str, Any]:
    url = resolve(s["path"])
    headers = dict(s.get("headers", {}))
    kwargs: dict[str, Any] = {"headers": headers}
    if "json" in s:
        kwargs["json"] = s["json"]
    elif "raw" in s:
        kwargs["content"] = s["raw"].encode("utf-8")
        headers["content-type"] = s["content_type"]
    elif "form" in s:
        data: dict[str, list[str]] = {}
        for k, v in s["form"]["fields"]:
            data.setdefault(k, []).append(resolve(v))
        kwargs["data"] = data
        kwargs["files"] = [(k, (name, text.encode("utf-8"), "application/octet-stream"))
                           for k, name, text in s["form"]["files"]]
        if not kwargs["files"]:
            # an empty list makes httpx send a urlencoded body; the app must see a multipart form without files
            kwargs["files"] = [("unused", (None, b""))]
    if s.get("stream"):
        text = ""
        with client.stream(s["method"], url, **kwargs) as response:
            for chunk in response.iter_text():
                text += chunk
                if "event: status" in text and text.endswith("\n\n"):
                    break
            return {"status": response.status_code, "content_type": response.headers.get("content-type", ""),
                    "headers_out": pick(response.headers), "body": text}
    response = client.request(s["method"], url, **kwargs)
    body: Any = response.text if s.get("body", True) is not False else len(response.content)
    return {"status": response.status_code, "content_type": response.headers.get("content-type", ""),
            "headers_out": pick(response.headers), "body": body}


def pick(headers: Any) -> dict[str, str]:
    return {k: headers[k] for k in HEADERS if k in headers}


def wait(client: Any, what: dict[str, str]) -> None:
    deadline = time.monotonic() + TIMEOUT
    while time.monotonic() < deadline:
        if "run" in what:
            detail = client.get(f"/api/runs/{what['run']}").json()
            if not detail["live"] and (detail.get("status") or {}).get("state") in ("done", "failed", "stopped"):
                return
        elif "rebuild" in what:
            if not client.get(f"/api/runs/{what['rebuild']}").json()["report_rebuild"]["running"]:
                return
        else:
            study = client.get(f"/api/studies/{what['study']}").json()
            busy = study["repeating"] or study["compare_state"]["running"] or any(
                r["state"] in ("running", "queued") for r in study["runs"])
            if not busy and (study["compare"] or study["agreement"] or study["compare_state"]["error"]):
                return
        time.sleep(0.05)
    sys.exit(f"timed out waiting for {what}")


if __name__ == "__main__":
    main()
