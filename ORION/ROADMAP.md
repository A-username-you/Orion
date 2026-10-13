# ORION — Roadmap & Status

> Local-first AI assistant: it sees the screen, hears you, remembers everything, and acts on your machine.
> Stack: Rust daemon + Hyprland/Wayland + whisper.cpp + piper + candle + NVIDIA API. Machine: RTX 4050 6GB, 16 threads, 15GB RAM, dual 1080p monitors.

## 1. What ORION is

A JARVIS-style system assistant that runs as a single daemon on your Linux box:

```
You speak ──▶ whisper (STT) ──▶ Planner (LLM) ──▶ Tools ──▶ Screen/Input/Memory
                                                                    │
You see ◀── HUD/notifications ◀── results ◀── piper (TTS) ◀──────────┘
```

Everything local by default; NVIDIA API only for the brain (planning/vision) and only if you want it.

## 2. Done & verified (working today)

| Component | Status | Proof |
|---|---|---|
| Rust workspace (7 crates) | ✅ compiles clean, 0 errors | `cargo check --workspace` |
| Daemon binary (9.9MB) | ✅ builds + runs indefinitely | smoke test, ran until timeout |
| Message bus (in-process, NATS optional) | ✅ all modules connected via `orion.*` subjects | log lines from all 5 modules |
| Voice capture | ✅ mic live at 16kHz, VAD-gated, wake word "orion" | "Audio capture started" |
| STT | ✅ whisper-cli, CUDA build, ggml-large-v3 (3GB) | `--help` + binary functional |
| TTS | ✅ piper, en_US-lessac voice | `echo ... \| piper` produced /tmp/test.wav |
| Memory DB | ✅ SQLite at ~/.orion/memory.db, schema + indexes | "Database initialized" |
| Embeddings | ✅ all-MiniLM-L6-v2 (384-dim, candle, CPU) | "Embedding model loaded" |
| File watcher | ✅ live-indexes ~/Downloads, ~/Documents, ~/Orion | "File watcher started for 3 paths" |
| Hybrid memory search | ✅ cosine similarity over indexed chunks (JSON-stored) | code path compiles + runs |
| OCR | ✅ tesseract + eng data (installed this session) | extracted text from live screenshot |
| Screenshots | ✅ grim (Wayland), per-monitor & region | /tmp/test_shot.png captured |
| Input injection | ✅ ydotool daemon running; move/click/key/type all verified live | moved mouse, typed "orion test" |
| Window management | ✅ hyprctl IPC available (list/focus) | hyprctl clients -j works |
| NVIDIA API | ✅ key live, models endpoint responds | curl 200 |
| Skills (6) + docs + configs | ✅ orion-{voice,control,memory,planning,automation,security} | skills/ dir |
| One-command run | ✅ `scripts/run.sh` builds, links models, runs | verified end-to-end |

## 3. In flight (written, needs compile/test)

- **orion-control rewrite**: Hyprland + ydotool backend — click, double_click, type, key, hotkey (ctrl+shift+X combos), move_mouse, screenshot (full/region), OCR, window_list, window_focus. On disk at `core/control/src/lib.rs`, **not yet compiled**. Remaining: fix `scroll` (no `ydotool scroll` subcommand — use wheel button codes via click), add `users` crate for UID lookup, `cargo check`, then live-test each action through the bus.
- Note: `press_key`/`hotkey` already fixed to correct `code:1 code:0` syntax.

## 4. To add (priority order)

### Phase A — Close the loop (the "it works" milestone)
1. **Finish control backend** (in flight) — compile, then bus-test every action: screenshot→OCR round trip, click, type, window focus.
2. **orion-vision (new crate)**: `orion.vision.describe` — grim → base64 → NVIDIA VLM (`nemotron-parse` / VLM model) → "what's on screen". Also `orion.vision.find` — "where is the X button?" → returns coordinates for click. This is the "see everything" capability.
3. **Wire the planner brain**: `generate_plan` → NVIDIA chat API (OpenAI-compatible, key already works). Planner gets tools: control, memory.search, vision.describe, tts. Multi-step plans become real.
4. **Voice loop glue**: transcription → planner → execute → TTS speaks the result. First end-to-end JARVIS moment.

### Phase B — Presence ("everywhere I can see")
5. **HUD/overlay**: Hyprland layer-shell overlay (gtk-layer-shell or a small wlr-layer-shell client) showing ORION state, current transcript, plan steps, and a mic level meter — always visible on your monitors.
6. **Waybar module**: status line via `hyprctl`/custom script — memory count, listening state.
7. **Notifications**: `notify-send` (or `hyprctl notify`) for events: file indexed, task done, wake detected.
8. **Global hotkey**: Hyprland bind (e.g. Super+O) → publishes `orion.wake` so you can trigger by key as well as voice.
9. **Dunst/mako hooks** if you prefer existing notification infra over HUD.

