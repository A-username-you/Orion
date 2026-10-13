use anyhow::Result;
use tracing::{info, debug, warn, error};
use async_nats::{Client, Subscriber};
use tonic::{transport::Server, Request, Response, Status};
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;
use tokio_stream::wrappers::ReceiverStream;
use tokio::sync::mpsc;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use futures_util::stream::StreamExt as FuturesStreamExt;
use async_stream;
use futures_util;

// Local types needed by bus module (avoiding orion-core cycle)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusConfig {
    pub nats_url: String,
    pub grpc_addr: String,
    pub tls_enabled: bool,
}

pub struct MessageBus {
    config: BusConfig,
    nats: Option<Client>,
    grpc_server: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    // Local pub/sub subscribers (Vec<u8>)
    local_subscribers: Arc<RwLock<HashMap<String, Vec<tokio::sync::mpsc::Sender<Vec<u8>>>>>>,
    // gRPC Subscribe stream subscribers (Result<SubscribeResponse, Status>)
    grpc_subscribers: Arc<RwLock<HashMap<String, Vec<tokio::sync::mpsc::Sender<Result<SubscribeResponse, Status>>>>>>,
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
            grpc_server: None,
            local_subscribers: Arc::new(RwLock::new(HashMap::new())),
            grpc_subscribers: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        info!("Starting message bus");
        
        // Start gRPC server
        let grpc_handle = self.start_grpc_server().await?;
        self.grpc_server = Some(grpc_handle);
        
        // Keep running
        if let Some(handle) = self.grpc_server.take() {
            handle.await?;
        }
        
        Ok(())
    }

    async fn start_grpc_server(&self) -> Result<tokio::task::JoinHandle<Result<()>>> {
        let addr = self.config.grpc_addr.parse()?;
        let nats = self.nats.clone();
        let local_subscribers = self.local_subscribers.clone();
        let grpc_subscribers = self.grpc_subscribers.clone();
        
        let handle = tokio::spawn(async move {
            info!("gRPC server listening on {}", addr);
            
            Server::builder()
                .add_service(BusServer::new(BusService::new(nats, local_subscribers, grpc_subscribers)))
                .serve(addr)
                .await
                .map_err(|e| anyhow::anyhow!("gRPC server error: {}", e))
        });
        
        Ok(handle)
    }

    pub async fn publish(&self, subject: &str, payload: &impl serde::Serialize) -> Result<()> {
        let data = serde_json::to_vec(payload)?;
        
        // Publish to NATS if available
        if let Some(nats) = &self.nats {
            nats.publish(subject.to_string(), data.clone().into()).await?;
        }
        
        // Publish to local subscribers
        let subs = self.local_subscribers.read().await;
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
                let mut subs = self.local_subscribers.write().await;
                subs.entry(subject.to_string()).or_default().push(tx);
            }
        
            // Subscribe to NATS if available
            let nats_sub = if let Some(nats) = &self.nats {
                Some(nats.subscribe::<String>(subject.to_string()).await.ok())
            } else {
                None
            };
        
            Ok(BusSubscription {
                subject: subject.to_string(),
                local_rx: rx,
                nats_sub: nats_sub.flatten(),
                subscribers: self.local_subscribers.clone(),
            })
        }
    }

pub struct BusSubscription {
    subject: String,
    local_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    nats_sub: Option<Subscriber>,
    subscribers: Arc<RwLock<HashMap<String, Vec<tokio::sync::mpsc::Sender<Vec<u8>>>>>>,
}

