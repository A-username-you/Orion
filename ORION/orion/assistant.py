"""Orion's brain: persona system prompt, chat loop with tool calling,
sliding-window context + automatic summarization, reasoning traces."""

from __future__ import annotations

import json
import re
import time

from .config import Config
from .llm import LLMClient
from .memory import Memory
from .tools import Tools

SYSTEM_PROMPT = """You are Orion — a general-purpose expert AI assistant running \
locally on the user's machine. You combine deep technical expertise (coding, \
debugging, architecture) with strong knowledge work (analysis, summarization, \
research, writing, planning).

Identity & tone:
- Direct, precise, technically excellent. No filler, no flattery.
- Match response length to the question's weight: one-liners get one-liners.
- When you reason through something non-trivial, show a brief trace of your \
thinking ("Reasoning:" then the key steps) before the final answer.
- Admit uncertainty plainly; never fabricate sources, quotes, or results.

Capabilities:
- You can see and monitor the user's screen (tool: read_screen) and describe \
what's happening on it.
- You can acquire knowledge from the internet (tools: web_search, fetch_page, \
learn) and remember it durably (remember / recall).
- You can execute Python (python_exec) for math, data analysis, verification.
- You have long-term memory that persists across sessions.

Rules:
- Use tools proactively when they beat answering from memory alone: anything \
current, factual-but-uncertain, computational, or on-screen.
- For "learn/master/research X" requests, call learn(topic, depth).
- Keep tool-call arguments minimal and correct JSON.
- NEVER re-implement a tool inside python_exec (e.g. fetching URLs with \
urllib, or searching by hand): use the actual tool. python_exec is for \
computation on data you already have.
- After tool results arrive, stop calling tools and write the final answer \
as soon as you have enough information. Avoid long chains of searches.
- Facts the user asks you to remember go through the remember tool.
- Be safe: refuse clearly harmful requests briefly, offer safe alternatives.
"""


