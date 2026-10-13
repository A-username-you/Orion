# JARVIS — Full-System AI Assistant
## Project Plan (All Capabilities, All Platforms, Full Autonomy)

---

## 1. Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                        JARVIS CORE                              │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐           │
│  │  Voice   │ │ Computer │ │Automation│ │ Knowledge│           │
│  │ Pipeline │ │ Control  │ │ Engine   │ │  Graph   │           │
│  └────┬─────┘ └────┬─────┘ └────┬─────┘ └────┬─────┘           │
│       │            │            │            │                  │
│       └────────────┴────────────┴────────────┘                  │
│                            │                                     │
│                   ┌────────▼────────┐                            │
│                   │  PLANNING LLM   │  (Nemotron-3-Ultra local) │
│                   │  + Tool Router  │                            │
│                   └────────┬────────┘                            │
│                            │                                     │
│       ┌────────────────────┼────────────────────┐               │
│       ▼                    ▼                    ▼               │
│  ┌─────────┐         ┌─────────┐         ┌─────────┐           │
│  │ Desktop │         │ Server  │         │ Mobile  │           │
│  │  Linux  │◄───────►│  API    │◄───────►│  App    │           │
│  │  (CUDA) │  gRPC   │  (Fast) │  WS/BLE │  (React│           │
│  └─────────┘         └─────────┘         │ Native) │           │
│                                            └─────────┘           │
└─────────────────────────────────────────────────────────────────┘
```

---

## 2. Core Modules (Build Order)

### Phase 1: Foundation (Weeks 1-3)
| Module | Tech | Description |
|--------|------|-------------|
| **Voice Pipeline** | `whisper.cpp` (STT) + `piper`/`kokoro` (TTS) | Local, offline, GPU-accelerated. Wake word → VAD → STT → LLM → TTS |
| **Computer Control** | `computer-use` skill (CUA driver) | Click, type, scroll, screenshot, OCR. X11/Wayland + accessibility tree |
| **Memory Graph** | `sqlite-vec` + `llm-wiki` skill | Entities: files, emails, code, conversations. Vector + keyword search |
| **Planning Core** | Hermes + `hermes-agent` skill | Goal decomposition → tool calls → verification loop |

### Phase 2: Automation & Integration (Weeks 4-6)
| Module | Tech | Description |
|--------|------|-------------|
| **Automation Engine** | `cronjob_manage` + custom DAG runner | Triggers: time, event, file change, webhook. Actions: any tool call |
| **Desktop Integration** | `desktop-ui` skills + DBus | Window management, notifications, clipboard, global hotkeys |
| **Server API** | FastAPI + gRPC | REST for web, gRPC for desktop↔server, WebSocket for real-time |
| **Mobile Bridge** | React Native + BLE/WS | Voice relay, notifications, remote control, camera share |

### Phase 3: Intelligence & Autonomy (Weeks 7-10)
| Module | Tech | Description |
|--------|------|-------------|
| **Skill System** | `hermes-agent-skill-authoring` | Dynamic skill load/unload, sandboxed execution, versioning |
| **Multi-Agent Orchestration** | `delegate_task` pattern | Subagents for coding, research, browsing — coordinated by planner |
| **Self-Improvement Loop** | Eval harness + RLHF-lite | Log outcomes → critique → patch skills/prompts → regression test |
| **Security/OPSEC** | `ai-agent-opsec` + `aip-agent-guard` | Classified term detection, tool allowlists, audit logging |

---

## 3. Tech Stack Decisions

| Layer | Choice | Rationale |
|-------|--------|-----------|
| **LLM (Local)** | Nemotron-3-Ultra (current) → Llama-3.1-70B-Instruct-AWQ | Your GPU, quantized for speed |
| **LLM (Cloud Fallback)** | NVIDIA NIM / OpenRouter | When local context > 32k or needs vision |
| **Embeddings** | NVIDIA Embed (current) → BGE-M3 | Multilingual, dense + sparse |
| **Vector DB** | `sqlite-vec` (local) / Qdrant (server) | Zero-dep local, scalable remote |
| **Message Bus** | NATS (local) + gRPC (inter-process) | Pub/sub + RPC, low latency |
| **Config** | TOML + `hermes-agent` profiles | Per-device, per-user, env-specific |
| **Observability** | OpenTelemetry → Grafana/Loki | Traces, metrics, logs unified |

---

## 4. Directory Structure

```
~/Orion/JARVIS/
├── core/                    # Rust/Go core daemon (single binary)
│   ├── voice/               # whisper.cpp + piper bindings
│   ├── control/             # CUA driver wrapper
│   ├── memory/              # sqlite-vec + graph logic
│   ├── planning/            # LLM router + tool executor
│   └── bus/                 # NATS client + gRPC server
├── skills/                  # Hermes skills (auto-loaded)
│   ├── jarvis-voice/
│   ├── jarvis-control/
│   ├── jarvis-memory/
│   ├── jarvis-automation/
│   └── jarvis-security/
├── server/                  # FastAPI + gRPC
│   ├── api/                 # REST endpoints
│   ├── ws/                  # WebSocket for real-time
│   └── auth/                # mTLS + JWT
├── desktop/                 # Linux desktop app (Tauri/Rust)
│   ├── tray/                # System tray + global hotkeys
│   ├── overlay/             # Voice visualizer, command palette
│   └── accessibility/       # AT-SPI / Wayland protocols
├── mobile/                  # React Native (Expo)
│   ├── voice/               # Audio streaming
│   ├── notifications/       # Push + local
│   └── remote/              # Screen view, file browser
├── tests/                   # E2E + unit + eval harness
│   ├── scenarios/           # YAML test cases
│   └── benchmarks/          # Latency, accuracy, resource
├── config/                  # TOML configs per profile
│   ├── local.desktop.toml
│   ├── local.server.toml
│   └── mobile.toml
├── docker/                  # Compose for server stack
│   ├── docker-compose.yml
│   ├── Dockerfile.core
│   └── Dockerfile.server
└── docs/                    # Architecture, API, skill authoring
```

---

## 5. Milestones & Deliverables

| Milestone | Target | Demo Criteria |
|-----------|--------|---------------|
| **M1: Voice Loop** | Week 2 | "Jarvis, what time is it?" → TTS response < 800ms |
| **M2: Computer Control** | Week 3 | "Open Firefox, go to github.com" → executes + screenshot proof |
| **M3: Memory Recall** | Week 4 | "Find that email about API keys from last week" → returns exact message |
| **M4: Automation** | Week 5 | "Every morning at 8, summarize my calendar and email" → runs autonomously |
| **M5: Server + Mobile** | Week 6 | Phone app shows desktop screen, receives push, relays voice |
| **M6: Multi-Agent** | Week 8 | "Refactor the auth module" → spawns coder subagent → opens PR |
| **M7: Self-Improve** | Week 10 | Failing eval → auto-patches skill → passes regression |

---

## 6. Risk Mitigation

| Risk | Likelihood | Mitigation |
|------|------------|------------|
| Local LLM too slow for real-time | High | Speculative decoding, KV cache reuse, fallback to cloud |
| Wayland accessibility gaps | Medium | X11 fallback, `wlr-foreign-toplevel-management` protocol |
| Skill sandbox escapes | Low | gVisor / firejail per skill, capability-based allowlist |
| Battery drain on mobile | High | BLE for wake, WS only on demand, batch sync |
| Context explosion | Medium | Hierarchical summarization, sliding window + RAG |

---

## 7. Next Steps (This Week)

1. **Prototype Voice Loop** — `whisper.cpp` + `piper` on your GPU, measure latency
2. **Scaffold Core Daemon** — Rust project with NATS + gRPC, plug in `computer-use` skill
3. **Memory MVP** — `sqlite-vec` + one skill (`llm-wiki` pattern) indexing `~/Downloads/my-time-netlify`
4. **Config Profiles** — Set up `hermes-agent` profiles: `desktop`, `server`, `mobile`

---

## 8. Skill Authoring Pattern (Reusable)

Every capability = 1 Hermes skill:
```markdown
# SKILL.md frontmatter
name: jarvis-voice
description: Use when voice I/O needed. Local STT/TTS with wake word.
category: jarvis
triggers: ["voice", "speech", "tts", "stt", "wake word"]
---
# Body: install, config, API, examples, troubleshooting
```

Skills live in `~/Orion/JARVIS/skills/` and auto-load via `hermes-agent` profile config.

---

## 9. Budget Estimate (Local-First)

| Component | Cost |
|-----------|------|
| GPU (you have) | $0 |
| Cloud fallback (NVIDIA NIM) | ~$50/mo for burst |
| Mobile dev (Apple/Google fees) | $125/yr |
| Domain + TLS | $15/yr |
| **Total Year 1** | **~$740** |

---

**Ready to start M1 (Voice Loop)?** I'll scaffold the Rust core + whisper.cpp integration and wire it into your existing Hermes profile.