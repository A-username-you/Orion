use anyhow::Result;
use orion_types::{JarvisConfig, MemoryConfig, MemoryEntry, SearchQuery, SearchResult, MessageBus};
use tracing::{info, debug, warn, error};
use tokio::sync::mpsc;
use std::sync::Arc;
use std::path::{Path, PathBuf};
use sqlite::{Connection, State, Value};
use walkdir::WalkDir;
use notify::{RecommendedWatcher, Watcher, RecursiveMode, EventKind};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "embeddings")]
use candle_core::{Device, Tensor, DType, IndexOp};
#[cfg(feature = "embeddings")]
use candle_nn::VarBuilder;
#[cfg(feature = "embeddings")]
use candle_transformers::models::bert::{BertModel, Config as BertConfig};
use tokenizers::Tokenizer;

pub struct MemoryGraph {
    config: MemoryConfig,
    bus: Arc<MessageBus>,
    #[cfg(feature = "embeddings")]
    embedding_model: Option<Arc<BertModel>>,
    #[cfg(not(feature = "embeddings"))]
    embedding_model: Option<()>,
    tokenizer: Option<Tokenizer>,
    device: Device,
    watcher: Option<RecommendedWatcher>,
}

impl MemoryGraph {
    pub async fn new(config: &MemoryConfig, bus: Arc<MessageBus>) -> Result<Self> {
        info!("Initializing Memory Graph");

        // Initialize database (ensures schema exists; connections are opened per-operation)
        Self::init_db(&config.db_path).await?;

        #[cfg(feature = "embeddings")]
        let (embedding_model, tokenizer, device) = Self::init_embedding_model(&config.embedding_model).await?;

        #[cfg(not(feature = "embeddings"))]
        let (embedding_model, tokenizer, device) = (None, None, Device::Cpu);

        Ok(Self {
            config: config.clone(),
            bus,
            #[cfg(feature = "embeddings")]
            embedding_model: Some(embedding_model),
            #[cfg(not(feature = "embeddings"))]
            embedding_model,
            tokenizer: Some(tokenizer),
            device,
            watcher: None,
        })
    }

    async fn init_db(path: &PathBuf) -> Result<()> {
        let db = Connection::open(path)?;
        
        // Note: vec0 virtual table requires sqlite-vec extension
        // For now, use regular table with JSON embedding storage
        // db.execute("CREATE VIRTUAL TABLE IF NOT EXISTS vec_items USING vec0(embedding float[1024])")?;
        
        // Create main tables
        db.execute(
            "CREATE TABLE IF NOT EXISTS entries (
                id TEXT PRIMARY KEY,
                entity_type TEXT NOT NULL,
                content TEXT NOT NULL,
                metadata TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                source_path TEXT
            )"
        )?;
        
