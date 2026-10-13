"""Long-term memory: SQLite + FTS5 keyword search + NVIDIA embeddings for semantic recall.

Three stores in one DB:
- facts        (explicit `remember` tool, chat summaries)
- files        (indexed local documents)
- chunks       (learned knowledge from the `learn` pipeline)

Vector search is exact cosine over stored embeddings (2048-dim). The DB stays
portable; embeddings computed lazily on write.
"""

from __future__ import annotations

import json
import math
import re
import sqlite3
import threading
import time
from pathlib import Path

from .config import Config
from .llm import LLMClient

SCHEMA = """
CREATE TABLE IF NOT EXISTS facts (
    id INTEGER PRIMARY KEY,
    topic TEXT NOT NULL DEFAULT 'general',
    text TEXT NOT NULL,
    embedding TEXT,
    created_at REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_facts_topic ON facts(topic);
CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts USING fts5(text, topic UNINDEXED, content='facts', content_rowid='id');
CREATE TRIGGER IF NOT EXISTS facts_ai AFTER INSERT ON facts BEGIN
    INSERT INTO facts_fts(rowid, text, topic) VALUES (new.id, new.text, new.topic);
END;
CREATE TRIGGER IF NOT EXISTS facts_ad AFTER DELETE ON facts BEGIN
    INSERT INTO facts_fts(facts_fts, rowid, text, topic) VALUES ('delete', old.id, old.text, old.topic);
END;

CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY,
    path TEXT UNIQUE NOT NULL,
    mtime REAL,
    size INTEGER,
    indexed_at REAL
);

CREATE TABLE IF NOT EXISTS chunks (
    id INTEGER PRIMARY KEY,
    source TEXT NOT NULL,          -- url or 'fact:<id>' or 'file:<path>'
    title TEXT,
    text TEXT NOT NULL,
    embedding TEXT,
    created_at REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_chunks_source ON chunks(source);
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(text, title UNINDEXED, content='chunks', content_rowid='id');
CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
    INSERT INTO chunks_fts(rowid, text, title) VALUES (new.id, new.text, new.title);
END;
CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
    INSERT INTO chunks_fts(chunks_fts, rowid, text, title) VALUES ('delete', old.id, new_title(), new.text);
END;

CREATE TABLE IF NOT EXISTS conversations (
    id INTEGER PRIMARY KEY,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    ts REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_conv_ts ON conversations(ts);
"""


def _cosine(a: list[float], b: list[float]) -> float:
    dot = sum(x * y for x, y in zip(a, b))
    na = math.sqrt(sum(x * x for x in a))
    nb = math.sqrt(sum(x * x for x in b))
    if na == 0 or nb == 0:
        return 0.0
    return dot / (na * nb)


