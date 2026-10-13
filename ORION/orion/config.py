"""Configuration: env-first, TOML fallback, sane defaults. Model swap = one env var."""

from __future__ import annotations

import os
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

DEFAULT_CONFIG_PATHS = [
    "~/.config/orion/assistant.toml",
    "~/.orion/assistant.toml",
]

DEFAULTS = {
    "chat_model": "nvidia/nemotron-3-super-120b-a12b",
    "vision_model": "meta/llama-3.2-11b-vision-instruct",
    "embed_model": "nvidia/nemotron-3-embed-1b",
    "api_base": "https://integrate.api.nvidia.com/v1",
    "api_key_env": "NVIDIA_API_KEY",
    "env_file": "~/.orion/assistant.env",
    "db_path": "~/.orion/assistant.db",
    "history_limit": 24,          # sliding window of recent turns kept in prompt
    "summary_every": 12,          # turns before the window gets summarized
    "max_search_results": 6,
    "screen_monitor_interval": 5.0,   # seconds between screen checks
    "screen_change_threshold": 0.08,  # perceptual-hash distance considered "changed"
    "screen_capture_cmd": "grim '{path}'",  # Wayland (Hyprland); -o appended if output given
    "screenshot_scale": 1024,      # long-edge px before sending to vision model
    "vision_max_tokens": 400,
    "request_timeout": 120.0,
    "max_tool_iterations": 8,      # tool-call loop safety bound
    "temperature": 0.2,
}


@dataclass
class Config:
    chat_model: str
    vision_model: str
    embed_model: str
    api_base: str
    db_path: Path
    history_limit: int
    summary_every: int
    max_search_results: int
    screen_monitor_interval: float
    screen_change_threshold: float
    screen_capture_cmd: str
    screenshot_scale: int
    vision_max_tokens: int
    request_timeout: float
    max_tool_iterations: int
    temperature: float
    api_key: str = field(repr=False, default="")

    def tool_definitions(self) -> list[dict]:
        """OpenAI-style tool schema published to the model on every request."""
        return [
            {
                "type": "function",
                "function": {
                    "name": "python_exec",
                    "description": "Execute Python code in a sandboxed subprocess "
                    "(stdlib only) and return stdout/stderr. Use for math, data "
                    "analysis, file processing, anything computable.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "code": {"type": "string", "description": "Python source to run."}
                        },
                        "required": ["code"],
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "web_search",
                    "description": "Search the web (DuckDuckGo). Returns titled links. "
                    "Use for current events or facts you are unsure about.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": {"type": "string"},
                            "max_results": {"type": "integer", "default": 6},
                        },
                        "required": ["query"],
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "fetch_page",
                    "description": "Fetch a web page as readable text (HTML -> text, "
                    "keeps https URLs, strips scripts). For reading search results.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "url": {"type": "string"},
                            "max_chars": {"type": "integer", "default": 12000},
                        },
                        "required": ["url"],
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "read_screen",
                    "description": "Capture the screen now and describe it with the vision "
                    "model. Use when the user references what's on screen or asks you to "
                    "find/verify something visually.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "question": {
                                "type": "string",
                                "description": "What to look for / describe.",
                                "default": "Describe this screen concisely.",
                            }
                        },
                        "required": [],
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "read_file",
                    "description": "Read a text file from the local filesystem.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {"type": "string"},
                            "max_chars": {"type": "integer", "default": 20000},
                        },
                        "required": ["path"],
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "write_file",
                    "description": "Write a text file to the local filesystem (creates parents).",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {"type": "string"},
                            "content": {"type": "string"},
                        },
                        "required": ["path", "content"],
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "remember",
                    "description": "Store a durable fact in long-term memory for future sessions. "
                    "The memory is retrieved automatically when related topics come up.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "text": {"type": "string"},
                            "topic": {"type": "string", "default": "general"},
                        },
                        "required": ["text"],
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "recall",
                    "description": "Search long-term memory (facts you stored + indexed files). "
                    "Returns best matches with similarity scores.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": {"type": "string"},
                            "limit": {"type": "integer", "default": 5},
                        },
                        "required": ["query"],
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "learn",
                    "description": "Deep-dive a topic on the internet: multi-query search, fetch "
                    "top pages, distill findings, and store the synthesis in long-term memory. "
                    "Use when the user asks to 'learn/master/research' a topic.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "topic": {"type": "string"},
                            "depth": {"type": "string",
                                      "enum": ["quick", "standard", "deep"],
                                      "default": "standard"},
                        },
                        "required": ["topic"],
                    },
                },
            },
        ]


def load_config(overrides: dict | None = None) -> Config:
    """Resolve config: DEFAULTS < TOML file < environment < explicit overrides."""
    values = dict(DEFAULTS)

    # 1. TOML file
    for candidate in DEFAULT_CONFIG_PATHS:
        p = Path(os.path.expanduser(candidate))
        if p.exists():
            with p.open("rb") as f:
                data = tomllib.load(f)
            # accept both flat and sectioned layouts
            for section in (data.get("assistant", {}), data.get("orion", {}), data):
                if isinstance(section, dict):
                    for k in DEFAULTS:
                        if k in section:
                            values[k] = section[k]
            break

    # 2. environment (ORION_* overrides everything file-based)
    for k in DEFAULTS:
        env_val = os.environ.get("ORION_" + k.upper())
        if env_val is not None and env_val != "":
            values[k] = env_val

    # 3. explicit overrides
    if overrides:
        values.update({k: v for k, v in overrides.items() if v is not None})

    # resolve API key: env var direct > env file
    api_key = os.environ.get(values["api_key_env"], "")
    env_file = Path(os.path.expanduser(values["env_file"]))
    if not api_key and env_file.exists():
        for line in env_file.read_text().splitlines():
            if line.strip().startswith("#"):
                continue
            if "=" in line:
                k, v = line.split("=", 1)
                if k.strip() == values["api_key_env"]:
                    api_key = v.strip()
                    break
    values["api_key"] = api_key

    values["db_path"] = Path(os.path.expanduser(values["db_path"]))
    return Config(
        chat_model=values["chat_model"],
        vision_model=values["vision_model"],
        embed_model=values["embed_model"],
        api_base=values["api_base"],
        api_key=values["api_key"],
        db_path=values["db_path"],
        history_limit=int(values["history_limit"]),
        summary_every=int(values["summary_every"]),
        max_search_results=int(values["max_search_results"]),
        screen_monitor_interval=float(values["screen_monitor_interval"],
                                      ) if not isinstance(values["screen_monitor_interval"], (int, float)) else float(values["screen_monitor_interval"]),
        screen_change_threshold=float(values["screen_change_threshold"]),
        screen_capture_cmd=str(values["screen_capture_cmd"]),
        screenshot_scale=int(values["screenshot_scale"]),
        vision_max_tokens=int(values["vision_max_tokens"]),
        request_timeout=float(values["request_timeout"]),
        max_tool_iterations=int(values["max_tool_iterations"]),
        temperature=float(values["temperature"]),
    )
