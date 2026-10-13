#!/bin/bash
# Run ORION core daemon
#
# Usage: ./scripts/run.sh [desktop|server|mobile]
#
# The daemon uses built-in defaults if no config.toml is present.
# NATS is optional - the in-process bus works standalone.

set -e

PROFILE="${1:-desktop}"
MODELS_DIR="${HOME}/.orion/models"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Ensure data + model dirs exist
mkdir -p "${HOME}/.orion" "${MODELS_DIR}"

# Ensure models exist (link from legacy jarvis dir or download)
if [[ ! -e "${MODELS_DIR}/ggml-large-v3.bin" && -e "${HOME}/.jarvis/models/ggml-large-v3.bin" ]]; then
    echo "Linking existing models from ~/.jarvis/models..."
    ln -sfn "${HOME}"/.jarvis/models/* "${MODELS_DIR}/" 2>/dev/null || true
fi

if [[ ! -e "${MODELS_DIR}/all-MiniLM-L6-v2/model.safetensors" ]]; then
    echo "Downloading all-MiniLM-L6-v2 embedding model..."
    mkdir -p "${MODELS_DIR}/all-MiniLM-L6-v2"
    wget -q -P "${MODELS_DIR}/all-MiniLM-L6-v2" \
        https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/model.safetensors \
        https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/config.json \
        https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/tokenizer.json
fi

if [[ ! -e "${MODELS_DIR}/ggml-large-v3.bin" ]]; then
    echo "Downloading Whisper large-v3 (3GB)..."
    wget -q --show-progress -P "${MODELS_DIR}" \
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin"
fi

if [[ ! -e "${MODELS_DIR}/en_US-lessac-medium.onnx" ]]; then
    echo "Downloading Piper voice..."
    wget -q -P "${MODELS_DIR}" \
        "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx" \
        "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx.json"
fi

# Build if binary is missing
BIN="${ROOT}/core/target/release/orion-core"
if [[ ! -x "${BIN}" ]]; then
    echo "Building ORION core (first run, ~2 min)..."
    cd "${ROOT}/core"
    cargo build --release
fi

# Set up environment
export ORION_PROFILE="${PROFILE}"
export RUST_LOG="${RUST_LOG:-info}"
export NATS_URL="${NATS_URL:-nats://localhost:4222}"

# Start NATS if available (optional - in-process bus works without it)
if command -v nats-server >/dev/null 2>&1 && ! pgrep -x "nats-server" > /dev/null; then
    echo "Starting NATS server..."
    nats-server -js -m 8222 &
    sleep 2
fi

# Run ORION core
echo "Starting ORION core (profile: ${PROFILE})..."
echo "  Bus subjects: orion.wake, orion.transcription, orion.tts, orion.memory.*, orion.planning.*"
echo "  Ctrl+C to shut down."
exec "${BIN}"