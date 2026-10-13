# SKILL.md
name: orion-control
description: Use when computer control needed. Click, type, screenshot, OCR, window management via CUA driver.
category: orion
triggers: ["control", "click", "type", "screenshot", "ocr", "window", "automation", "cua"]

---

## Overview

ORION Control provides full desktop automation via the CUA (Computer Use Agent) driver:
- **Click/Type/Scroll/Keys** — Precise input simulation
- **Screenshots** — Full screen or region, PNG output
- **OCR** — Text extraction from screen regions (Tesseract)
- **Window Management** — List, focus, move, resize windows
- **Accessibility Tree** — Read UI elements for semantic control

Works on X11 and Wayland (via CUA driver abstraction).

---

## Installation

```bash
# Build ORION core (includes control)
cd /home/unk-/Orion/ORION/core
cargo build --release --workspace

# Install system dependencies (Arch)
sudo pacman -S tesseract tesseract-data-eng xdotool wl-clipboard grim slurp

# For Wayland: ensure XDG_SESSION_TYPE=wayland and protocol support
# For X11: standard X11 libraries
```

---

## Configuration

Edit `~/.config/orion/config.toml`:

```toml
[control]
backend = "auto"  # auto, x11, wayland, cua
screenshot_interval_ms = 1000
accessibility_enabled = true
```

---

## API (via Message Bus)

### Publish: `orion.control.action`
Execute a control action:
```json
{
  "action_type": "click",
  "params": { "x": 500, "y": 300, "button": "left" }
}
```

```json
{
  "action_type": "type",
  "params": { "text": "hello world" }
}
```

```json
{
  "action_type": "key",
  "params": { "key": "Return", "modifiers": ["ctrl"] }
}
```

```json
{
  "action_type": "scroll",
  "params": { "x": 100, "y": 100, "delta_x": 0, "delta_y": -3 }
}
```

```json
{
  "action_type": "screenshot",
  "params": { "region": { "x": 0, "y": 0, "width": 1920, "height": 1080 } }
}
```

```json
{
  "action_type": "window_list",
  "params": {}
}
```

```json
{
  "action_type": "window_focus",
  "params": { "title": "Firefox" }
}
```

```json
{
  "action_type": "ocr",
  "params": { "region": { "x": 100, "y": 100, "width": 500, "height": 200 } }
}
```

### Subscribe: `orion.control.result`
Receive action result:
```json
{
  "success": true,
  "output": { "windows": [...] },
  "error": null,
  "screenshot": [PNG bytes...]
}
```

### Subscribe: `orion.control.screenshot`
Periodic screenshot stream (1 FPS by default):
```json
{ "image": [PNG bytes...], "timestamp": 1234567890 }
```

---

## Usage Examples

### From Hermes (Python)
```python
import asyncio
import async_nats
import json
import base64

async def click(x, y):
    nc = await async_nats.connect("nats://localhost:4222")
    await nc.publish("orion.control.action", json.dumps({
        "action_type": "click",
        "params": {"x": x, "y": y, "button": "left"}
    }).encode())
    # Wait for result
    sub = await nc.subscribe("orion.control.result")
    async for msg in sub.messages:
        result = json.loads(msg.data)
        print(f"Success: {result['success']}")
        break
    await nc.close()

async def screenshot():
    nc = await async_nats.connect("nats://localhost:4222")
    sub = await nc.subscribe("orion.control.screenshot")
    async for msg in sub.messages:
        with open("screenshot.png", "wb") as f:
            f.write(msg.data)
        break
    await nc.close()
```

### From Rust
```rust
use orion_core::*;

let nc = async_nats::connect("nats://localhost:4222").await?;
nc.publish("orion.control.action", serde_json::to_vec(&ControlAction {
    action_type: ControlActionType::Click { x: 100, y: 200, button: MouseButton::Left },
    params: serde_json::json!({}),
})?.into()).await?;
```

---

## Troubleshooting

| Issue | Fix |
|-------|-----|
| CUA driver fails to start | Check `cua-driver` logs, ensure display server running |
| Wayland clicks don't work | Enable `xdg-desktop-portal`, try `backend = "x11"` |
| Screenshots are black | Wayland security: use `grim` backend or grant screen capture permission |
| OCR inaccurate | Ensure `tesseract-data-eng` installed, try preprocessing |
| Permission denied | Add user to `input` group, check Polkit rules |

---

## Performance Targets

| Metric | Target |
|--------|--------|
| Click latency | < 50ms |
| Screenshot (1080p) | < 100ms |
| OCR (500x200) | < 500ms |
| Window list | < 20ms |