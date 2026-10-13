"""Tool implementations: python exec, web search, page fetch, file I/O, memory, learn."""

from __future__ import annotations

import json
import re
import subprocess
import sys
import urllib.parse
import urllib.request
from html.parser import HTMLParser
from pathlib import Path

from .config import Config
from .llm import LLMClient
from .memory import Memory


class _TextExtractor(HTMLParser):
    """Strip scripts/styles/tags -> readable text with rough structure kept."""

    SKIP = {"script", "style", "noscript", "svg", "iframe", "head"}

    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.out: list[str] = []
        self._skip = 0
        self._pre = 0

    def handle_starttag(self, tag, attrs):
        if tag in self.SKIP:
            self._skip += 1
        if tag in ("p", "div", "br", "li", "tr", "h1", "h2", "h3", "h4", "pre", "code"):
            self.out.append("\n")
        if tag == "pre":
            self._pre += 1

    def handle_endtag(self, tag):
        if tag in self.SKIP and self._skip > 0:
            self._skip -= 1
        if tag == "pre" and self._pre > 0:
            self._pre -= 1

    def handle_data(self, data):
        if self._skip == 0:
            self.out.append(data)

    def text(self) -> str:
        raw = "".join(self.out)
        raw = re.sub(r"[ \t]+", " ", raw)
        raw = re.sub(r"\n{3,}", "\n\n", raw)
        return raw.strip()


def html_to_text(html: str) -> str:
    p = _TextExtractor()
    try:
        p.feed(html)
    except Exception:
        pass
    return p.text()


def html_escape_unescape(s: str) -> str:
    """& < " &#39; -> chars (html.unescape without the import cost pattern)."""
    import html as _html
    return _html.unescape(s).strip()


