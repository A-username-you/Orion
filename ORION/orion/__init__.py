"""Orion — a general-purpose expert AI assistant.

Local-first conversational assistant with:
- natural-language chat with context retention (sliding window + RAG memory)
- tool use (python exec, file I/O, web search, screen perception) via structured JSON calls
- reasoning traces for complex answers
- screen monitoring (Wayland capture -> vision model)
- internet knowledge acquisition ("learn" pipeline: search -> fetch -> distill -> remember)

Zero third-party Python dependencies: pure standard library.
"""

__version__ = "0.1.0"

from .config import Config, load_config  # noqa: F401
