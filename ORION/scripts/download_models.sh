#!/bin/bash
# Download ORION models

set -e

MODELS_DIR="${HOME}/.orion/models"
mkdir -p "${MODELS_DIR}"

echo "Downloading models to ${MODELS_DIR}"

# Whisper large-v3 (STT)
echo "Downloading Whisper large-v3..."
wget -nc -P "${MODELS_DIR}" \
  "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin"

# Piper voice (TTS) - en_US lessac medium
echo "Downloading Piper en_US-lessac-medium..."
wget -nc -P "${MODELS_DIR}" \
  "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx"
wget -nc -P "${MODELS_DIR}" \
  "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx.json"

# BGE-M3 (Embeddings)
echo "Downloading BGE-M3..."
wget -nc -P "${MODELS_DIR}/bge-m3" \
  "https://huggingface.co/BAAI/bge-m3/resolve/main/model.safetensors"
wget -nc -P "${MODELS_DIR}/bge-m3" \
  "https://huggingface.co/BAAI/bge-m3/resolve/main/config.json"
wget -nc -P "${MODELS_DIR}/bge-m3" \
  "https://huggingface.co/BAAI/bge-m3/resolve/main/tokenizer.json"

# Kokoro TTS (lighter alternative)
echo "Downloading Kokoro..."
wget -nc -P "${MODELS_DIR}/kokoro" \
  "https://huggingface.co/hexgrad/Kokoro-82M/resolve/main/kokoro-v1.0.onnx"
wget -nc -P "${MODELS_DIR}/kokoro" \
  "https://huggingface.co/hexgrad/Kokoro-82M/resolve/main/voices.json"

echo "All models downloaded!"
echo ""
echo "Models directory:"
ls -lh "${MODELS_DIR}"/*