class Tools:
    def __init__(self, cfg: Config, client: LLMClient, memory: Memory):
        self.cfg = cfg
        self.client = client
        self.memory = memory

    # ------------------------------------------------------------------ python
    def python_exec(self, code: str) -> dict:
        """Run Python in a subprocess sandbox: stdlib-only, 20s timeout, 64KB out."""
        try:
            proc = subprocess.run(
                [sys.executable, "-c", code],
                capture_output=True,
                text=True,
                timeout=20,
                env={"PATH": "/usr/bin:/bin", "HOME": str(Path.home()), "PYTHONHASHSEED": "0"},
            )
            out = (proc.stdout or "")[:32000]
            err = (proc.stderr or "")[:32000]
            return {"stdout": out, "stderr": err, "exit_code": proc.returncode}
        except subprocess.TimeoutExpired:
            return {"stdout": "", "stderr": "TIMEOUT after 20s", "exit_code": -1}
        except Exception as e:
            return {"stdout": "", "stderr": f"sandbox error: {e}", "exit_code": -1}

    # ------------------------------------------------------------------ search
    _UA = ("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 "
           "(KHTML, like Gecko) Chrome/125.0 Safari/537.36")

    def _http_get(self, url: str, timeout: int = 15, accept: str = "text/html") -> str:
        req = urllib.request.Request(url, headers={
            "User-Agent": self._UA,
            "Accept": accept,
            "Accept-Language": "en-US,en;q=0.9",
        })
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.read(2_000_000).decode(errors="replace")

    def web_search(self, query: str, max_results: int | None = None) -> dict:
        """Multi-engine cascade: DDG lite -> Brave HTML -> Wikipedia API."""
        max_results = max_results or self.cfg.max_search_results
        for engine, fn in (("ddg", self._search_ddg),
                           ("brave", self._search_brave),
                           ("wikipedia", self._search_wikipedia)):
            try:
                results = fn(query, max_results)
            except Exception:
                results = []
            if results:
                return {"query": query, "engine": engine, "results": results[:max_results]}
        return {"query": query, "engine": "none", "results": [],
                "note": "all engines exhausted/blocked — try again later"}

    def _search_ddg(self, query: str, max_results: int) -> list[dict]:
        url = "https://html.duckduckgo.com/lite/?q=" + urllib.parse.quote_plus(query)
        html = self._http_get(url)
        results = []
        # DDG lite anchors: href/class order varies; uddg= holds the real URL
        pattern = re.compile(
            r"<a[^>]*?class='result-link'[^>]*?href=\"([^\"]+)\"[^>]*>(.*?)</a>"
            r"|<a[^>]*?href=\"([^\"]+)\"[^>]*?class='result-link'[^>]*>(.*?)</a>", re.S)
        for g in pattern.findall(html)[:max_results * 2]:
            href, title = (g[0], g[1]) if g[0] else (g[2], g[3])
            m = re.search(r"uddg=([^&]+)", href)
            real = urllib.parse.unquote(m.group(1)) if m else href
            title = re.sub(r"<[^>]+>", "", title).strip()
            if real.startswith("//"):
                real = "https:" + real
            if title and real.startswith("http"):
                results.append({"title": title, "url": real})
            if len(results) >= max_results:
                break
        return results

    def _search_brave(self, query: str, max_results: int) -> list[dict]:
        url = "https://search.brave.com/search?q=" + urllib.parse.quote_plus(query)
        html = self._http_get(url, timeout=20)
        results = []
        # Brave SERP: results are <a href=URL>…<div class="title …" title="TITLE">
        block_pat = re.compile(
            r'<a[^>]+href="(https?://(?!search\.brave|brave\.com|cdn\.search\.brave)[^"]+)"'
            r'[^>]*>(?:(?!</a>).)*?class="title[^"]*"[^>]*title="([^"]+)"', re.S)
        for url_, title in block_pat.findall(html)[:max_results * 2]:
            title = html_escape_unescape(title)
            if title and url_.startswith("http"):
                results.append({"title": title, "url": url_})
            if len(results) >= max_results:
                break
        return results

    def _search_wikipedia(self, query: str, max_results: int) -> list[dict]:
        url = ("https://en.wikipedia.org/w/api.php?action=query&list=search&format=json"
               "&srlimit=" + str(max_results) + "&srsearch=" + urllib.parse.quote_plus(query))
        data = json.loads(self._http_get(url, accept="application/json"))
        out = []
        for r in data.get("query", {}).get("search", []):
            out.append({"title": r["title"] + " — Wikipedia",
                        "url": "https://en.wikipedia.org/wiki/" + urllib.parse.quote(
                            r["title"].replace(" ", "_"))})
        return out

    # ------------------------------------------------------------------ fetch
    def fetch_page(self, url: str, max_chars: int = 12000) -> dict:
        req = urllib.request.Request(url, headers={
            "User-Agent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0 Safari/537.36",
            "Accept": "text/html,application/xhtml+xml",
        })
        try:
            with urllib.request.urlopen(req, timeout=20) as r:
                ctype = r.headers.get("Content-Type", "")
                raw = r.read(2_000_000).decode(errors="replace")
            if "html" in ctype.lower() or re.match(r"\s*<(!doctype|html)", raw, re.I):
                text = html_to_text(raw)
            else:
                text = raw
            title = ""
            m = re.search(r"<title[^>]*>(.*?)</title>", raw, re.S | re.I)
            if m:
                title = re.sub(r"\s+", " ", m.group(1)).strip()[:120]
            return {"url": url, "title": title, "text": text[:max_chars],
                    "truncated": len(text) > max_chars}
        except Exception as e:
            return {"url": url, "error": f"fetch failed: {e}"}

    # ------------------------------------------------------------------ files
    def read_file(self, path: str, max_chars: int = 20000) -> dict:
        try:
            p = Path(path).expanduser().resolve()
            content = p.read_text(errors="replace")
            return {"path": str(p), "content": content[:max_chars],
                    "truncated": len(content) > max_chars}
        except Exception as e:
            return {"path": path, "error": str(e)}

    def write_file(self, path: str, content: str) -> dict:
        try:
            p = Path(path).expanduser()
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(content)
            return {"path": str(p), "bytes": len(content), "ok": True}
        except Exception as e:
            return {"path": path, "error": str(e)}

    # ------------------------------------------------------------------ memory
    def remember(self, text: str, topic: str = "general") -> dict:
        fid = self.memory.add_fact(text, topic)
        return {"ok": True, "fact_id": fid, "note": f"stored under topic '{topic}'"}

    def recall(self, query: str, limit: int = 5) -> dict:
        return {"query": query, "matches": self.memory.search(query, limit)}

    # ------------------------------------------------------------------ learn
    def learn(self, topic: str, depth: str = "standard") -> dict:
        """Internet knowledge acquisition: search -> fetch -> distill -> remember."""
        n_pages = {"quick": 2, "standard": 4, "deep": 6}[depth]
        queries = [f"{topic} overview", f"{topic} key concepts explained", f"what is {topic}"]
        if depth == "deep":
            queries.append(f"{topic} advanced topics deep dive")
        seen_urls: set[str] = set()
        pages: list[dict] = []
        for q in queries:
            sr = self.web_search(q)
            for r in sr.get("results", []):
                if len(pages) >= n_pages:
                    break
                if r["url"] in seen_urls:
                    continue
                seen_urls.add(r["url"])
                page = self.fetch_page(r["url"], max_chars=9000)
                if page.get("text") and len(page["text"]) > 300 and not page.get("error"):
                    pages.append(page)
            if len(pages) >= n_pages:
                break
        if not pages:
            return {"topic": topic, "learned": False, "reason": "no usable pages found"}
        # distill into a compact knowledge brief
        corpus = "\n\n".join(
            f"PAGE: {p['title']}\n{p['text']}" for p in pages)
        distill_prompt = (
            "Synthesize the following web sources into a structured knowledge brief "
            f"about '{topic}'. Sections: Definition, Key Concepts, Important Details, "
            "Further Reading (urls). Be factual, dense, max ~400 words. Sources:\n\n" + corpus[:30000]
        )
        try:
            brief = self.client.chat(
                [{"role": "system", "content": "You are Orion's knowledge distillation module. "
                                                "Produce precise, dense, factual briefs."},
                 {"role": "user", "content": distill_prompt}],
                temperature=0.1, max_tokens=900,
            ).get("content", "")
        except Exception as e:
            brief = f"(distillation failed: {e})\n\n" + corpus[:4000]
        cid = self.memory.add_knowledge(f"learned:{topic}", topic, brief)
        return {
            "topic": topic, "learned": True, "pages_read": len(pages),
            "sources": [p.get("url") for p in pages],
            "brief": brief[:6000], "chunk_id": cid,
        }

    # ------------------------------------------------------------------ dispatch
    def dispatch(self, name: str, args: dict) -> dict:
        fn = getattr(self, name, None)
        if not fn or not callable(fn):
            return {"error": f"unknown tool '{name}'"}
        try:
            return fn(**args)
        except TypeError as e:
            return {"error": f"bad arguments for {name}: {e}"}
        except Exception as e:
            return {"error": f"{name} failed: {e}"}
