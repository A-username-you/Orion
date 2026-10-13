use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use dirs;
use tokio::sync::mpsc;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;
use async_nats::Subscriber;
use futures_util::stream::StreamExt;
use futures_util::stream::Stream;
use tracing::{info, warn, debug, error};

// ORION Shared Types
// This crate contains all shared types to avoid circular dependencies

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JarvisConfig {
    pub voice: VoiceConfig,
    pub control: ControlConfig,
    pub memory: MemoryConfig,
    pub planning: PlanningConfig,
    pub bus: BusConfig,
    pub profiles: ProfilesConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceConfig {
    pub wake_word: String,
    pub stt_model: String,
    pub tts_model: String,
    pub sample_rate: u32,
    pub vad_threshold: f32,
    pub device: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlConfig {
    pub backend: ControlBackend,
    pub screenshot_interval_ms: u64,
    pub accessibility_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlBackend {
    Auto,
    X11,
    Wayland,
    CUA,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub db_path: PathBuf,
    pub embedding_model: String,
    pub embedding_dim: usize,
    pub chunk_size: usize,
    pub chunk_overlap: usize,
    pub watch_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningConfig {
    pub llm_model: String,
    pub max_context_tokens: usize,
    pub temperature: f32,
    pub tool_timeout_ms: u64,
    pub max_retries: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusConfig {
    pub nats_url: String,
    pub grpc_addr: String,
    pub tls_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfilesConfig {
    pub active: String,
    pub desktop: ProfileConfig,
    pub server: ProfileConfig,
    pub mobile: ProfileConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileConfig {
    pub enabled_modules: Vec<String>,
    pub overrides: serde_json::Value,
}

impl Default for JarvisConfig {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let orion_dir = home.join(".orion");
        
        Self {
            voice: VoiceConfig {
                wake_word: "orion".to_string(),
                stt_model: "whisper-large-v3".to_string(),
                tts_model: "piper-en-us".to_string(),
                sample_rate: 16000,
                vad_threshold: 0.5,
                device: None,
            },
            control: ControlConfig {
                backend: ControlBackend::Auto,
                screenshot_interval_ms: 1000,
                accessibility_enabled: true,
            },
            memory: MemoryConfig {
                db_path: orion_dir.join("memory.db"),
                embedding_model: "all-MiniLM-L6-v2".to_string(),
                embedding_dim: 384,
                chunk_size: 512,
                chunk_overlap: 50,
                watch_paths: vec![
                    home.join("Downloads"),
                    home.join("Documents"),
                    home.join("Orion"),
                ],
            },
            planning: PlanningConfig {
                llm_model: "nvidia/nemotron-3-ultra".to_string(),
                max_context_tokens: 32768,
                temperature: 0.3,
                tool_timeout_ms: 30000,
                max_retries: 2,
            },
            bus: BusConfig {
                nats_url: "nats://127.0.0.1:4222".to_string(),
                grpc_addr: "127.0.0.1:50051".to_string(),
                tls_enabled: false,
            },
            profiles: ProfilesConfig {
                active: "desktop".to_string(),
                desktop: ProfileConfig {
                    enabled_modules: vec!["voice".into(), "control".into(), "memory".into(), "planning".into()],
                    overrides: serde_json::json!({}),
                },
                server: ProfileConfig {
                    enabled_modules: vec!["memory".into(), "planning".into(), "bus".into()],
                    overrides: serde_json::json!({}),
                },
                mobile: ProfileConfig {
                    enabled_modules: vec!["voice".into(), "bus".into()],
                    overrides: serde_json::json!({}),
                },
            },
        }
    }
}

impl JarvisConfig {
    pub fn load() -> Result<Self> {
        let config_path = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("orion")
            .join("config.toml");
        
        if config_path.exists() {
            let content = std::fs::read_to_string(config_path)?;
            let config: JarvisConfig = toml::from_str(&content)?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }
    
    pub fn save(&self) -> Result<()> {
        let config_path = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("orion")
            .join("config.toml");
        
        std::fs::create_dir_all(config_path.parent().unwrap())?;
        let content = toml::to_string_pretty(self)?;
        std::fs::write(config_path, content)?;
        Ok(())
    }
}

// Message bus types
pub struct BusSubscription {
    subject: String,
    local_rx: mpsc::Receiver<Vec<u8>>,
    nats_sub: Option<Subscriber>,
    subscribers: Arc<RwLock<HashMap<String, Vec<mpsc::Sender<Vec<u8>>>>>>,
}

impl BusSubscription {
    pub async fn next(&mut self) -> Option<BusMessage> {
        tokio::select! {
            Some(data) = self.local_rx.recv() => {
                Some(BusMessage { subject: self.subject.clone(), payload: data })
            }
            Some(msg) = async { self.nats_sub.as_mut()?.next().await } => {
                Some(BusMessage { subject: self.subject.clone(), payload: msg.payload.to_vec() })
            }
        }
    }
}

impl Drop for BusSubscription {
    fn drop(&mut self) {
        let subject = self.subject.clone();
        let subscribers = self.subscribers.clone();
        tokio::spawn(async move {
            let mut subs = subscribers.write().await;
            if let Some(channels) = subs.get_mut(&subject) {
                channels.retain(|tx| !tx.is_closed());
                if channels.is_empty() {
                    subs.remove(&subject);
                }
            }
        });
    }
}

#[derive(Debug, Clone)]
pub struct BusMessage {
    pub subject: String,
    pub payload: Vec<u8>,
}

// MessageBus implementation
use async_nats::Client;

pub struct MessageBus {
    config: BusConfig,
    nats: Option<Client>,
    subscribers: Arc<RwLock<HashMap<String, Vec<mpsc::Sender<Vec<u8>>>>>>,
}

impl MessageBus {
    pub async fn new(config: &BusConfig) -> Result<Self> {
        info!("Initializing Message Bus");
        
        let nats = async_nats::connect(&config.nats_url).await.ok();
        if nats.is_some() {
            info!("Connected to NATS at {}", config.nats_url);
        } else {
            warn!("NATS connection failed, using in-process bus only");
        }
        
        Ok(Self {
            config: config.clone(),
            nats,
            subscribers: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub async fn publish(&self, subject: &str, payload: &impl serde::Serialize) -> Result<()> {
        let data = serde_json::to_vec(payload)?;
        
        // Publish to NATS if available
        if let Some(nats) = &self.nats {
            nats.publish(subject.to_string(), data.clone().into()).await?;
        }
        
        // Publish to local subscribers
        let subs = self.subscribers.read().await;
        if let Some(channels) = subs.get(subject) {
            for tx in channels {
                let _ = tx.try_send(data.clone());
            }
        }
        
        Ok(())
    }

    pub async fn subscribe(&self, subject: &str) -> Result<BusSubscription> {
        let (tx, rx) = tokio::sync::mpsc::channel(100);
        
        // Register local subscriber
        {
            let mut subs = self.subscribers.write().await;
            subs.entry(subject.to_string()).or_default().push(tx);
        }
        
        // Subscribe to NATS if available
        let nats_sub = if let Some(nats) = &self.nats {
            Some(nats.subscribe(subject.to_string()).await?)
        } else {
            None
        };
        
        Ok(BusSubscription {
            subject: subject.to_string(),
            local_rx: rx,
            nats_sub,
            subscribers: self.subscribers.clone(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioChunk {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcription {
    pub text: String,
    pub confidence: f32,
    pub language: String,
    pub segments: Vec<TranscriptionSegment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TTSRequest {
    pub text: String,
    pub voice: Option<String>,
    pub speed: f32,
    pub sample_rate: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TTSResponse {
    pub audio: AudioChunk,
    pub phonemes: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlAction {
    pub action_type: ControlActionType,
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlActionType {
    Click { x: i32, y: i32, button: MouseButton },
    Type { text: String },
    Key { key: String, modifiers: Vec<String> },
    Scroll { x: i32, y: i32, delta_x: i32, delta_y: i32 },
    Screenshot { region: Option<ScreenRegion> },
    WindowList,
    WindowFocus { title: String },
    OCR { region: Option<ScreenRegion> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenRegion {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlResult {
    pub success: bool,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub screenshot: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub entity_type: String,
    pub content: String,
    pub embedding: Vec<f32>,
    pub metadata: serde_json::Value,
    pub created_at: u64,
    pub updated_at: u64,
    pub source_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    pub text: Option<String>,
    pub embedding: Option<Vec<f32>>,
    pub entity_types: Option<Vec<String>>,
    pub filters: serde_json::Value,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub entries: Vec<MemoryEntry>,
    pub scores: Vec<f32>,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanRequest {
    pub goal: String,
    pub context: serde_json::Value,
    pub constraints: Vec<String>,
    pub available_tools: Vec<String>,
    pub task_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub tool: String,
    pub args: serde_json::Value,
    pub depends_on: Vec<usize>,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanResponse {
    pub steps: Vec<PlanStep>,
    pub reasoning: String,
    pub estimated_duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub tool: String,
    pub args: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: String,
    pub success: bool,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub duration_ms: u64,
}