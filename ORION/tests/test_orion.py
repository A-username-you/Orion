"""Orion test suite — real end-to-end tests against live NVIDIA NIM services.

Run: python -m unittest discover -s tests -v   (from ORION dir)
Requires ~/.orion/assistant.env (NVIDIA_API_KEY) — same key Hermes uses.
"""

import json
import os
import sys
import time
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from orion.assistant import Assistant              # noqa: E402
from orion.config import load_config              # noqa: E402
from orion.memory import Memory                    # noqa: E402
from orion.tools import Tools, html_to_text       # noqa: E402


CFG = load_config()
HAS_KEY = bool(CFG.api_key) and not os.environ.get("ORION_SKIP_LIVE")


class TestConfig(unittest.TestCase):
    def test_defaults(self):
        self.assertIn("nemotron", CFG.chat_model)
        self.assertEqual(CFG.api_base, "https://integrate.api.nvidia.com/v1")

    def test_tool_definitions_shape(self):
        defs = CFG.tool_definitions()
        names = [d["function"]["name"] for d in defs]
        self.assertIn("web_search", names)
        self.assertIn("learn", names)
        for d in defs:
            self.assertEqual(d["type"], "function")
            self.assertTrue(d["function"]["description"])


class TestHTMLToText(unittest.TestCase):
    def test_strips_script_and_tags(self):
        html = "<html><head><script>var x=1;</script><style>.a{}</style></head>" \
               "<body><h1>Title</h1><p>Hello <b>world</b></p></body></html>"
        text = html_to_text(html)
        self.assertIn("Hello world", text)
        self.assertIn("Title", text)
        self.assertNotIn("var x", text)
        self.assertNotIn(".a{}", text)


class TestInlineToolCallRecovery(unittest.TestCase):
    """Regression: model writes tool-call JSON as plain text -> must be
    detected, executed, and the loop continued instead of ending the turn."""

    def _make(self, replies):
        from orion.assistant import Assistant

        class FakeClient:
            def __init__(self):
                self.replies = list(replies)
                self.calls = 0

            def chat(self, messages, tools=None, **kw):
                self.calls += 1
                # once scripted replies run out, model "goes silent" (None)
                return self.replies.pop(0) if self.replies else {"content": None}

        a = Assistant(CFG, client=FakeClient())
        # stub tools so recovery tests are hermetic (no network / subprocess)
        a.tools = type("StubTools", (), {
            "dispatch": staticmethod(lambda name, args: {"stub": True, "name": name})})()
        return a

    def test_recovers_trailing_json_tool_call(self):
        a = self._make([
            {"content": "Let me compute that.\n{\n  \"code\": \"print(6*7)\"\n}"},
            {"content": "The answer is 42."},
        ])
        ans = a.chat("what is 6*7?")
        self.assertEqual(ans, "The answer is 42.")
        self.assertTrue(any("python_exec" in t for t in a.last_trace),
                        a.last_trace)
        self.assertIn("recovered inline call", a.last_trace[0])

    def test_no_recovery_for_non_tool_json(self):
        a = self._make([
            {"content": 'The config is {"name": "orion", "version": 3}.'},
        ])
        ans = a.chat("what config?")
        self.assertIn("orion", ans)

    def test_no_recovery_for_partial_required(self):
        # fetch_page requires url; a JSON missing it must NOT be treated as a call
        a = self._make([
            {"content": 'Hmm.\n{"max_chars": 8000}'},
        ])
        ans = a.chat("fetch something")
        self.assertIn("max_chars", ans)

    def test_recovers_multiline_and_nested(self):
        a = self._make([
            {"content": 'I will search now.\n{\n  "query": "test query",\n  "max_results": 3\n}'},
            {"content": "Search done, answer follows."},
        ])
        ans = a.chat("search test query")
        self.assertEqual(ans, "Search done, answer follows.")
        self.assertTrue(any("web_search" in t for t in a.last_trace))

    def test_wrapup_on_silent_answer_after_tools(self):
        # model returns empty content after a tool call -> _wrap_up must fire
        a = self._make([
            {"tool_calls": [{"id": "c1", "type": "function",
                             "function": {"name": "web_search",
                                          "arguments": '{"query": "x"}'}}]},
            {"content": "   "},                      # silent after tools
            {"content": "Recovered final answer."},   # wrap-up retry
        ])
        ans = a.chat("look something up")
        self.assertEqual(ans, "Recovered final answer.")

    def test_wrapup_falls_back_to_tool_digest(self):
        # every wrap-up attempt returns empty -> surface tool findings
        a = self._make([
            {"tool_calls": [{"id": "c1", "type": "function",
                             "function": {"name": "web_search",
                                          "arguments": '{"query": "x"}'}}]},
            {"content": ""}, {"content": None},
        ])
        ans = a.chat("look something up")
        self.assertIn("web_search", ans)
        self.assertIn("could not finish", ans)

    def test_wrapup_on_iteration_exhaustion(self):
        # model keeps calling tools until the loop bound -> wrap-up answer
        tool_reply = {"tool_calls": [{"id": "c", "type": "function",
                                      "function": {"name": "web_search",
                                                   "arguments": '{"query": "x"}'}}]}
        replies = [tool_reply] * 8 + [{"content": "Final synthesis."}]
        a = self._make(replies)
        ans = a.chat("exhaust the loop")
        self.assertEqual(ans, "Final synthesis.")


