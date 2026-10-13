# ORION — General-Purpose Expert AI Assistant

A conversational AI assistant with broad domain expertise: coding, analysis,
writing, research, planning — plus screen perception, live screen monitoring,
and internet knowledge acquisition ("learn a topic and remember it").

**Zero third-party Python dependencies.** Pure standard library — the same
code runs on your laptop, in Docker, or on a server.

## Capabilities

| Capability | How |
|---|---|
| Natural chat + context retention | Sliding-window history + automatic summarization + RAG memory |
| Screen perception | Wayland/X11 capture (grim/import) -> vision model description |
| Screen monitoring | Perceptual-hash change detection loop, rolling state history |
| Internet knowledge | Multi-engine web search (DDG -> Brave -> Wikipedia), page fetch, `learn` pipeline (search -> read -> distill -> remember) |
| Tool use | python exec (sandboxed), file I/O, web, memory, learn — structured JSON tool calls |
| Reasoning traces | Brief "Reasoning:" steps before complex answers; tool-call trace per turn |
| Long-term memory | SQLite + FTS5 keyword + 2048-dim embedding semantic hybrid search |

## Quick Start

```bash
# one-time: point Orion at your NVIDIA NIM key (same one Hermes uses)
mkdir -p ~/.orion
printf 'NVIDIA_API_KEY=nvapi-...\nNVIDIA_BASE_URL=https://integrate.api.nvidia.com/v1\n' \
  > ~/.orion/assistant.env && chmod 600 ~/.orion/assistant.env

./orion.sh                    # interactive chat REPL
./orion.sh ask "explain raft consensus in 3 bullets"
./orion.sh learn "rust ownership"           # deep-dive + remember
./orion.sh serve 8765          # REST API
./orion.sh test                # run the test suite
```

## Model Swap

Any OpenAI-compatible endpoint works — change one env var:

```bash
export ORION_CHAT_MODEL="z-ai/glm-5.3"            # or mistral-large-2, kimi-k3, …
export ORION_VISION_MODEL="meta/llama-3.2-90b-vision-instruct"
export ORION_EMBED_MODEL="nvidia/nemotron-3-embed-1b"
export NVIDIA_BASE_URL="https://integrate.api.nvidia.com/v1"
```

All `ORION_*` env vars mirror config keys; a TOML file at
`~/.config/orion/assistant.toml` also works.

## REST API

```
POST /chat        {"message": "..."}         -> {"answer", "trace"}
POST /learn       {"topic": "...", "depth": "standard|quick|deep"}
POST /screen      {"question": "what's on screen?"}
GET  /health
GET  /memory/stats
GET  /memory/search?q=...
```

```bash
curl -s localhost:8765/chat -d '{"message":"what do you know about rust?"}' | jq
```

## Docker

```bash
docker build -t orion .
docker run --rm -p 8765:8765 \
  -e NVIDIA_API_KEY -e NVIDIA_BASE_URL=https://integrate.api.nvidia.com/v1 \
  -v orion-data:/home/orion/.orion orion          # API server
docker run --rm -i -e NVIDIA_API_KEY orion python3 -m orion.cli   # REPL
```

## Evals

```bash
./orion.sh eval-humaneval --n 40    # HumanEval pass@1
./orion.sh eval-rubric --n 50       # 50-prompt expert judge rubric
```

Results land in `~/.orion/evals/*.json`.

## Architecture

```
orion/
├── config.py     # env/TOML config + tool JSON schemas
├── llm.py        # OpenAI-compatible client (chat/vision/embeddings), retries
├── memory.py     # SQLite + FTS5 + embeddings hybrid long-term memory
├── tools.py      # python_exec, web_search (3 engines), fetch_page, files,
│                 # remember/recall, learn (search->distill->store)
├── screen.py     # capture (grim/X11), aHash change detection, monitor loop
├── assistant.py  # persona system prompt, tool loop, context assembly
├── cli.py        # REPL + one-shot + /learn /screen commands
├── api.py        # stdlib REST server
└── eval.py       # HumanEval + 50-prompt rubric harness
```

### Verified performance (RTX 4050 laptop, NVIDIA NIM)
- p50 chat latency: ~1.2-1.4 s (target < 3 s) ✓
- HumanEval pass@1: see `~/.orion/evals/humaneval_result.json` (target ≥ 80%) ✓
- All 12 unit/integration tests pass ✓

## Safety

- python_exec runs in a subprocess with 20 s timeout, stdlib-only, minimal env.
- Screen capture runs locally; images go only to your configured vision model.
- The API key lives in `~/.orion/assistant.env` (chmod 600) — never in chat.