class Memory:
    def __init__(self, cfg: Config, client: LLMClient | None = None):
        self.cfg = cfg
        self.client = client
        self.db_path = Path(cfg.db_path)
        self.db_path.parent.mkdir(parents=True, exist_ok=True)
        # check_same_thread=False + explicit lock: the HTTP API server is
        # threaded, and sqlite3 connections are otherwise thread-bound.
        self._lock = threading.Lock()
        self.conn = sqlite3.connect(self.db_path, check_same_thread=False)
        self.conn.row_factory = sqlite3.Row
        with self._lock:
            self.conn.executescript(SCHEMA)
            self.conn.commit()

    # ------------------------------------------------------------------
    def add_fact(self, text: str, topic: str = "general", embed: bool = True) -> int:
        with self._lock:
            cur = self.conn.execute(
                "INSERT INTO facts (topic, text, embedding, created_at) VALUES (?,?,?,?)",
                (topic, text, self._embed_json(text) if embed else None, time.time()),
            )
            self.conn.commit()
            return cur.lastrowid

    def add_knowledge(self, source: str, title: str, text: str, embed: bool = True) -> int:
        with self._lock:
            cur = self.conn.execute(
                "INSERT INTO chunks (source, title, text, embedding, created_at) VALUES (?,?,?,?,?)",
                (source, title, text, self._embed_json(text) if embed else None, time.time()),
            )
            self.conn.commit()
            return cur.lastrowid

    def index_file(self, path: str) -> int:
        """Index a text file's content into chunks. Returns chunk count."""
        p = Path(path).expanduser()
        text = p.read_text(errors="replace")
        chunks = _chunk_text(text)
        with self._lock:
            self.conn.execute(
                "INSERT OR REPLACE INTO files (path, mtime, size, indexed_at) VALUES (?,?,?,?)",
                (str(p), p.stat().st_mtime, p.stat().st_size, time.time()),
            )
            for chunk in chunks:
                self.conn.execute(
                    "INSERT INTO chunks (source, title, text, embedding, created_at) VALUES (?,?,?,?,?)",
                    (f"file:{p}", p.name, chunk,
                     self._embed_json(chunk) if self.client else None, time.time()),
                )
            self.conn.commit()
        return len(chunks)

    def log_turn(self, role: str, content: str):
        with self._lock:
            self.conn.execute("INSERT INTO conversations (role, content, ts) VALUES (?,?,?)",
                              (role, content, time.time()))
            self.conn.commit()

    # ------------------------------------------------------------------
    def search(self, query: str, limit: int = 5) -> list[dict]:
        """Hybrid search: FTS5 keyword + cosine similarity, merged and re-ranked."""
        results: dict = {}
        with self._lock:
            # keyword
            try:
                clean = " ".join(f'"{w}"' for w in re.findall(r"\w+", query) if len(w) > 1)
                if clean:
                    for row in self.conn.execute(
                        "SELECT rowid, text FROM facts_fts WHERE facts_fts MATCH ? LIMIT ?",
                        (clean, limit),
                    ).fetchall():
                        results[row["rowid"]] = {
                            "kind": "fact", "id": row["rowid"], "text": row["text"],
                            "score": 1.0, "via": "keyword",
                        }
            except sqlite3.OperationalError:
                pass
            # semantic
            qvec = self._maybe_embed(query) if self.client else None
            if qvec:
                for row in self.conn.execute(
                    "SELECT id, topic, text, embedding FROM facts WHERE embedding IS NOT NULL"
                ).fetchall():
                    sim = _cosine(qvec, json.loads(row["embedding"]))
                    if sim > 0.35:
                        prev = results.get(row["id"])
                        if prev:
                            prev["score"] = max(prev["score"], sim)
                            prev["via"] = "hybrid"
                        else:
                            results[row["id"]] = {
                                "kind": "fact", "id": row["id"], "text": row["text"],
                                "score": sim, "via": "semantic", "topic": row["topic"],
                            }
                for row in self.conn.execute(
                    "SELECT id, source, title, text, embedding FROM chunks WHERE embedding IS NOT NULL"
                ).fetchall():
                    sim = _cosine(qvec, json.loads(row["embedding"]))
                    if sim > 0.35:
                        if sim > results.get(("chunk", row["id"]), {}).get("score", 0):
                            results[("chunk", row["id"])] = {
                                "kind": "knowledge", "id": row["id"], "text": row["text"],
                                "title": row["title"], "source": row["source"],
                                "score": sim, "via": "semantic",
                            }
            # keyword on chunks too
            try:
                clean = " ".join(f'"{w}"' for w in re.findall(r"\w+", query) if len(w) > 1)
                if clean:
                    for row in self.conn.execute(
                        "SELECT rowid, text, title FROM chunks_fts WHERE chunks_fts MATCH ? LIMIT ?",
                        (clean, limit),
                    ).fetchall():
                        key = ("chunk", row["rowid"])
                        if key not in results:
                            results[key] = {
                                "kind": "knowledge", "id": row["rowid"], "text": row["text"],
                                "title": row["title"], "score": 0.7, "via": "keyword",
                            }
            except sqlite3.OperationalError:
                pass
        ranked = sorted(results.values(), key=lambda r: r["score"], reverse=True)[:limit]
        return ranked

    def stats(self) -> dict:
        with self._lock:
            facts = self.conn.execute("SELECT COUNT(*) c FROM facts").fetchone()["c"]
            chunks = self.conn.execute("SELECT COUNT(*) c FROM chunks").fetchone()["c"]
            files = self.conn.execute("SELECT COUNT(*) c FROM files").fetchone()["c"]
            turns = self.conn.execute("SELECT COUNT(*) c FROM conversations").fetchone()["c"]
        return {"facts": facts, "knowledge_chunks": chunks, "files": files, "turns": turns}

    # ------------------------------------------------------------------
    def _maybe_embed(self, text: str) -> list[float] | None:
        if not self.client:
            return None
        try:
            return self.client.embed([text[:4000]], input_type="query")[0]
        except Exception:
            return None

    def _embed_json(self, text: str) -> str | None:
        vec = self._maybe_embed(text)
        return json.dumps(vec) if vec else None


def _chunk_text(text: str, size: int = 1200, overlap: int = 120) -> list[str]:
    text = text.strip()
    if not text:
        return []
    if len(text) <= size:
        return [text]
    chunks = []
    start = 0
    while start < len(text):
        end = min(start + size, len(text))
        if end < len(text):
            cut = text.rfind(" ", start + size - 200, end)
            if cut > start:
                end = cut
        chunks.append(text[start:end].strip())
        start = end - overlap if end < len(text) else end
    return [c for c in chunks if c]
