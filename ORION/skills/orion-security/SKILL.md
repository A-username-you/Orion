# SKILL.md
name: orion-security
description: Use when OPSEC/security needed. Classified term detection, tool allowlists, audit logging, capability sandboxing.
category: orion
triggers: ["security", "opsec", "classified", "audit", "sandbox", "allowlist", "guard"]

---

## Overview

ORION Security enforces operational security:
- **Classified Term Detection** — Blocks PII, secrets, API keys from leaving sandbox
- **Tool Allowlists** — Per-profile tool permissions
- **Audit Logging** — Every action logged with hash chain
- **Capability Sandboxing** — gVisor/firejail per skill
- **Network Policies** — Egress control per module

---

## Installation

```bash
# Build ORION core
cd /home/unk-/Orion/ORION/core
cargo build --release --workspace

# Install sandboxing
sudo pacman -S firejail gvisor-containerd-shim
```

---

## Configuration

Edit `~/.config/orion/config.toml`:

```toml
[security]
enabled = true
audit_log_path = "~/.orion/audit.log"
classified_patterns = [
  "api[_-]?key",
  "secret[_-]?key", 
  "password",
  "token",
  "private[_-]?key",
  "-----BEGIN.*PRIVATE KEY-----",
  "sk-[a-zA-Z0-9]{32,}",
  "xoxb-[a-zA-Z0-9-]+",
  "gh[pousr]_[a-zA-Z0-9]{36,}",
  "\\b\\d{4}[- ]?\\d{4}[- ]?\\d{4}[- ]?\\d{4}\\b",  # Credit card
  "\\b\\d{3}-\\d{2}-\\d{4}\\b",  # SSN
]

[security.allowlists]
default = ["orion-memory", "orion-voice"]
desktop = ["orion-control", "orion-automation", "codex", "claude-code"]
server = ["orion-planning", "orion-memory"]
mobile = ["orion-voice"]

[security.sandbox]
enabled = true
runtime = "firejail"  # firejail, gvisor, none
profiles = { "orion-control" = "firejail --net=none --nosound", "codex" = "firejail --net=default" }

[security.network]
default_deny = true
allowed_domains = [
  "api.nvidia.com",
  "huggingface.co",
  "github.com",
  "raw.githubusercontent.com"
]
```

---

## API (via Message Bus)

### Publish: `orion.security.audit`
Log security event (auto-called by modules):
```json
{
  "event_type": "tool_call|data_egress|classified_blocked|permission_denied",
  "tool": "orion-control",
  "user": "unk-",
  "details": { "action": "screenshot", "region": "full" },
  "risk_level": "low|medium|high|critical",
  "timestamp": 1234567890
}
```

### Subscribe: `orion.security.violations`
Real-time violation alerts:
```json
{
  "violation_type": "classified_term_detected",
  "term_matched": "api_key",
  "context": "Attempted to send API key in web request",
  "blocked": true,
  "timestamp": 1234567890
}
```

### Publish: `orion.security.check`
Pre-flight check for tool calls:
```json
{
  "tool": "web_search",
  "args": { "query": "my api key is sk-1234..." },
  "profile": "desktop"
}
```

### Subscribe: `orion.security.check_result`
```json
{
  "allowed": false,
  "reason": "Classified term 'api key' detected in arguments",
  "sanitized_args": { "query": "my api key is [REDACTED]..." }
}
```

---

## Classified Term Detection

Uses regex + ML classifier (local) to detect:
- API keys (OpenAI, Anthropic, GitHub, AWS, NVIDIA, etc.)
- Private keys (SSH, PGP, TLS)
- Passwords in URLs/configs
- PII (SSN, credit card, phone, email)
- Internal project codenames
- Custom patterns from config

---

## Tool Allowlist Model

```
Profile → Tool Allowlist
-------------------
default    → memory, voice
desktop    → default + control, automation, codex, claude-code
server     → default + planning, memory (no control/voice)
mobile     → voice only
custom     → user-defined
```

Enforced at message bus layer — unauthorized publish/subscribe rejected.

---

## Audit Log Format

```
[timestamp] [hash] [event_type] [tool] [user] [risk] [details_json]
```

Hash chain: each line includes SHA256(previous_line + current_line) for tamper evidence.

---

## Usage Examples

### From Hermes (Python)
```python
import asyncio
import async_nats
import json

async def check_before_action(tool, args, profile="desktop"):
    nc = await async_nats.connect("nats://localhost:4222")
    await nc.publish("orion.security.check", json.dumps({
        "tool": tool,
        "args": args,
        "profile": profile
    }).encode())
    
    sub = await nc.subscribe("orion.security.check_result")
    async for msg in sub.messages:
        result = json.loads(msg.data)
        if not result['allowed']:
            print(f"BLOCKED: {result['reason']}")
            # Use sanitized_args instead
            args = result.get('sanitized_args', args)
        return args
    await nc.close()
```

---

## Troubleshooting

| Issue | Fix |
|-------|-----|
| Legitimate requests blocked | Add pattern to allowlist, adjust regex, check risk_level |
| Sandbox breaks tool | Check firejail profile, add `--whitelist=/path`, try `runtime=none` |
| Network blocked | Add domain to `allowed_domains`, check DNS resolution in sandbox |
| Audit log growing | Configure logrotate, archive old logs |

---

## Performance Targets

| Metric | Target |
|--------|--------|
| Check latency | < 5ms |
| Classified detection | < 10ms |
| Sandbox spawn | < 200ms |
| Audit write | < 1ms |