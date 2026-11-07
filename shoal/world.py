"""The simulated social platform, persisted in SQLite.

All writes go through `World`; it validates every action so malformed LLM output can never corrupt the
state. One connection, used from a single asyncio thread (the engine awaits LLM calls, then applies
results synchronously).
"""

from __future__ import annotations

import hashlib
import json
import sqlite3
from pathlib import Path

from shoal.models import (
    Action,
    ActionType,
    AgentMemory,
    GodEvent,
    Persona,
    Post,
    RoundStats,
)

_SCHEMA = """
CREATE TABLE IF NOT EXISTS personas (
    id TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS follows (
    follower TEXT NOT NULL,
    followee TEXT NOT NULL,
    PRIMARY KEY (follower, followee)
);
CREATE TABLE IF NOT EXISTS posts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    round INTEGER NOT NULL,
    author_id TEXT NOT NULL,
    content TEXT NOT NULL,
    parent_id INTEGER,
    repost_of INTEGER,
    likes INTEGER NOT NULL DEFAULT 0,
    dislikes INTEGER NOT NULL DEFAULT 0,
    reposts INTEGER NOT NULL DEFAULT 0,
    replies INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS posts_round ON posts(round);
CREATE TABLE IF NOT EXISTS reactions (
    agent_id TEXT NOT NULL,
    post_id INTEGER NOT NULL,
    kind TEXT NOT NULL,
    PRIMARY KEY (agent_id, post_id)
);
CREATE TABLE IF NOT EXISTS actions (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    round INTEGER NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS actions_round ON actions(round);
CREATE TABLE IF NOT EXISTS memories (
    agent_id TEXT PRIMARY KEY,
    notes TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS round_stats (
    round INTEGER PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    round INTEGER NOT NULL,
    content TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS post_sentiment (
    post_id INTEGER PRIMARY KEY,
    score REAL NOT NULL
);
"""

_POST_COLUMNS = "id, round, author_id, content, parent_id, repost_of, likes, dislikes, reposts, replies"
_MAX_CONTENT = 600
_RECENCY_DECAY = 0.7  # feed score multiplier per round of age
_TIER_WINDOW = 2  # replies/followed posts only jump the queue while younger than this many rounds


