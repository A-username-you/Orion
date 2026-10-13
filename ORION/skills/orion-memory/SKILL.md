# SKILL.md
name: orion-memory
description: Use when knowledge/memory needed. Local vector database with hybrid search, file watching, entity extraction.
category: orion
triggers: ["memory", "knowledge", "search", "recall", "vector", "embedding", "sqlite-vec", "rag"]

---

## Overview

ORION Memory provides a local, persistent knowledge graph:
- **Vector Search** — sqlite-vec for fast similarity search (1024-dim embeddings)
- **Keyword Search** — SQLite FTS5 for exact matching
- **Hybrid Search** — Combines vector + keyword with reciprocal rank fusion
- **File Watching** — Auto-indexes code/docs in configured paths
- **Entity Extraction** — Code symbols, functions, classes, configs

All local, no cloud dependencies.

---

## Installation

```bash
# Build ORION core (includes memory)
cd /home/unk-/Orion/ORION/core
cargo build --release --workspace

# Install system dependencies (Arch)
sudo pacman -S sqlite

# Download embedding model
mkdir -p ~/.orion/models
# BGE-M3 (multilingual, 1024-dim)
wget -P ~/.orion/models https://huggingface.co/BAAI/bge-m3/resolve/main/model.safetensors
wget -P ~/.orion/models https://huggingface.co/BAAI/bge-m3/resolve/main/config.json
wget -P ~/.orion/models https://huggingface.co/BAAI/bge-m3/resolve/main/tokenizer.json
```

---

## Configuration

Edit `~/.config/orion/config.toml`:

```toml
[memory]
db_path = "~/.orion/memory.db"
embedding_model = "bge-m3"
embedding_dim = 1024
chunk_size = 512
chunk_overlap = 50
watch_paths = [
  "~/Downloads",
  "~/Documents", 
  "~/Orion",
  "~/Projects"
]
```

---

## API (via Message Bus)

### Publish: `orion.memory.search`
Search memory:
```json
{
  "text": "API key configuration",
  "embedding": null,
  "entity_types": ["code", "config"],
  "filters": {},
  "limit": 10,
  "offset": 0
}
```

### Subscribe: `orion.memory.results`
Receive search results:
```json
{
  "entries": [
    {
      "id": "~/Orion/config.toml:0",
      "entity_type": "config",
      "content": "[api]\nkey = \"...\"",
      "embedding": [...],
      "metadata": {"chunk_index": 0, "total_chunks": 1},
      "created_at": 1234567890,
      "updated_at": 1234567890,
      "source_path": "~/Orion/config.toml"
    }
  ],
  "scores": [0.92],
  "total": 1
}
```

### Subscribe: `orion.memory.indexed`
File indexed notification:
```json
{
  "path": "~/Orion/src/main.rs",
  "event": "created_or_modified"
}
```

### Subscribe: `orion.memory.removed`
File removed notification:
```json
{
  "path": "~/Orion/old_file.rs"
}
```

---

## Usage Examples

### From Hermes (Python)
```python
import asyncio
import async_nats
import json

async def search_memory(query):
    nc = await async_nats.connect("nats://localhost:4222")
    await nc.publish("orion.memory.search", json.dumps({
        "text": query,
        "limit": 5
    }).encode())
    
    sub = await nc.subscribe("orion.memory.results")
    async for msg in sub.messages:
        result = json.loads(msg.data)
        for entry, score in zip(result['entries'], result['scores']):
            print(f"[{score:.2f}] {entry['source_path']}: {entry['content'][:100]}...")
        break
    await nc.close()

asyncio.run(search_memory("database connection"))
```

### From Rust
```rust
use orion_core::*;

let nc = async_nats::connect("nats://localhost:4222").await?;
nc.publish("orion.memory.search", serde_json::to_vec(&SearchQuery {
    text: Some("authentication flow".into()),
    embedding: None,
    entity_types: Some(vec!["code".into()]),
    filters: serde_json::json!({}),
    limit: 10,
    offset: 0,
})?.into()).await?;
```

---

## Database Schema

```sql
-- Main entries table
CREATE TABLE entries (
    id TEXT PRIMARY KEY,
    entity_type TEXT NOT NULL,  -- code, config, doc, email, note, etc.
    content TEXT NOT NULL,
    metadata TEXT NOT NULL,     -- JSON
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    source_path TEXT
);

-- Vector index (sqlite-vec virtual table)
CREATE VIRTUAL TABLE vec_items USING vec0(embedding float[1024]);

-- Indexes
CREATE INDEX idx_entries_type ON entries(entity_type);
CREATE INDEX idx_entries_created ON entries(created_at);
CREATE INDEX idx_entries_source ON entries(source_path);
```

---

## Entity Types

| Type | Description | Source |
|------|-------------|--------|
| `code` | Source code chunks | File watcher |
| `config` | Config files (toml, yaml, json) | File watcher |
| `doc` | Markdown, text docs | File watcher |
| `email` | Email messages | Future: email integration |
| `note` | User notes | Future: note integration |
| `conversation` | Chat history | Future: chat integration |

---

## Troubleshooting

| Issue | Fix |
|-------|-----|
| Search returns no results | Run initial indexing: `orion memory reindex` |
| Embeddings not generated | Check model path, ensure CUDA available |
| High memory usage | Reduce `chunk_size`, limit `watch_paths` |
| File watcher misses events | Increase inotify limits: `echo 524288 | sudo tee /proc/sys/fs/inotify/max_user_watches` |

---

## Performance Targets

| Metric | Target |
|--------|--------|
| Search latency (10k docs) | < 50ms |
| Indexing speed | ~50 files/sec |
| Database size (10k chunks) | ~50 MB |
| GPU VRAM (embedding) | ~1 GB |