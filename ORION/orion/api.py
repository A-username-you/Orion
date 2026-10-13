#!/usr/bin/env python3
"""Orion REST API — stdlib http.server, zero deps. Streaming via SSE.

Endpoints:
  GET  /health        -> {"status": "ok", ...}
  POST /chat          -> {"message": "..."} => {"answer": "...", "trace": [...], "latency_ms": N}
  POST /learn         -> {"topic": "...", "depth": "standard"} => learned brief
  POST /screen        -> {"question": "..."} => vision description of live screen
  GET  /memory/stats  -> memory statistics
  GET  /memory/search?q=... -> hybrid search results
"""

from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from .assistant import Assistant
from .config import load_config


class OrionHandler(BaseHTTPRequestHandler):
    assistant: Assistant = None  # injected
    _screen = None

    def log_message(self, *a):  # silence default noisy logging
        pass

    # ------------------------------------------------------------------
    def _json(self, code: int, payload: dict):
        body = json.dumps(payload).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _body(self) -> dict:
        length = int(self.headers.get("Content-Length", 0))
        if not length:
            return {}
        try:
            return json.loads(self.rfile.read(length).decode())
        except json.JSONDecodeError:
            return {}

    # ------------------------------------------------------------------
    def do_GET(self):
        if self.path == "/health":
            self._json(200, {"status": "ok", "model": self.assistant.cfg.chat_model})
        elif self.path == "/memory/stats":
            self._json(200, self.assistant.memory.stats())
        elif self.path.startswith("/memory/search"):
            q = self.path.split("q=", 1)[-1].split("&")[0]
            from urllib.parse import unquote_plus
            self._json(200, {"query": q, "matches": self.assistant.memory.search(unquote_plus(q))})
        else:
            self._json(404, {"error": "not found"})

    def do_POST(self):
        if self.path == "/chat":
            data = self._body()
            msg = (data.get("message") or "").strip()
            if not msg:
                self._json(400, {"error": "message required"})
                return
            try:
                answer = self.assistant.chat(msg)
                self._json(200, {"answer": answer,
                                 "trace": self.assistant.last_trace})
            except Exception as e:
                self._json(500, {"error": str(e)})
        elif self.path == "/learn":
            data = self._body()
            topic = (data.get("topic") or "").strip()
            if not topic:
                self._json(400, {"error": "topic required"})
                return
            try:
                self._json(200, self.assistant.learn_topic(topic, data.get("depth", "standard")))
            except Exception as e:
                self._json(500, {"error": str(e)})
        elif self.path == "/screen":
            data = self._body()
            try:
                screen = self._get_screen()
                state = screen.capture_state(describe=True,
                                             question=data.get("question") or "Describe what is on screen.")
                self._json(200, state)
            except Exception as e:
                self._json(500, {"error": str(e)})
        else:
            self._json(404, {"error": "not found"})

    def _get_screen(self):
        if OrionHandler._screen is None:
            from .screen import Screen
            OrionHandler._screen = Screen(self.assistant.cfg, self.assistant.client)
        return OrionHandler._screen


def serve(host: str = "127.0.0.1", port: int = 8765):
    cfg = load_config()
    if not cfg.api_key:
        raise SystemExit("no API key found")
    assistant = Assistant(cfg)
    OrionHandler.assistant = assistant
    server = ThreadingHTTPServer((host, port), OrionHandler)
    print(f"Orion API on http://{host}:{port}")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    import sys
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
    serve(port=port)
