# SKILL.md
name: orion-automation
description: Use when scheduled/triggered automation needed. Cron jobs, event-driven workflows, DAG execution.
category: orion
triggers: ["automation", "cron", "schedule", "trigger", "workflow", "dag", "pipeline"]

---

## Overview

ORION Automation runs tasks on schedules or triggers:
- **Cron Jobs** — Traditional time-based scheduling
- **Event Triggers** — File changes, webhooks, bus messages, system events
- **DAG Workflows** — Multi-step pipelines with dependencies
- **Conditionals** — Run only when conditions met
- **Retries/Backoff** — Exponential backoff, max attempts

---

## Installation

```bash
# Build ORION core
cd /home/unk-/Orion/ORION/core
cargo build --release --workspace

# The automation engine runs in the planning core
# No separate binary needed
```

---

## Configuration

Edit `~/.config/orion/config.toml`:

```toml
[automation]
enabled = true
max_concurrent = 10
default_timeout_ms = 300000  # 5 minutes
retry_policy = { max_attempts = 3, base_delay_ms = 1000, max_delay_ms = 60000 }

# Example jobs (can also be defined via API)
[[automation.jobs]]
id = "morning_briefing"
name = "Morning Briefing"
schedule = "0 8 * * *"  # 8 AM daily
trigger = { type = "cron" }
steps = [
  { tool = "orion-planning", action = "plan", args = { goal = "Summarize calendar and emails for today" } },
  { tool = "orion-voice", action = "tts", args = { text = "{{step_0.result.summary}}" } }
]

[[automation.jobs]]
id = "auto_backup"
name = "Auto Backup"
schedule = "0 2 * * *"  # 2 AM daily
trigger = { type = "cron" }
steps = [
  { tool = "orion-control", action = "execute", args = { command = "borg create ..." } }
]

[[automation.jobs]]
id = "code_watch"
name = "Code Change Watcher"
trigger = { type = "file_change", paths = ["~/Orion"], pattern = "*.rs" }
steps = [
  { tool = "orion-memory", action = "reindex", args = { path = "{{trigger.path}}" } },
  { tool = "orion-planning", action = "plan", args = { goal = "Check for TODO/FIXME in {{trigger.path}}" } }
]
```

---

## Job Definition Schema

```json
{
  "id": "unique_job_id",
  "name": "Human readable name",
  "enabled": true,
  "schedule": "0 8 * * *",  // cron expression (optional if trigger.type != cron)
  "trigger": {
    "type": "cron|file_change|webhook|bus_message|system_event|manual",
    "config": { ... }
  },
  "condition": "expr",  // optional: CEL expression, e.g. "time.hour >= 9 && time.hour <= 17"
  "steps": [
    {
      "tool": "tool_name",
      "action": "action_name",
      "args": { ... },  // template variables: {{step_N.result}}, {{trigger}}, {{time}}
      "depends_on": ["step_0"],
      "timeout_ms": 30000,
      "retry": { "max_attempts": 3, "backoff_ms": 1000 }
    }
  ],
  "on_success": ["job_id"],  // trigger other jobs
  "on_failure": ["alert_job"]
}
```

---

## Trigger Types

| Type | Config | Example |
|------|--------|---------|
| `cron` | `schedule` | `"0 8 * * *"` |
| `file_change` | `paths`, `pattern`, `recursive` | `{paths:["~/Orion"], pattern:"*.rs"}` |
| `webhook` | `path`, `secret` | `{path:"/webhook/github", secret:"..."}` |
| `bus_message` | `subject`, `filter` | `{subject:"orion.memory.indexed"}` |
| `system_event` | `event_type` | `{event_type:"sleep"}` |
| `manual` | (none) | Trigger via API |

---

## Template Variables

| Variable | Description |
|----------|-------------|
| `{{step_N.result}}` | Result of step N |
| `{{step_N.result.field}}` | Nested field |
| `{{trigger.path}}` | File change path |
| `{{trigger.subject}}` | Bus message subject |
| `{{trigger.payload}}` | Bus message payload |
| `{{time.now}}` | Current timestamp |
| `{{time.iso}}` | ISO8601 now |
| `{{job.id}}` | Current job ID |
| `{{config.x}}` | Any config value |

---

## API (via Message Bus)

### Publish: `orion.automation.job.create`
Create job:
```json
{ "job": { ... job definition ... } }
```

### Publish: `orion.automation.job.delete`
Delete job:
```json
{ "id": "job_id" }
```

### Publish: `orion.automation.job.trigger`
Manual trigger:
```json
{ "id": "job_id", "args": { "custom": "values" } }
```

### Subscribe: `orion.automation.job.status`
Job status updates:
```json
{
  "job_id": "morning_briefing",
  "run_id": "run_123",
  "status": "running|success|failed|partial",
  "step": 2,
  "step_result": { ... },
  "started_at": 1234567890,
  "finished_at": 1234567950
}
```

---

## Usage Examples

### From Hermes (Python)
```python
import asyncio
import async_nats
import json

async def create_daily_job():
    nc = await async_nats.connect("nats://localhost:4222")
    await nc.publish("orion.automation.job.create", json.dumps({
        "job": {
            "id": "daily_summary",
            "name": "Daily Code Summary",
            "enabled": True,
            "schedule": "0 18 * * 1-5",  # 6 PM weekdays
            "trigger": {"type": "cron"},
            "steps": [
                {
                    "tool": "orion-memory",
                    "action": "search",
                    "args": {"text": "TODO OR FIXME", "entity_types": ["code"], "limit": 20},
                    "depends_on": []
                },
                {
                    "tool": "orion-voice",
                    "action": "tts",
                    "args": {"text": "You have {{step_0.result.total}} pending tasks"},
                    "depends_on": [0]
                }
            ]
        }
    }).encode())
    await nc.close()

asyncio.run(create_daily_job())
```

---

## Troubleshooting

| Issue | Fix |
|-------|-----|
| Job not running | Check `enabled: true`, verify cron syntax, check NATS connectivity |
| Steps fail silently | Subscribe to `orion.automation.job.status`, check tool availability |
| Template not resolving | Ensure `{{step_N.result}}` matches actual step index |
| High resource usage | Reduce `max_concurrent`, add `condition` to limit runs |

---

## Performance Targets

| Metric | Target |
|--------|--------|
| Job dispatch latency | < 10ms |
| Cron precision | ±1 second |
| File change detection | < 500ms |
| Max concurrent jobs | 10 (configurable) |