        // Embeddings stored as JSON blob (temporary until sqlite-vec available)
        db.execute(
            "CREATE TABLE IF NOT EXISTS embeddings (
                entry_id TEXT PRIMARY KEY,
                embedding TEXT NOT NULL,  -- JSON array
                FOREIGN KEY(entry_id) REFERENCES entries(id)
            )"
        )?;
        
        db.execute(
            "CREATE INDEX IF NOT EXISTS idx_entries_type ON entries(entity_type)"
        )?;
        
        db.execute(
            "CREATE INDEX IF NOT EXISTS idx_entries_created ON entries(created_at)"
        )?;
        
        db.execute(
            "CREATE INDEX IF NOT EXISTS idx_entries_source ON entries(source_path)"
        )?;
        
        info!("Database initialized at {}", path.display());
        Ok(())
    }

    #[cfg(feature = "embeddings")]
    async fn init_embedding_model(model_name: &str) -> Result<(Arc<BertModel>, Tokenizer, Device)> {
        let device = Device::new_cuda(0).unwrap_or(Device::Cpu);
        info!("Using device: {:?}", device);

        let model_path = Self::resolve_model_path(model_name)?;

        let tokenizer = Tokenizer::from_file(model_path.join("tokenizer.json"))
            .map_err(|e| anyhow::anyhow!("Tokenizer load failed: {}", e))?;

        let config_json = std::fs::read_to_string(model_path.join("config.json"))
            .map_err(|e| anyhow::anyhow!("Config load failed: {}", e))?;
        let config = serde_json::from_str::<BertConfig>(&config_json)
            .map_err(|e| anyhow::anyhow!("Config parse failed: {}", e))?;

        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[model_path.join("model.safetensors")], DType::F32, &device)? };
        let model = BertModel::load(vb, &config)?;

        info!("Embedding model loaded");
        Ok((Arc::new(model), tokenizer, device))
    }

        #[cfg(not(feature = "embeddings"))]
        async fn init_embedding_model(_model_name: &str) -> Result<((), Tokenizer, Device)> {
            warn!("Embeddings feature not enabled, skipping model load");
            let device = Device::Cpu;
            let model_path = Self::resolve_model_path("bge-m3")?;
            let tokenizer = Tokenizer::from_file(model_path.join("tokenizer.json"))
                .map_err(|e| anyhow::anyhow!("Tokenizer load failed: {}", e))?;
            Ok(((), tokenizer, device))
        }

    fn resolve_model_path(name: &str) -> Result<PathBuf> {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let models_dir = home.join(".orion").join("models");
        Ok(models_dir.join(name))
    }

    pub async fn run(mut self) -> Result<()> {
        info!("Starting memory graph");

        // Start file watcher
        self.start_file_watcher().await?;

        // Initial indexing of watch paths
        self.index_watch_paths().await?;

        // Subscribe to commands
        let cmd_handle = self.subscribe_commands().await?;

        // Wait for shutdown
        cmd_handle.await??;

        Ok(())
    }

    async fn start_file_watcher(&mut self) -> Result<()> {
            let config = self.config.clone();
            let bus = self.bus.clone();
            let db_path = self.config.db_path.clone();
            let embedding_model = self.embedding_model.clone();
            let tokenizer = self.tokenizer.clone();
            let device = self.device.clone();
        
            let (tx, mut rx) = mpsc::channel(100);
        
            let mut watcher = RecommendedWatcher::new(
                move |res| {
                    if let Ok(event) = res {
                        let _ = tx.blocking_send(event);
                    }
                },
                notify::Config::default(),
            )?;
        
            for path in &config.watch_paths {
                watcher.watch(path, RecursiveMode::Recursive)?;
            }
        
            self.watcher = Some(watcher);
        
            let config_clone = config.clone();
            tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    if let Err(e) = Self::handle_file_event(
                        &event, &config_clone, &db_path, &embedding_model, &tokenizer, &device, &bus
                    ).await {
                        warn!("File event handling error: {}", e);
                    }
                }
            });
        
            info!("File watcher started for {} paths", config.watch_paths.len());
            Ok(())
        }

    async fn handle_file_event(
        event: &notify::Event,
        config: &MemoryConfig,
        db_path: &PathBuf,
        embedding_model: &Option<Arc<BertModel>>,
        tokenizer: &Option<Tokenizer>,
        device: &Device,
        bus: &Arc<MessageBus>,
    ) -> Result<()> {
        match event.kind {
            EventKind::Create(_) | EventKind::Modify(_) => {
                for path in &event.paths {
                    if path.is_file() && Self::should_index(path) {
                        Self::index_file(path, config, db_path, embedding_model, tokenizer, device).await?;
                        let _ = bus.publish("orion.memory.indexed", &serde_json::json!({
                                                "path": path.display().to_string(),
                                                "event": "created_or_modified"
                                            })).await;
                    }
                }
            }
            EventKind::Remove(_) => {
                for path in &event.paths {
                    if let Ok(db) = Connection::open(db_path) {
                        let path_str = path.display().to_string();
                        db.execute(&format!("DELETE FROM entries WHERE source_path = '{}'", path_str))?;
                        // db.execute(&format!("DELETE FROM vec_items WHERE rowid IN (SELECT rowid FROM entries WHERE source_path = '{}')", path_str))?;
                    }
                    let _ = bus.publish("orion.memory.removed", &serde_json::json!({
                        "path": path.display().to_string()
                    })).await;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn should_index(path: &Path) -> bool {
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        matches!(ext, 
            "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "json" | "toml" | "yaml" | "yml" |
            "md" | "txt" | "html" | "css" | "sql" | "sh" | "bash" | "zsh" | "fish" |
            "go" | "java" | "kt" | "cpp" | "cc" | "c" | "h" | "hpp" | "cs" | "rb" |
            "php" | "swift" | "dart" | "lua" | "r" | "m" | "mm" | "pl" | "pm"
        )
    }

    async fn index_watch_paths(&self) -> Result<()> {
        for path in &self.config.watch_paths {
            self.index_path(path).await?;
        }
        Ok(())
    }

    async fn index_path(&self, path: &PathBuf) -> Result<()> {
        if path.is_file() {
            if Self::should_index(path) {
                MemoryGraph::index_file(path, &self.config, &self.config.db_path, &self.embedding_model, &self.tokenizer, &self.device).await?;
            }
        } else if path.is_dir() {
            for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
                if entry.file_type().is_file() && Self::should_index(entry.path()) {
                    let entry_path = entry.path().to_path_buf();
                    MemoryGraph::index_file(&entry_path, &self.config, &self.config.db_path, &self.embedding_model, &self.tokenizer, &self.device).await?;
                }
            }
        }
        Ok(())
    }

    async fn index_file(
        path: &PathBuf,
        config: &MemoryConfig,
        db_path: &PathBuf,
        embedding_model: &Option<Arc<BertModel>>,
        tokenizer: &Option<Tokenizer>,
        device: &Device,
    ) -> Result<()> {
        let content = std::fs::read_to_string(path)?;
        if content.trim().is_empty() {
            return Ok(());
        }
        
        let chunks = Self::chunk_text(&content, config.chunk_size, config.chunk_overlap);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        
        if let (Some(model), Some(tokenizer)) = (embedding_model, tokenizer) {
            let db = Connection::open(db_path)?;
            for (i, chunk) in chunks.iter().enumerate() {
                let id = format!("{}:{}", path.display(), i);
                let embedding = Self::embed_text(model, tokenizer, device, chunk)?;
                
                // Insert into entries table
                let metadata = serde_json::json!({
                    "chunk_index": i,
                    "total_chunks": chunks.len(),
                    "file_size": content.len(),
                });
                
                let mut stmt = db.prepare(
                    "INSERT OR REPLACE INTO entries (id, entity_type, content, metadata, created_at, updated_at, source_path)
                     VALUES (?, ?, ?, ?, ?, ?, ?)"
                )?;
                stmt.bind_iter([
                    (1, id.as_str()),
                    (2, "code"),
                    (3, chunk),
                    (4, metadata.to_string().as_str()),
                    (5, now.to_string().as_str()),
                    (6, now.to_string().as_str()),
                    (7, path.display().to_string().as_str()),
                ])?;
                
                // Insert into embeddings table (JSON)
                let embedding_json = serde_json::to_string(&embedding)?;
                
                let mut stmt = db.prepare(
                    "INSERT OR REPLACE INTO embeddings (entry_id, embedding) VALUES (?, ?)"
                )?;
                stmt.bind_iter([(1, id.as_str()), (2, embedding_json.as_str())])?;
            }
        }
        
        Ok(())
    }

    fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        let mut chunks = Vec::new();
        let mut start = 0;
        
        while start < chars.len() {
            let end = (start + chunk_size).min(chars.len());
            let chunk: String = chars[start..end].iter().collect();
            chunks.push(chunk);
            start += chunk_size - overlap;
        }
        
        chunks
    }

    fn embed_text(
            model: &BertModel,
            tokenizer: &Tokenizer,
            device: &Device,
            text: &str,
        ) -> Result<Vec<f32>> {
            let encoding = tokenizer.encode(text, true)
                .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;

            let input_ids = Tensor::new(&encoding.get_ids()[..], device)?.unsqueeze(0)?;
            let token_type_ids = Tensor::new(&encoding.get_type_ids()[..], device)?.unsqueeze(0)?;
        
            // BERT forward takes (input_ids, token_type_ids, attention_mask) - attention_mask is optional
            let attention_mask = Tensor::new(&encoding.get_attention_mask()[..], device)?.unsqueeze(0)?;
            let output = model.forward(&input_ids, &token_type_ids, Some(&attention_mask))?;

            // Use [CLS] token embedding
            let cls_embedding = output.i(0)?.i(0)?; // [seq_len, hidden] -> [hidden]
            let embedding: Vec<f32> = cls_embedding.to_vec1()?;

            Ok(embedding)
        }

    async fn subscribe_commands(&self) -> Result<tokio::task::JoinHandle<Result<()>>> {
        let bus = self.bus.clone();
        let db_path = self.config.db_path.clone();
        let tokenizer = self.tokenizer.clone();
        let device = self.device.clone();
        let embedding_model = self.embedding_model.clone();

        let handle = tokio::spawn(async move {
            let mut sub = bus.subscribe("orion.memory.search").await?;

            while let Some(msg) = sub.next().await {
                if let Ok(query) = serde_json::from_slice::<SearchQuery>(&msg.payload) {
                    let result = Self::search(&query, &db_path, &embedding_model, &tokenizer, &device).await;
                    let _ = bus.publish("orion.memory.results", &result).await;
                }
            }

            Ok(())
        });

        Ok(handle)
    }

    async fn search(
        query: &SearchQuery,
        db_path: &PathBuf,
        embedding_model: &Option<Arc<BertModel>>,
        tokenizer: &Option<Tokenizer>,
        device: &Device,
    ) -> SearchResult {
            let mut results = Vec::new();
            let mut scores = Vec::new();

            #[cfg(feature = "embeddings")]
            if let (Some(model), Some(tokenizer)) = (embedding_model, tokenizer) {
                let model = model.as_ref();
                let query_embedding = if let Some(emb) = &query.embedding {
                    emb.clone()
                } else if let Some(text) = &query.text {
                    match Self::embed_text(model, tokenizer, device, text) {
                        Ok(e) => e,
                        Err(_) => return SearchResult { entries: vec![], scores: vec![], total: 0 },
                    }
                } else {
                    return SearchResult { entries: vec![], scores: vec![], total: 0 };
                };

                // Fetch all embeddings and compute cosine similarity in Rust (simpler for now)
                // In production, use sqlite-vec for efficient vector search
                let db = Connection::open(db_path).unwrap();
                let mut stmt = db.prepare(
                    "SELECT e.id, e.entity_type, e.content, e.metadata, e.created_at, e.updated_at, e.source_path,
                            em.embedding
                     FROM entries e
                     JOIN embeddings em ON e.id = em.entry_id
                     LIMIT ? OFFSET ?"
                ).unwrap();

                stmt.bind_iter([(1, query.limit as i64), (2, query.offset as i64)]).unwrap();

                let mut rows = stmt.into_iter();

                while let Some(Ok(row)) = rows.next() {
                    let entry = MemoryEntry {
                        id: rows.read::<String, _>("id").unwrap(),
                        entity_type: rows.read::<String, _>("entity_type").unwrap(),
                        content: rows.read::<String, _>("content").unwrap(),
                        embedding: query_embedding.clone(),
                        metadata: serde_json::from_str(&rows.read::<String, _>("metadata").unwrap()).unwrap_or_default(),
                        created_at: rows.read::<i64, _>("created_at").unwrap() as u64,
                        updated_at: rows.read::<i64, _>("updated_at").unwrap() as u64,
                        source_path: rows.read::<Option<String>, _>("source_path").unwrap().map(PathBuf::from),
                    };

                    let embedding_json: String = rows.read("embedding").unwrap();
                    let stored_embedding: Vec<f32> = serde_json::from_str(&embedding_json).unwrap_or_default();

                    // Compute cosine similarity
                    let score = Self::cosine_similarity(&query_embedding, &stored_embedding);

                    results.push(entry);
                    scores.push(score);
                }
            }

            // Sort by score descending
            let mut indexed: Vec<_> = results.into_iter().zip(scores.into_iter()).collect();
            indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

            let total = indexed.len();
            results = indexed.iter().map(|(e, _)| e.clone()).collect();
            scores = indexed.iter().map(|(_, s)| *s).collect();

            SearchResult {
                entries: results,
                scores,
                total,
            }
        }
    
    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }
        
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        
        if norm_a == 0.0 || norm_b == 0.0 {
            0.0
        } else {
            dot / (norm_a * norm_b)
        }
    }
}