### Phase C — System control ("and in this system")
10. **System tools**: shutdown/reboot/lock (`systemctl`, `hyprctl dispatch exit`), brightness (`brightnessctl`), volume (`pactl`), media keys, clipboard (`wl-clipboard` read/write), app launch (`hyprctl exec`).
11. **Hyprland config awareness**: read `~/.config/hypr/hyprland.conf`, let ORION *configure it* — "make the gaps 8px" → edits + `hyprctl reload`.
12. **Automation engine** (crate): cron + file/bus triggers + DAG steps (spec exists in skills/orion-automation). Morning briefing: calendar+email summary → TTS.
13. **Security module** (crate): allowlist per profile, redact API keys/secrets from anything sent to the cloud LLM, audit log. Critical before Phase C tools get destructive.

### Phase D — Memory & scale
14. **sqlite-vec**: real ANN search past ~10k chunks (build from source as loadable extension).
15. **Bigger embedding model**: switch to BGE-M3 (2.3GB) or NV-embed via API once sqlite-vec lands; 384-dim MiniLM is fine for now.
16. **Conversation memory**: index bus transcripts so "what did I ask yesterday?" works.
17. **sqlite FTS5**: keyword + vector hybrid with reciprocal rank fusion.

### Phase E — Multi-device & API
18. **NATS install** (optional today): unlocks cross-process + remote clients. `sudo pacman -S nats-server`.
19. **FastAPI server** (`server/main.py` exists, untested): REST + WebSocket for a phone/web client; screenshots over WS, voice relay.
20. **Mobile client**: React Native app — push-to-talk → STT → bus → results (Phase E, lowest priority).
21. **gRPC bus** (orion-bus, compiles): turn on for typed external clients once needed.

### Phase F — Smarts upgrades
22. **Wake word done right**: openWakeWord or a tiny KWS model instead of whisper-per-second (heavy). Saves constant GPU/CPU burn.
23. **Streaming TTS**: piper already fast (RTF 0.038) — chunk long text and start playing while generating.
24. **Screenshot loop**: periodic grim (configurable, default off) so vision context is always fresh; diff vs last frame to skip idle.
25. **Local LLM option**: wire planner to llama.cpp server (RTX 4050 6GB fits ~7-8B Q4) for offline mode; NVIDIA API as the big brain.

## 5. Known limitations (honest list)

- Wake word currently transcribes ~1s windows through whisper-large = heavy; "orion" string match. Phase F-22 fixes.
- Embeddings JSON-scanned per query — fine ≤10k chunks, then sqlite-vec (Phase D-14).
- candle runs CPU-only on this machine (cudarc rejects CUDA 13.4) — embeddings ~50ms/query at current scale, acceptable.
- BGE-M3 has no safetensors upstream — that's why MiniLM.
- No NATS installed — in-process bus only (fine for single-machine; needed for Phase E).
- Control backend written but unverified — first task next session.
- No tests yet (`cargo test --workspace` passes trivially — nothing to test).

## 6. Config file (optional)

Defaults are baked in. Override at `~/.config/orion/config.toml`:

```toml
[voice]
wake_word = "orion"
sample_rate = 16000

[memory]
db_path = "/home/unk-/.orion/memory.db"
embedding_model = "all-MiniLM-L6-v2"
chunk_size = 512
chunk_overlap = 50

[bus]
nats_url = "nats://127.0.0.1:4222"
grpc_addr = "127.0.0.1:50051"
```

## 7. Bus contract (stable API)

```
orion.wake                    → {"timestamp"}
orion.transcription           → {"text", "confidence", "language", "timestamp"}
orion.tts                     → {"text", "speed", "sample_rate"}      ⇒ orion.tts.response
orion.control.action          → {"action_type": "click|type|key|hotkey|move_mouse|scroll|screenshot|ocr|window_list|window_focus|double_click",
                                 "params": {...}}                    ⇒ orion.control.result
orion.memory.search           → {"text", "limit", "offset"}           ⇒ orion.memory.results
orion.memory.indexed/removed  → file watcher events
orion.planning.plan           → {"goal", "context", "constraints", "available_tools"}
                              ⇒ orion.planning.plan_response
orion.vision.describe         → {"region": optional}                  ⇒ orion.vision.result   (Phase A-2)
```
