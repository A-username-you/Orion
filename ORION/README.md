# ORION — Full-System AI Assistant

A local-first, autonomous AI assistant with voice, computer control, memory, planning, and automation — built in Rust, running entirely on your machine.

**Status: ✅ Working** — the core daemon builds, runs, and all 5 modules initialize.

## Architecture

```
ORION Core (Rust daemon, ~10MB binary)
├── orion-types      → shared types + in-process message bus (NATS optional)
├── orion-voice      → whisper.cpp STT + piper TTS (via vendor binaries)
├── orion-control    → computer control bus API (CUA backend stub)
├── orion-memory     → SQLite + all-MiniLM-L6-v2 embeddings + file watcher
├── orion-planning   → LLM router + dependency-ordered tool executor
└── orion-bus        → NATS bridge + gRPC server (optional network layer)
```

### Verified working
- ✅ Daemon boots, all modules start, runs indefinitely (Ctrl+C to stop)
- ✅ Audio capture from default mic (16kHz, VAD-gated, wake word "orion")
- ✅ SQLite memory DB with schema + indexes at `~/.orion/memory.db`
- ✅ Embedding model loads via candle (CPU or CUDA)
- ✅ File watcher indexes `~/Downloads`, `~/Documents`, `~/Orion` in real time
- ✅ whisper-cli (CUDA build) + piper TTS both functional

## Quick Start

```bash
cd ~/Orion/ORION
./scripts/run.sh          # builds if needed, links/downloads models, runs daemon
```

Models land in `~/.orion/models/`:

| Model | Size | Purpose |
|-------|------|---------|
| ggml-large-v3.bin | 3GB | Speech-to-text (whisper.cpp, CUDA) |
| en_US-lessac-medium.onnx | 63MB | Text-to-speech (piper) |
| all-MiniLM-L6-v2/ | 88MB | Embeddings (candle, CPU/GPU) |

If you previously ran this project as "JARVIS", existing models in `~/.jarvis/models/` are symlinked automatically.

## Message Bus API

All modules communicate over the bus (in-process by default, NATS if `nats-server` is running):

```
orion.wake                     → wake word detected
orion.transcription            → STT result (after wake word + silence)
orion.tts                      → request TTS {"text": "...", "speed": 1.0, "sample_rate": 16000}
orion.tts.response             → synthesized audio
orion.control.action           → {"action_type": "click", "params": {...}}
orion.control.result           → action result
orion.control.screenshot       → periodic screenshots
orion.memory.search            → {"text": "query", "limit": 10, "offset": 0}
orion.memory.results           → search results with cosine similarity scores
orion.memory.indexed/removed   → file watcher events
orion.planning.plan             → {"goal": "...", "available_tools": [...]}
orion.planning.plan_response   → plan steps with dependencies
orion.automation.job.*         → job management (spec'd, not yet implemented)
orion.security.check           → pre-flight security check (spec'd, not yet implemented)
```

## Skills (Hermes Integration)

Each capability is a Hermes skill — auto-loaded by profile:

| Skill | Description |
|-------|-------------|
| `orion-voice` | Local STT/TTS with wake word |
| `orion-control` | Desktop automation (click, type, screenshot, OCR) |
| `orion-memory` | Vector + keyword search, file watching |
| `orion-planning` | Goal decomposition, tool orchestration |
| `orion-automation` | Cron, triggers, DAG workflows |
| `orion-security` | OPSEC, classified detection, sandboxing |

## Configuration

Defaults are built in — **no config file needed**. To customize, create `~/.config/orion/config.toml`:

```toml
[memory]
db_path = "/home/unk-/.orion/memory.db"
embedding_model = "all-MiniLM-L6-v2"
chunk_size = 512
chunk_overlap = 50

[voice]
wake_word = "orion"
sample_rate = 16000

[bus]
nats_url = "nats://127.0.0.1:4222"
grpc_addr = "127.0.0.1:50051"
```

## Development

```bash
cd core
cargo check --workspace       # fast verification
cargo build --release          # full build (~2 min)
cargo test --workspace
cargo fmt --all
cargo clippy --workspace
```

Binary: `core/target/release/orion-core`

## Project Structure

```
ORION/
├── core/                    # Rust workspace
│   ├── types/               # shared types + in-process bus
│   ├── voice/               # STT/TTS via whisper.cpp + piper binaries
│   ├── control/             # computer control bus API
│   ├── memory/              # SQLite + embeddings + file watcher
│   ├── planning/            # LLM router + tool executor
│   ├── bus/                 # NATS bridge + gRPC
│   └── src/main.rs          # daemon entry
├── vendor/                  # whisper.cpp (built w/ CUDA), piper (built)
├── skills/                  # orion-{voice,control,memory,planning,automation,security}
├── config/                  # desktop/server/mobile profiles
├── docker/                  # NATS, Grafana, Loki, Tempo, Prometheus stack
├── scripts/                 # run.sh (one-command start), build.sh
├── server/                  # FastAPI REST/WebSocket API (planned)
├── desktop/ mobile/         # future clients
└── docs/                    # architecture docs
```

## Hardware Requirements

- **GPU**: optional — CUDA 13.4 works with whisper.cpp; embeddings fall back to CPU
- **RAM**: 4GB+ (8GB+ with GPU embedding models)
- **Storage**: ~4GB for models
- **OS**: Linux (tested on Arch w/ Rust 1.98, CMake 4.4)

## Known Limitations / Next Steps

- **Control engine** — bus API works; X11/Wayland click/type backend is a stub (returns "not available")
- **Planning LLM** — generates fallback plans; wire to Nemotron via NVIDIA API or local llama.cpp
- **sqlite-vec** — embeddings stored as JSON; swap in sqlite-vec for real ANN search at scale
- **Wake word** — currently transcribes 1s buffers through whisper and string-matches "orion" (works but heavy); a KWS model (openWakeWord) would be lighter
- **Automation engine** — spec'd in `skills/orion-automation`, not yet a crate
- **BGE-M3** — repo has no safetensors; switched to all-MiniLM-L6-v2 (384-dim). For BGE-M3, convert `pytorch_model.bin` or use the ONNX build

## License

MIT — Build your own ORION.