class World:
    def __init__(self, db_path: Path, *, seed: int = 0):
        """`seed` only perturbs feed tie-breaks so equally ranked posts are not ordered identically for everyone."""
        self._seed = seed
        self._db = sqlite3.connect(str(db_path))
        self._db.row_factory = sqlite3.Row
        self._db.execute("PRAGMA journal_mode=WAL")
        self._db.execute("PRAGMA synchronous=NORMAL")  # WAL + NORMAL survives app crashes, avoids an fsync per action
        self._db.executescript(_SCHEMA)
        self._db.commit()
        self._persona_cache: dict[str, Persona] | None = None

    def close(self) -> None:
        self._db.close()

    # ---------- personas & follow graph ----------

    def add_personas(self, personas: list[Persona]) -> None:
        with self._db:
            self._db.executemany(
                "INSERT OR REPLACE INTO personas (id, data) VALUES (?, ?)",
                [(p.id, p.model_dump_json()) for p in personas],
            )
        self._persona_cache = None

    def personas(self) -> list[Persona]:
        return list(self._personas().values())

    def persona(self, agent_id: str) -> Persona:
        try:
            return self._personas()[agent_id]
        except KeyError:
            raise KeyError(f"unknown agent {agent_id!r}") from None

    def _personas(self) -> dict[str, Persona]:
        if self._persona_cache is None:
            rows = self._db.execute("SELECT data FROM personas ORDER BY id").fetchall()
            self._persona_cache = {p.id: p for p in (Persona.model_validate_json(r["data"]) for r in rows)}
        return self._persona_cache

    def add_follow(self, follower: str, followee: str) -> bool:
        """Create a follow edge without logging an action (used to seed the initial graph)."""
        known = self._personas()
        if follower == followee or follower not in known or followee not in known:
            return False
        with self._db:
            cur = self._db.execute(
                "INSERT OR IGNORE INTO follows (follower, followee) VALUES (?, ?)", (follower, followee)
            )
        return cur.rowcount == 1

    def followers(self, agent_id: str) -> list[str]:
        rows = self._db.execute(
            "SELECT follower FROM follows WHERE followee = ? ORDER BY follower", (agent_id,)
        ).fetchall()
        return [r[0] for r in rows]

    def following(self, agent_id: str) -> list[str]:
        rows = self._db.execute(
            "SELECT followee FROM follows WHERE follower = ? ORDER BY followee", (agent_id,)
        ).fetchall()
        return [r[0] for r in rows]

    def follow_edges(self) -> list[tuple[str, str]]:
        """Every follow edge as (follower, followee), in a stable order."""
        rows = self._db.execute("SELECT follower, followee FROM follows ORDER BY follower, followee").fetchall()
        return [(r[0], r[1]) for r in rows]

    def _follower_counts(self) -> dict[str, int]:
        rows = self._db.execute("SELECT followee, COUNT(*) FROM follows GROUP BY followee").fetchall()
        return {r[0]: r[1] for r in rows}

    # ---------- actions ----------

    def apply(self, round: int, action: Action) -> int | None:
        """Validate and apply one action. Invalid actions are stored as `nothing`. Returns a new post id."""
        with self._db:
            problem = self._validate(action)
            if problem:
                action = _downgrade(action, problem)
            post_id = self._execute(round, action)
            self._db.execute(
                "INSERT INTO actions (round, data) VALUES (?, ?)", (round, action.model_dump_json())
            )
        return post_id

    def _validate(self, a: Action) -> str | None:
        known = self._personas()
        if a.agent_id not in known:
            return "unknown agent"
        content = a.content.strip()
        match a.type:
            case ActionType.POST:
                return None if content else "empty post"
            case ActionType.REPLY:
                return self._check_target(a) or (None if content else "empty reply")
            case ActionType.REPOST:
                problem = self._check_target(a)
                if problem:
                    return problem
                original = self._original(self.post(a.target_post_id))
                return "repost of own post" if original.author_id == a.agent_id else None
            case ActionType.LIKE | ActionType.DISLIKE:
                problem = self._check_target(a)
                if problem:
                    return problem
                if self.post(a.target_post_id).author_id == a.agent_id:
                    return "reaction to own post"
                seen = self._db.execute(
                    "SELECT 1 FROM reactions WHERE agent_id = ? AND post_id = ?",
                    (a.agent_id, a.target_post_id),
                ).fetchone()
                return "already reacted" if seen else None
            case ActionType.FOLLOW:
                target = a.target_agent_id
                if target not in known:
                    return "unknown follow target"
                if target == a.agent_id:
                    return "self-follow"
                if target in self.following(a.agent_id):
                    return "already following"
                return None
            case _:
                return None

    def _check_target(self, a: Action) -> str | None:
        if a.target_post_id is None:
            return "missing target post"
        exists = self._db.execute("SELECT 1 FROM posts WHERE id = ?", (a.target_post_id,)).fetchone()
        return None if exists else "unknown target post"

    def _execute(self, round: int, a: Action) -> int | None:
        content = a.content.strip()[:_MAX_CONTENT]
        match a.type:
            case ActionType.POST:
                return self._insert_post(round, a.agent_id, content)
            case ActionType.REPLY:
                self._bump(a.target_post_id, "replies")
                return self._insert_post(round, a.agent_id, content, parent_id=a.target_post_id)
            case ActionType.REPOST:
                original = self._original(self.post(a.target_post_id))
                self._bump(original.id, "reposts")
                return self._insert_post(round, a.agent_id, content, repost_of=original.id)
            case ActionType.LIKE | ActionType.DISLIKE:
                self._db.execute(
                    "INSERT INTO reactions (agent_id, post_id, kind) VALUES (?, ?, ?)",
                    (a.agent_id, a.target_post_id, a.type.value),
                )
                self._bump(a.target_post_id, "likes" if a.type is ActionType.LIKE else "dislikes")
            case ActionType.FOLLOW:
                self._db.execute(
                    "INSERT INTO follows (follower, followee) VALUES (?, ?)", (a.agent_id, a.target_agent_id)
                )
        return None

    def _insert_post(
        self, round: int, author: str, content: str, *, parent_id: int | None = None, repost_of: int | None = None
    ) -> int:
        cur = self._db.execute(
            "INSERT INTO posts (round, author_id, content, parent_id, repost_of) VALUES (?, ?, ?, ?, ?)",
            (round, author, content, parent_id, repost_of),
        )
        return int(cur.lastrowid)

    def _bump(self, post_id: int, column: str) -> None:
        self._db.execute(f"UPDATE posts SET {column} = {column} + 1 WHERE id = ?", (post_id,))

    def _original(self, post: Post) -> Post:
        """Reposting a repost credits the original post."""
        return self.post(post.repost_of) if post.repost_of is not None else post

    def actions(self, round: int | None = None) -> list[tuple[int, Action]]:
        sql, args = "SELECT round, data FROM actions", ()
        if round is not None:
            sql, args = sql + " WHERE round = ?", (round,)
        rows = self._db.execute(sql + " ORDER BY seq", args).fetchall()
        return [(r["round"], Action.model_validate_json(r["data"])) for r in rows]

    # ---------- posts & feeds ----------

    def post(self, post_id: int) -> Post:
        row = self._db.execute(f"SELECT {_POST_COLUMNS} FROM posts WHERE id = ?", (post_id,)).fetchone()
        if row is None:
            raise KeyError(f"unknown post {post_id}")
        return Post(**dict(row))

    def posts(self, round: int | None = None) -> list[Post]:
        sql, args = f"SELECT {_POST_COLUMNS} FROM posts", ()
        if round is not None:
            sql, args = sql + " WHERE round = ?", (round,)
        return [Post(**dict(r)) for r in self._db.execute(sql + " ORDER BY id", args).fetchall()]

    def feed_for(self, agent_id: str, round: int, k: int) -> list[Post]:
        """Rank visible posts for one agent.

        Posts the agent has not yet engaged with come first; ones it already liked, disliked, replied to or
        reposted sink to the bottom. Among fresh posts (this round or the previous one), replies to the
        agent's own posts come first, then posts by followed accounts, then the rest. Older posts get no tier
        bonus and compete on score alone: engagement x author reach x recency, then a seeded tie-break.
        """
        rows = self._db.execute(
            f"SELECT {', '.join('p.' + c for c in _POST_COLUMNS.split(', '))}, parent.author_id AS parent_author "
            "FROM posts p LEFT JOIN posts parent ON p.parent_id = parent.id "
            "WHERE p.round <= ? AND p.author_id != ?",
            (round, agent_id),
        ).fetchall()
        following = set(self.following(agent_id))
        engaged = self.engagement_by(agent_id)
        reach = self._author_reach()

        def rank(row: sqlite3.Row) -> tuple[bool, int, float, int]:
            post = _row_post(row)
            age = max(0, round - post.round)
            if age >= _TIER_WINDOW:
                tier = 2
            elif row["parent_author"] == agent_id:
                tier = 0
            elif post.author_id in following:
                tier = 1
            else:
                tier = 2
            score = engagement_weight(post) * reach.get(post.author_id, 0.5) * _RECENCY_DECAY**age
            return post.id in engaged, tier, -score, self._tiebreak(agent_id, post.id)

        return [_row_post(r) for r in sorted(rows, key=rank)[:k]]

    def engagement_by(self, agent_id: str) -> dict[int, list[str]]:
        """Post id -> how this agent already engaged with it ("liked", "disliked", "replied", "reposted")."""
        out: dict[int, list[str]] = {}
        for row in self._db.execute(
            "SELECT post_id, kind FROM reactions WHERE agent_id = ? ORDER BY post_id", (agent_id,)
        ):
            out.setdefault(row[0], []).append("liked" if row[1] == ActionType.LIKE.value else "disliked")
        for row in self._db.execute(
            "SELECT parent_id, repost_of FROM posts WHERE author_id = ? "
            "AND (parent_id IS NOT NULL OR repost_of IS NOT NULL) ORDER BY id",
            (agent_id,),
        ):
            for target, label in ((row[0], "replied"), (row[1], "reposted")):
                if target is not None and label not in out.setdefault(target, []):
                    out[target].append(label)
        return out

    def _author_reach(self) -> dict[str, float]:
        """Static influence blended with the share of the population that currently follows the author."""
        personas = self._personas()
        others = max(1, len(personas) - 1)
        counts = self._follower_counts()
        return {pid: 0.5 + p.influence + 0.5 * counts.get(pid, 0) / others for pid, p in personas.items()}

    def _tiebreak(self, agent_id: str, post_id: int) -> int:
        digest = hashlib.blake2b(f"{self._seed}:{agent_id}:{post_id}".encode(), digest_size=8).digest()
        return int.from_bytes(digest, "big")

    # ---------- memory, stats, events ----------

    def save_memory(self, mem: AgentMemory) -> None:
        with self._db:
            self._db.execute(
                "INSERT OR REPLACE INTO memories (agent_id, notes) VALUES (?, ?)",
                (mem.agent_id, json.dumps(mem.notes, ensure_ascii=False)),
            )

    def memory(self, agent_id: str) -> AgentMemory:
        row = self._db.execute("SELECT notes FROM memories WHERE agent_id = ?", (agent_id,)).fetchone()
        return AgentMemory(agent_id=agent_id, notes=json.loads(row["notes"]) if row else [])

    def save_round_stats(self, stats: RoundStats) -> None:
        with self._db:
            self._db.execute(
                "INSERT OR REPLACE INTO round_stats (round, data) VALUES (?, ?)",
                (stats.round, stats.model_dump_json()),
            )

    def round_stats(self) -> list[RoundStats]:
        rows = self._db.execute("SELECT data FROM round_stats ORDER BY round").fetchall()
        return [RoundStats.model_validate_json(r["data"]) for r in rows]

    def save_post_sentiment(self, scores: dict[int, float]) -> None:
        """Store per-post sentiment (-1..1, clamped). Re-scoring a post overwrites its old score."""
        if not scores:
            return
        with self._db:
            self._db.executemany(
                "INSERT OR REPLACE INTO post_sentiment (post_id, score) VALUES (?, ?)",
                [(int(pid), max(-1.0, min(1.0, float(score)))) for pid, score in scores.items()],
            )

    def post_sentiments(self) -> dict[int, float]:
        """Post id -> sentiment score, for every post the engine's sentiment pass has scored."""
        rows = self._db.execute("SELECT post_id, score FROM post_sentiment ORDER BY post_id").fetchall()
        return {r[0]: r[1] for r in rows}

    def add_event(self, event: GodEvent) -> None:
        with self._db:
            self._db.execute("INSERT INTO events (round, content) VALUES (?, ?)", (event.round, event.content))

    def events(self) -> list[GodEvent]:
        rows = self._db.execute("SELECT round, content FROM events ORDER BY round, seq").fetchall()
        return [GodEvent(round=r["round"], content=r["content"]) for r in rows]


def engagement_weight(post: Post) -> float:
    """`likes + 2*reposts + replies - dislikes`, shifted so a fresh post scores 1 and heavy dislikes floor at 0.2."""
    engagement = post.likes + 2 * post.reposts + post.replies - post.dislikes
    return max(0.2, 1.0 + engagement)


def _row_post(row: sqlite3.Row) -> Post:
    return Post(**{c: row[c] for c in _POST_COLUMNS.split(", ")})


def _downgrade(action: Action, problem: str) -> Action:
    note = f"[invalid {action.type.value}: {problem}]"
    return Action(
        agent_id=action.agent_id,
        type=ActionType.NOTHING,
        reasoning=f"{note} {action.reasoning}".strip(),
    )
