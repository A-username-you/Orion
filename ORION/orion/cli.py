#!/usr/bin/env python3
"""Orion REPL — interactive CLI chat.

Usage:
  orion                       # interactive chat
  orion "question"            # one-shot
  orion --learn TOPIC         # deep-dive + remember a topic
  orion --screen-watch        # chat with live screen monitoring on
"""

from __future__ import annotations

import argparse
import sys

from .assistant import Assistant
from .config import load_config


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="orion", description="Orion — expert AI assistant")
    p.add_argument("question", nargs="*", help="one-shot question (omitted = interactive)")
    p.add_argument("--learn", metavar="TOPIC", help="learn a topic from the internet and remember it")
    p.add_argument("--depth", choices=["quick", "standard", "deep"], default="standard")
    p.add_argument("--screen-watch", action="store_true", help="enable live screen monitoring")
    p.add_argument("--show-trace", action="store_true", help="print tool-call trace after each turn")
    p.add_argument("--stats", action="store_true", help="print memory stats and exit")
    p.add_argument("--reset", action="store_true", help="clear session context (keeps long-term memory)")
    return p


def main(argv=None) -> int:
    args = build_parser().parse_args(argv)
    cfg = load_config()
    if not cfg.api_key:
        print("Orion: no API key found. Set NVIDIA_API_KEY or create ~/.orion/assistant.env",
              file=sys.stderr)
        return 2

    assistant = Assistant(cfg)

    screen = None
    if args.screen_watch:
        from .screen import Screen
        screen = Screen(cfg, assistant.client)
        assistant.screen = screen

    if args.stats:
        print(assistant.memory.stats())
        return 0

    if args.reset:
        assistant.reset()
        print("Session context cleared.")
        return 0

    if args.learn:
        print(f"Orion: learning '{args.learn}' (depth={args.depth})…")
        result = assistant.learn_topic(args.learn, args.depth)
        if result.get("learned"):
            print(f"Learned from {result['pages_read']} pages; stored in memory.")
            print(result["brief"])
        else:
            print(f"Failed: {result.get('reason')}")
        return 0 if result.get("learned") else 1

    if args.question:
        answer = assistant.chat(" ".join(args.question))
        print(answer)
        if args.show_trace and assistant.last_trace:
            print("\n[trace] " + " | ".join(assistant.last_trace), file=sys.stderr)
        return 0

    # interactive
    print("Orion ready. Commands: /reset /stats /trace /learn <topic> /quit")
    while True:
        try:
            line = input("you> ").strip()
        except (EOFError, KeyboardInterrupt):
            print()
            break
        if not line:
            continue
        if line == "/quit":
            break
        if line == "/reset":
            assistant.reset()
            print("orion> context cleared")
            continue
        if line == "/stats":
            print("orion>", assistant.memory.stats())
            continue
        if line == "/trace":
            print("orion>", " | ".join(assistant.last_trace) or "(none)")
            continue
        if line.startswith("/learn "):
            topic = line[len("/learn "):].strip()
            print("orion> learning…")
            r = assistant.learn_topic(topic)
            print("orion>", r.get("brief") or r.get("reason"))
            continue
        if line.startswith("/screen"):
            if screen is None:
                from .screen import Screen
                screen = Screen(cfg, assistant.client)
                assistant.screen = screen
            try:
                state = screen.capture_state(describe=True,
                                             question="Describe what is on screen right now.")
                print("orion>", state.get("description", "(no description)"))
            except Exception as e:
                print("orion> screen error:", e)
            continue
        try:
            answer = assistant.chat(line)
            print("orion>", answer)
            if args.show_trace and assistant.last_trace:
                print("      [trace]", " | ".join(assistant.last_trace))
        except Exception as e:
            print(f"orion> error: {e}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
