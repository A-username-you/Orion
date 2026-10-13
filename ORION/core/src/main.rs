use anyhow::Result;
use orion_types::JarvisConfig;
use tracing::{info, error};
use tokio::signal;
use std::sync::Arc;
use orion_types::MessageBus;
use orion_voice::VoicePipeline;
use orion_control::ControlEngine;
use orion_memory::MemoryGraph;
use orion_planning::PlanningCore;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    info!("Starting ORION Core Daemon");

    // Load configuration
    let config = JarvisConfig::load()?;
    info!("Configuration loaded for profile: {}", config.profiles.active);

    // Initialize message bus first (other modules depend on it)
    // Note: orion_bus::MessageBus adds a gRPC server on top; the in-process bus
    // (orion_types::MessageBus) is what the modules communicate through.
    let bus = Arc::new(MessageBus::new(&config.bus).await?);
    info!("Message bus connected");

    // Initialize core modules
    let memory = MemoryGraph::new(&config.memory, bus.clone()).await?;
    info!("Memory graph initialized");

    let control = ControlEngine::new(&config.control, bus.clone()).await?;
    info!("Control engine initialized");

    let voice = VoicePipeline::new(&config.voice, bus.clone()).await?;
    info!("Voice pipeline initialized");

    let planning = PlanningCore::new(&config.planning, bus.clone()).await?;
    info!("Planning core initialized");

    // Start all modules
    let mut handles = vec![];

    handles.push(tokio::spawn(async move {
        if let Err(e) = memory.run().await {
            error!("Memory graph error: {}", e);
        }
    }));

    handles.push(tokio::spawn(async move {
        if let Err(e) = control.run().await {
            error!("Control engine error: {}", e);
        }
    }));

    handles.push(tokio::spawn(async move {
        if let Err(e) = voice.run().await {
            error!("Voice pipeline error: {}", e);
        }
    }));

    handles.push(tokio::spawn(async move {
        if let Err(e) = planning.run().await {
            error!("Planning core error: {}", e);
        }
    }));

    // Wait for shutdown signal
    match signal::ctrl_c().await {
        Ok(()) => info!("Shutdown signal received"),
        Err(e) => error!("Error listening for shutdown: {}", e),
    }

    info!("Shutting down ORION Core Daemon");

    // Graceful shutdown - cancel all tasks
    for handle in handles {
        handle.abort();
    }

    Ok(())
}