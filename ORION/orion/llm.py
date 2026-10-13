"""LLM client: OpenAI-compatible chat/vision/embeddings over stdlib urllib.

Zero deps. Handles: streaming disabled (simple), retries on 429/5xx/timeouts,
vision messages with base64 images, embeddings with input_type.
"""

from __future__ import annotations

import base64
import json
import time
import urllib.error
import urllib.request

from .config import Config


class LLMError(RuntimeError):
    pass


class LLMClient:
    def __init__(self, cfg: Config):
        self.cfg = cfg

    # ------------------------------------------------------------------
    def _post(self, path: str, payload: dict, timeout: float | None = None) -> dict:
        url = self.cfg.api_base.rstrip("/") + path
        body = json.dumps(payload).encode()
        timeout = timeout or self.cfg.request_timeout
        last_err = None
        for attempt in range(4):
            req = urllib.request.Request(
                url,
                data=body,
                method="POST",
                headers={
                    "Authorization": "Bearer " + self.cfg.api_key,
                    "Content-Type": "application/json",
                    "Accept": "application/json",
                },
            )
            try:
                with urllib.request.urlopen(req, timeout=timeout) as r:
                    return json.load(r)
            except urllib.error.HTTPError as e:
                if e.code in (429, 500, 502, 503, 504) and attempt < 3:
                    delay = 2 ** attempt
                    time.sleep(delay)
                    continue
                detail = ""
                try:
                    detail = e.read().decode(errors="replace")[:300]
                except Exception:
                    pass
                raise LLMError(f"HTTP {e.code} from {path}: {detail}") from e
            except (urllib.error.URLError, TimeoutError, OSError) as e:
                if attempt < 3:
                    time.sleep(2 ** attempt)
                    continue
                raise LLMError(f"network error calling {path}: {e}") from e
        raise LLMError(f"exhausted retries: {last_err}")

    # ------------------------------------------------------------------
    def chat(self, messages: list[dict], tools: list[dict] | None = None,
             model: str | None = None, temperature: float | None = None,
             max_tokens: int | None = None, thinking: bool | None = None) -> dict:
        payload = {
            "model": model or self.cfg.chat_model,
            "messages": messages,
            "temperature": self.cfg.temperature if temperature is None else temperature,
        }
        if tools:
            payload["tools"] = tools
            payload["tool_choice"] = "auto"
        if max_tokens:
            payload["max_tokens"] = max_tokens
        if thinking is not None:
            # NIM reasoning models (nemotron-3): disable chain-of-thought
            # for deterministic structured-output tasks (e.g. judge evals).
            payload["chat_template_kwargs"] = {"thinking": thinking}
        out = self._post("/chat/completions", payload)
        try:
            msg = out["choices"][0]["message"]
            # normalize: some reasoning models return content: null with the
            # answer in reasoning_content — surface it as content.
            if not msg.get("content") and msg.get("reasoning_content"):
                msg["content"] = msg["reasoning_content"]
            return msg
        except (KeyError, IndexError) as e:
            raise LLMError(f"malformed chat response: {json.dumps(out)[:300]}") from e

    # ------------------------------------------------------------------
    def vision(self, image_path: str, question: str, model: str | None = None,
               max_tokens: int | None = None) -> str:
        with open(image_path, "rb") as f:
            b64 = base64.b64encode(f.read()).decode()
        suffix = image_path.rsplit(".", 1)[-1].lower()
        mime = {"png": "image/png", "jpg": "image/jpeg", "jpeg": "image/jpeg"}.get(suffix, "image/png")
        msg = {
            "role": "user",
            "content": [
                {"type": "text", "text": question},
                {"type": "image_url", "image_url": {"url": f"data:{mime};base64,{b64}"}},
            ],
        }
        out_msg = self.chat([{"role": "system",
                              "content": "You are Orion's visual perception module. "
                                         "Answer precisely and concisely about the given screen image."},
                             msg],
                            model=model or self.cfg.vision_model,
                            temperature=0.0,
                            max_tokens=max_tokens or self.cfg.vision_max_tokens)
        return out_msg.get("content") or ""

    # ------------------------------------------------------------------
    def embed(self, texts: list[str], input_type: str = "query") -> list[list[float]]:
        out = self._post("/embeddings", {
            "model": self.cfg.embed_model,
            "input": texts,
            "input_type": input_type,
            "encoding_format": "float",
        })
        try:
            return [d["embedding"] for d in sorted(out["data"], key=lambda d: d["index"])]
        except (KeyError, IndexError) as e:
            raise LLMError(f"malformed embeddings response: {json.dumps(out)[:300]}") from e