class Assistant:
    """Conversational core: history, tool loop, memory, reasoning traces."""

    def __init__(self, cfg: Config, client: LLMClient | None = None,
                 memory: Memory | None = None, tools: Tools | None = None,
                 screen=None):
        self.cfg = cfg
        self.client = client or LLMClient(cfg)
        self.memory = memory or Memory(cfg, self.client)
        self.tools = tools or Tools(cfg, self.client, self.memory)
        self.screen = screen
        self.history: list[dict] = []        # openai-style messages
        self.summaries: list[str] = []        # rolling session summaries
        self.turn_count = 0
        self.last_trace: list[str] = []
        self._ensure_greeting_checked = False

    # ------------------------------------------------------------------
    def _context_messages(self, user_text: str) -> list[dict]:
        """Assemble prompt: system + summaries + recent window + memory hits."""
        sys_text = SYSTEM_PROMPT
        if self.summaries:
            sys_text += "\n\nSession so far (summarized):\n" + "\n".join(
                f"- {s}" for s in self.summaries[-3:])
        # semantic memory injection
        try:
            hits = self.memory.search(user_text, limit=3)
            if hits:
                sys_text += "\n\nRelevant long-term memory:\n" + "\n".join(
                    f"- [{h.get('via','?')}] {h['text'][:200]}" for h in hits)
        except Exception:
            pass
        messages = [{"role": "system", "content": sys_text}]
        messages.extend(self.history[-self.cfg.history_limit:])
        messages.append({"role": "user", "content": user_text})
        return messages

    # ------------------------------------------------------------------
    def _find_trailing_tool_call(self, text: str) -> tuple[str, dict] | None:
        """Recover tool calls the model emitted as plain text.

        Some models narrate ("Let me fetch that page...") and then write the
        tool-call JSON inline at the end of the content instead of using the
        structured tool_calls field. Detect exactly that shape: a JSON object
        at the very end of the message whose keys match a known tool's
        parameter signature (all required params present, no unknown keys).
        Returns (tool_name, args, json_start_index) or None.
        """
        text = (text or "").rstrip()
        if not text.endswith("}"):
            return None
        obj = None
        obj_start = -1
        # try every '{' as a start; keep the widest span that parses as JSON
        # and reaches the end of the text
        for start in range(len(text)):
            if text[start] != "{":
                continue
            depth, in_str, esc = 0, False, False
            for i in range(start, len(text)):
                c = text[i]
                if in_str:
                    if esc:
                        esc = False
                    elif c == "\\":
                        esc = True
                    elif c == '"':
                        in_str = False
                    continue
                if c == '"':
                    in_str = True
                elif c == "{":
                    depth += 1
                elif c == "}":
                    depth -= 1
                    if depth == 0:
                        if i == len(text) - 1:
                            try:
                                parsed = json.loads(text[start:])
                            except json.JSONDecodeError:
                                parsed = None
                            if isinstance(parsed, dict):
                                obj = parsed
                                obj_start = start
                        break
            if obj is not None:
                break
        if not obj:
            return None
        keys = set(obj.keys())
        for tool in self.cfg.tool_definitions():
            fn = tool["function"]
            params = fn.get("parameters", {})
            props = set(params.get("properties", {}))
            required = set(params.get("required", []))
            if required.issubset(keys) and keys.issubset(props):
                return fn["name"], obj, obj_start
        return None

    # ------------------------------------------------------------------
    def chat(self, user_text: str, show_trace: bool = True) -> str:
        """One conversational turn: tool-loop until final text answer."""
        started = time.time()
        self.last_trace = []
        messages = self._context_messages(user_text)
        tool_defs = self.cfg.tool_definitions()

        for iteration in range(self.cfg.max_tool_iterations):
            msg = self.client.chat(messages, tools=tool_defs)
            tool_calls = msg.get("tool_calls") or []
            if not tool_calls:
                content = msg.get("content") or msg.get("reasoning_content") or ""
                recovered = self._find_trailing_tool_call(content)
                if recovered:
                    # model wrote the tool call as plain text — synthesize the
                    # structured exchange and keep looping
                    name, args, json_start = recovered
                    self.last_trace.append(
                        f"{name}({json.dumps(args)[:120]}) -> recovered inline call")
                    call_id = f"inline_{iteration}"
                    # narration without the trailing JSON block
                    narration = content[:json_start]
                    messages.append({
                        "role": "assistant",
                        "content": narration.strip() or None,
                        "tool_calls": [{
                            "id": call_id,
                            "type": "function",
                            "function": {"name": name,
                                         "arguments": json.dumps(args)},
                        }],
                    })
                    t0 = time.time()
                    result = self.tools.dispatch(name, args)
                    self.last_trace[-1] += f" {time.time() - t0:.2f}s"
                    messages.append({
                        "role": "tool",
                        "tool_call_id": call_id,
                        "name": name,
                        "content": json.dumps(result)[:14000],
                    })
                    continue
                answer = content.strip()
                if not answer and any(m.get("role") == "tool" for m in messages):
                    # model went silent after tool results — force a real answer
                    return self._wrap_up(user_text, messages, iteration, started)
                self._record(user_text, content, messages, iteration, started)
                return answer
            messages.append(msg)
            for tc in tool_calls:
                fn = tc["function"]
                name = fn["name"]
                try:
                    args = json.loads(fn["arguments"] or "{}")
                except json.JSONDecodeError as e:
                    args = {"error": f"model sent invalid JSON args: {e}"}
                t0 = time.time()
                result = self.tools.dispatch(name, args)
                dt = time.time() - t0
                self.last_trace.append(f"{name}({json.dumps(args)[:120]}) -> {dt:.2f}s")
                messages.append({
                    "role": "tool",
                    "tool_call_id": tc["id"],
                    "name": name,
                    "content": json.dumps(result)[:14000],
                })
        # exhausted iterations (or empty answer): force a plain answer
        return self._wrap_up(user_text, messages, iteration, started)

    def _wrap_up(self, user_text: str, messages: list, iteration: int,
                 started: float) -> str:
        """Force a final answer. If the model still returns something empty,
        retry once, then fall back to surfacing the tool findings directly."""
        tool_findings = [m for m in messages if m.get("role") == "tool"]
        for attempt, extra in enumerate((
            "Wrap up now with your best final answer.",
            "You must answer now in plain prose. Do NOT call any tools, do "
            "not narrate. Write the complete final answer directly.",
        )):
            messages.append({"role": "user", "content": extra})
            answer = self.client.chat(messages, tools=None).get("content") or ""
            answer = answer.strip()
            if answer:
                self._record(user_text, answer, messages, iteration, started)
                return answer
        # last resort: present what the tools found
        if tool_findings:
            digest = "\n\n".join(
                f"[{m['name']}] {m['content'][:2000]}" for m in tool_findings[-4:])
            answer = (f"I gathered the following but could not finish a clean "
                      f"synthesis:\n\n{digest}")
        else:
            answer = "I could not complete this request. Please try rephrasing."
        self._record(user_text, answer, messages, iteration, started)
        return answer

    # ------------------------------------------------------------------
    def chat_with_screen(self, user_text: str) -> str:
        """Variant that auto-includes live screen context when available."""
        if self.screen:
            try:
                state = self.screen.capture_state(describe=False)
                screen_ctx = (
                    f"[screen state: changed={state['changed']}, "
                    f"distance={state['distance']}]")
                user_text = user_text + "\n" + screen_ctx
            except Exception:
                pass
        return self.chat(user_text)

    # ------------------------------------------------------------------
    def _record(self, user_text: str, answer: str, messages: list, iterations: int, started: float):
        self.history.append({"role": "user", "content": user_text})
        self.history.append({"role": "assistant", "content": answer})
        self.turn_count += 1
        self.memory.log_turn("user", user_text)
        self.memory.log_turn("assistant", answer)
        if self.turn_count % self.cfg.summary_every == 0:
            self._summarize()

    def _summarize(self):
        """Compress older history so the window stays informative."""
        old = self.history[:-self.cfg.history_limit]
        if len(old) < 4:
            return
        corpus = "\n".join(f"{m['role']}: {m['content'][:400]}" for m in old[-20:])
        try:
            summary = self.client.chat(
                [{"role": "system", "content": "Summarize this conversation segment in <=3 dense bullet lines."},
                 {"role": "user", "content": corpus}],
                temperature=0.0, max_tokens=200,
            ).get("content", "")
            if summary:
                self.summaries.append(summary)
        except Exception:
            pass

    # ------------------------------------------------------------------
    def learn_topic(self, topic: str, depth: str = "standard") -> dict:
        return self.tools.learn(topic, depth)

    def reset(self):
        self.history.clear()
        self.summaries.clear()
        self.turn_count = 0
