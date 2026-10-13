# SKILL.md
name: orion-voice
description: Use when voice I/O needed. Local STT/TTS with wake word detection via ORION core.
category: orion
triggers: ["voice", "speech", "tts", "stt", "wake word", "piper", "whisper"]

---

## Overview

ORION Voice provides fully local, offline speech-to-text and text-to-speech using:
- **STT**: whisper.cpp (GPU-accelerated via CUDA)
- **TTS**: piper / kokoro (neural TTS, multiple voices)
- **VAD**: Silero VAD for voice activity detection
- **Wake Word**: Custom keyword spotting

All processing runs locally on your GPU — no cloud calls.

---

## Installation

```bash
# Build ORION core (includes voice)
cd /home/unk-/Orion/ORION/core
cargo build --release --workspace

# Download models
mkdir -p ~/.orion/models
# Whisper large-v3
wget -P ~/.orion/models https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin
# Piper voice (en_US)
wget -P ~/.orion/models https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx
wget -P ~/.orion/models https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx.json
```

---

## Configuration

Edit `~/.config/orion/config.toml`:

```toml
[voice]
wake_word = "orion"
stt_model = "whisper-large-v3"
tts_model = "piper-en-us"
sample_rate = 16000
vad_threshold = 0.5
device = "cuda"  # or "cpu"
```

---

## API (via Message Bus)

### Publish: `orion.tts`
Request TTS synthesis:
```json
{
  "text": "Hello, how can I help?",
  "voice": "en_US-lessac-medium",
  "speed": 1.0,
  "sample_rate": 16000
}
```

### Subscribe: `orion.tts.response`
Receive synthesized audio:
```json
{
  "audio": { "samples": [...], "sample_rate": 16000, "channels": 1, "timestamp_ms": 1234567890 },
  "phonemes": ["h", "ə", "l", "oʊ"]
}
```

### Subscribe: `orion.transcription`
Receive STT results (after wake word):
```json
{
  "text": "What time is it?",
  "confidence": 0.95,
  "language": "en",
  "timestamp": 1234567890
}
```

### Subscribe: `orion.wake`
Wake word detected:
```json
{ "timestamp": 1234567890 }
```

---

## Usage Examples

### From Hermes (Python)
```python
import asyncio
import async_nats

async def speak(text):
    nc = await async_nats.connect("nats://localhost:4222")
    await nc.publish("orion.tts", json.dumps({
        "text": text,
        "speed": 1.0
    }).encode())
    await nc.close()

async def listen():
    nc = await async_nats.connect("nats://localhost:4222")
    sub = await nc.subscribe("orion.transcription")
    async for msg in sub.messages:
        data = json.loads(msg.data)
        print(f"User said: {data['text']}")
    await nc.close()
```

### From Rust
```rust
use orion_core::*;
use async_nats::ConnectOptions;

let nc = ConnectOptions::new().connect("nats://localhost:4222").await?;
nc.publish("orion.tts", serde_json::to_vec(&TTSRequest {
    text: "System ready".into(),
    voice: None,
    speed: 1.0,
    sample_rate: 16000,
})?).into()).await?;
```

---

## Troubleshooting

| Issue | Fix |
|-------|-----|
| No audio input | Check `pactl list sources`, ensure mic not muted |
| Wake word not detected | Lower `vad_threshold`, speak closer/clearer |
| TTS sounds robotic | Try different piper voice, adjust `speed` |
| High latency | Ensure CUDA is working: `nvidia-smi`, check `candle-core` CUDA feature |
| Model not found | Verify `~/.orion/models/` has correct `.bin`/`.onnx` files |

---

## Performance Targets

| Metric | Target |
|--------|--------|
| Wake word → STT start | < 200ms |
| STT latency (16kHz, 5s) | < 800ms |
| TTS latency (first audio) | < 300ms |
| GPU VRAM (whisper large) | ~2.5 GB |
| GPU VRAM (piper) | ~500 MB |