impl BusSubscription {
    pub async fn next(&mut self) -> Option<BusMessage> {
        tokio::select! {
            Some(data) = self.local_rx.recv() => {
                Some(BusMessage { subject: self.subject.clone(), payload: data })
            }
            Some(msg) = async { 
                use futures_util::stream::StreamExt;
                self.nats_sub.as_mut()?.next().await 
            } => {
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

pub struct BusMessage {
    pub subject: String,
    pub payload: Vec<u8>,
}

// gRPC service definition
pub mod orion {
    pub mod bus {
        tonic::include_proto!("orion.bus");
    }
}

// Use the generated types directly from the include_proto macro
use orion::bus::bus_server::{BusServer, Bus};
use crate::orion::bus::{PublishRequest, PublishResponse, SubscribeRequest, SubscribeResponse};

#[derive(Clone)]
pub struct BusService {
    nats: Option<Client>,
    // Local pub/sub subscribers (Vec<u8>)
    local_subscribers: Arc<RwLock<HashMap<String, Vec<tokio::sync::mpsc::Sender<Vec<u8>>>>>>,
    // gRPC Subscribe stream subscribers (Result<SubscribeResponse, Status>)
    grpc_subscribers: Arc<RwLock<HashMap<String, Vec<tokio::sync::mpsc::Sender<Result<SubscribeResponse, Status>>>>>>,
}

impl BusService {
    pub fn new(nats: Option<Client>, 
               local_subscribers: Arc<RwLock<HashMap<String, Vec<tokio::sync::mpsc::Sender<Vec<u8>>>>>>,
               grpc_subscribers: Arc<RwLock<HashMap<String, Vec<tokio::sync::mpsc::Sender<Result<SubscribeResponse, Status>>>>>>) -> Self {
        Self { nats, local_subscribers, grpc_subscribers }
    }
}

#[tonic::async_trait]
impl Bus for BusService {
    async fn publish(&self, request: Request<PublishRequest>) -> Result<Response<PublishResponse>, Status> {
        let req = request.into_inner();
        
        if let Some(nats) = &self.nats {
            nats.publish(req.subject.to_string(), req.payload.clone().into()).await
                .map_err(|e| Status::internal(e.to_string()))?;
        }
        
        // Local publish to local_subscribers
        let subs = self.local_subscribers.read().await;
        if let Some(channels) = subs.get(&req.subject) {
            for tx in channels {
                let _ = tx.try_send(req.payload.clone());
            }
        }
        
        Ok(Response::new(PublishResponse { success: true }))
    }

    type SubscribeStream = ReceiverStream<Result<SubscribeResponse, Status>>;

    async fn subscribe(&self, request: Request<SubscribeRequest>) -> Result<Response<Self::SubscribeStream>, Status> {
        let req = request.into_inner();
        let subject = req.subject;
        
        let (tx, rx) = mpsc::channel::<Result<SubscribeResponse, Status>>(100);
        
        // Register local subscriber to grpc_subscribers
        {
            let mut subs = self.grpc_subscribers.write().await;
            subs.entry(subject.clone()).or_default().push(tx.clone());
        }
        
        // Create stream from local + NATS
        let nats_clone = self.nats.clone();
        let subject_clone2 = subject.clone();
        let tx_clone = tx.clone();
        
        // Spawn task to forward NATS messages to the channel
        tokio::spawn(async move {
            let mut nats_sub = if let Some(nats) = nats_clone {
                nats.subscribe::<String>(subject_clone2.clone()).await.ok()
            } else {
                None
            };
            
            if let Some(mut nats_sub) = nats_sub {
                while let Some(msg) = nats_sub.next().await {
                    if tx_clone.send(Ok(SubscribeResponse { payload: msg.payload.to_vec() })).await.is_err() {
                        break;
                    }
                }
            }
        });
        
        // Also forward local messages from local_subscribers
        let local_subscribers_clone = self.local_subscribers.clone();
        let subject_clone3 = subject.clone();
        let tx_clone2 = tx.clone();
        
        // We need to create a bridge from local_subscribers to grpc_subscribers
        // For simplicity, we'll just use the rx channel directly for gRPC
        // The local subscribers will receive messages via their own subscribe() method
        
        Ok(Response::new(ReceiverStream::new(rx)))
    }
}