class TestToolsOffline(unittest.TestCase):
    def test_python_exec_math(self):
        tools = Tools(CFG, None, Memory(CFG, None))
        r = tools.python_exec("print(6*7)")
        self.assertEqual(r["exit_code"], 0)
        self.assertIn("42", r["stdout"])

    def test_python_exec_timeout(self):
        tools = Tools(CFG, None, Memory(CFG, None))
        t0 = time.time()
        r = tools.python_exec("import time; time.sleep(30)")
        self.assertLess(time.time() - t0, 30)
        self.assertEqual(r["exit_code"], -1)
        self.assertIn("TIMEOUT", r["stderr"])

    def test_file_roundtrip(self):
        tools = Tools(CFG, None, Memory(CFG, None))
        path = os.path.expanduser("~/.orion/test_roundtrip.txt")
        w = tools.write_file(path, "orion test line")
        self.assertTrue(w.get("ok"))
        r = tools.read_file(path)
        self.assertIn("orion test line", r["content"])
        os.unlink(path)


@unittest.skipUnless(HAS_KEY, "no NVIDIA_API_KEY")
class TestLive(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.assistant = Assistant(CFG)

    def test_chat_basic(self):
        ans = self.assistant.chat("Reply with exactly the word: PONG")
        self.assertIn("PONG", ans.upper())

    def test_chat_tool_python(self):
        ans = self.assistant.chat(
            "What is 17*23? Use the python_exec tool, then answer with just the number.")
        self.assertIn("391", ans)

    def test_chat_tool_search(self):
        ans = self.assistant.chat(
            "Search the web: what is the current latest stable Linux kernel version? "
            "Use web_search, then answer briefly.")
        self.assertTrue(len(ans) >= 3, ans)
        self.assertTrue(self.assistant.last_trace, "expected tool calls in trace")

    def test_memory_remember_recall(self):
        self.assistant.chat("Remember this fact: the project codename is BELT-7. "
                            "Use the remember tool.")
        hits = self.assistant.memory.search("project codename")
        self.assertTrue(any("BELT-7" in h["text"] for h in hits), hits)

    def test_learn_quick(self):
        r = self.assistant.learn_topic("Rust ownership model", "quick")
        self.assertTrue(r.get("learned"), r)
        self.assertGreaterEqual(r.get("pages_read", 0), 1)

    def test_latency_p50(self):
        # 5 quick turns; p50 must be < 3s per acceptance criteria (API time only)
        lat = []
        for q in ["hi", "what is 2+2 (answer only)", "name one color",
                  "yes or no: is water wet?", "give me a one-word greeting"]:
            t0 = time.time()
            self.assistant.chat(q)
            lat.append(time.time() - t0)
        lat.sort()
        p50 = lat[len(lat) // 2]
        print(f"\nlatencies={[round(x,2) for x in lat]} p50={p50:.2f}s")
        self.assertLess(p50, 3.0, f"p50 latency {p50:.2f}s exceeds 3s")


if __name__ == "__main__":
    unittest.main(verbosity=2)
