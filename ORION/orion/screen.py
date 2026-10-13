"""Screen perception: Wayland capture (grim), perceptual-hash change detection,
vision-model description. Monitoring loop keeps a rolling window of screen
states so Orion can answer 'what changed on my screen' questions."""

from __future__ import annotations

import hashlib
import json
import shlex
import subprocess
import time
from collections import deque
from pathlib import Path

from .config import Config
from .llm import LLMClient

try:
    from PIL import Image  # optional, only for resize+ahash
    _HAS_PIL = True
except ImportError:
    _HAS_PIL = False


def _have_ffmpeg() -> bool:
    return subprocess.run(["which", "ffmpeg"], capture_output=True).returncode == 0


class Screen:
    """Capture + perceive the screen. Wayland-native (grim), X11 fallback (import)."""

    def __init__(self, cfg: Config, client: LLMClient, cache_dir: str | None = None):
        self.cfg = cfg
        self.client = client
        self.cache_dir = Path(cache_dir or Path.home() / ".orion" / "screen")
        self.cache_dir.mkdir(parents=True, exist_ok=True)
        self._history: deque[dict] = deque(maxlen=60)   # rolling window of states
        self._last_hash = None
        self._ffmpeg = _have_ffmpeg()

    # ------------------------------------------------------------------ capture
    _ENV_HINTS = ("/run/user/{uid}", "/run/user/{uid}/wayland-0")

    def _capture_env(self) -> dict:
        """Env for the capture command: inherit + ensure WAYLAND_DISPLAY/XDG_RUNTIME_DIR."""
        import os
        env = dict(os.environ)
        uid = os.getuid()
        runtime = f"/run/user/{uid}"
        if Path(runtime).is_dir() and not env.get("XDG_RUNTIME_DIR"):
            env["XDG_RUNTIME_DIR"] = runtime
        if not env.get("WAYLAND_DISPLAY"):
            for sock in ("wayland-1", "wayland-0"):
                if Path(runtime, sock).exists():
                    env["WAYLAND_DISPLAY"] = sock
                    break
        return env

    def capture(self, output: str | None = None) -> Path:
        path = self.cache_dir / f"capture_{int(time.time() * 1000)}.png"
        # argv-form execution (no shell) — the template comes from the local
        # trusted config file, and shlex keeps placeholders out of shell scope.
        template = self.cfg.screen_capture_cmd
        if output and "{output}" in template:
            cmd = shlex.split(template.format(output=output, path=str(path)))
        else:
            cmd = shlex.split(template.format(output="", path=str(path)))
            if output:  # template has no {output} slot: append -o explicitly
                cmd = [c for c in cmd if c] + ["-o", output]
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=20,
                              env=self._capture_env())
        if proc.returncode != 0 or not path.exists():
            # X11 fallback
            try:
                subprocess.run(["import", "-window", "root", str(path)],
                               capture_output=True, timeout=20, check=True,
                               env=self._capture_env())
            except Exception:
                raise RuntimeError(f"screen capture failed: {proc.stderr.strip()[:200]}")
        return path

    def _scaled(self, path: Path, max_edge: int) -> Path:
        """Downscale before sending to vision model (smaller = faster + cheaper)."""
        if self._ffmpeg:
            try:
                out = path.with_suffix(".scaled.jpg")
                subprocess.run(
                    ["ffmpeg", "-y", "-loglevel", "error", "-i", str(path),
                     "-vf", f"scale='if(gt(iw,ih),min({max_edge},iw),-2)':"
                            f"'if(gt(iw,ih),-2,min({max_edge},ih))'",
                     "-q:v", "5", str(out)],
                    capture_output=True, timeout=20, check=True)
                return out
            except Exception:
                pass
        if _HAS_PIL:
            try:
                with Image.open(path) as im:
                    im = im.convert("RGB")
                    w, h = im.size
                    if max(w, h) > max_edge:
                        scale = max_edge / float(max(w, h))
                        im = im.resize((max(1, int(w * scale)), max(1, int(h * scale))))
                    out = path.with_suffix(".scaled.jpg")
                    im.save(out, "JPEG", quality=70)
                    return out
            except Exception:
                pass
        return path

    # ------------------------------------------------------------------ hashing
    def _phash(self, path: Path) -> tuple[str, float]:
        """aHash (average hash) over an 8x8 gray thumbnail. Returns (hex, avg)."""
        # fast path: ffmpeg -> 8x8 raw gray
        if self._ffmpeg:
            try:
                proc = subprocess.run(
                    ["ffmpeg", "-loglevel", "error", "-i", str(path),
                     "-vf", "scale=8:8", "-f", "rawvideo", "-pix_fmt", "gray", "-"],
                    capture_output=True, timeout=15)
                if proc.returncode == 0 and len(proc.stdout) == 64:
                    vals = list(proc.stdout)
                    avg = sum(vals) / 64.0
                    bits = 0
                    for v in vals:
                        bits = (bits << 1) | (1 if v > avg else 0)
                    return f"{bits:016x}", avg
            except Exception:
                pass
        # pure-stdlib path
        from .imaging import png_ahash
        try:
            return png_ahash(path.read_bytes()), 0.5
        except Exception:
            data = hashlib.blake2b(path.read_bytes()[:65536], digest_size=8).hexdigest()
            return data, 0.5

    def capture_state(self, describe: bool = False, question: str | None = None) -> dict:
        """Capture, fingerprint, optionally describe. Detects screen changes."""
        path = self.capture()
        scaled = self._scaled(path, self.cfg.screenshot_scale)
        phash, avg = self._phash(scaled)
        changed = True
        distance = 1.0
        if self._last_hash:
            distance = self._hamming_hex(self._last_hash, phash)
            changed = distance > self.cfg.screen_change_threshold
        state = {
            "ts": time.time(),
            "capture": str(path),
            "scaled": str(scaled),
            "phash": phash,
            "changed": changed,
            "distance": round(distance, 4),
        }
        if describe:
            q = question or "Describe what's on screen: apps, content, any notable UI elements."
            try:
                state["description"] = self.client.vision(str(scaled), q)
            except Exception as e:
                state["description"] = f"(vision failed: {e})"
        self._history.append(state)
        if changed:
            self._last_hash = phash
        return state

    @staticmethod
    def _hamming_hex(a: str, b: str) -> float:
        """Normalized Hamming distance between two hex fingerprints (0..1)."""
        try:
            ia, ib = int(a, 16), int(b, 16)
        except ValueError:
            return 1.0
        bits = max(len(a), len(b)) * 4
        if bits == 0:
            return 0.0
        return bin(ia ^ ib).count("1") / bits

    # ------------------------------------------------------------------ monitor
    def monitor_loop(self, on_change=None, interval: float | None = None, max_ticks: int = 0):
        """Watch the screen; call on_change(state) when it changes. Blocks."""
        interval = interval or self.cfg.screen_monitor_interval
        ticks = 0
        while True:
            try:
                state = self.capture_state(describe=bool(on_change))
                if state["changed"] and on_change:
                    on_change(state)
            except Exception as e:
                if on_change:
                    on_change({"error": str(e), "ts": time.time()})
            ticks += 1
            if max_ticks and ticks >= max_ticks:
                return
            time.sleep(interval)

    def recent_history(self, n: int = 10) -> list[dict]:
        return list(self._history)[-n:]

    def describe_history(self) -> str:
        """Compact text summary of recent screen states for the chat context."""
        h = self.recent_history(8)
        if not h:
            return "No screen history yet."
        lines = []
        for s in h:
            t = time.strftime("%H:%M:%S", time.localtime(s["ts"]))
            desc = s.get("description") or ("(unchanged)" if not s.get("changed", True) else "(changed, not described)")
            lines.append(f"[{t}] {desc[:220]}")
        return "\n".join(lines)


class ScreenMonitor:
    """Background screen watcher feeding a knowledge buffer for the assistant."""

    def __init__(self, screen: Screen):
        self.screen = screen
        self._running = False
        self._thread = None

    def start(self):
        import threading
        if self._running:
            return
        self._running = True

        def _run():
            self.screen.monitor_loop(on_change=self._on_change,
                                     interval=self.screen.cfg.screen_monitor_interval,
                                     max_ticks=0)

        self._thread = threading.Thread(target=_run, daemon=True, name="orion-screen-monitor")
        self._thread.start()

    def stop(self):
        self._running = False

    def _on_change(self, state: dict):
        # buffer descriptions; the chat loop pulls them via recent_history()
        pass
