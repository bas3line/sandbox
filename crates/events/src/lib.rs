//! Lifecycle event fanout with a zero-service default and an optional NATS backend.

use std::sync::Arc;

use async_trait::async_trait;
use sandbox_core::{
    config::{BusConfig, BusKind},
    model::LifecycleEvent,
};
use thiserror::Error;
use tokio::sync::broadcast;

pub type BusRef = Arc<dyn EventBus>;

#[derive(Debug, Error)]
pub enum BusError {
    #[error("NATS connection failed: {0}")]
    NatsConnect(#[from] async_nats::ConnectError),
    #[error("NATS publish failed: {0}")]
    NatsPublish(#[from] async_nats::PublishError),
    #[error("event serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
}

#[async_trait]
pub trait EventBus: Send + Sync {
    async fn publish(&self, event: &LifecycleEvent) -> Result<(), BusError>;
    fn backend_name(&self) -> &'static str;
}

pub async fn connect(config: &BusConfig) -> Result<BusRef, BusError> {
    match config.kind {
        BusKind::Memory => Ok(Arc::new(MemoryBus::new(1_024))),
        BusKind::Nats => Ok(Arc::new(
            NatsBus::connect(&config.nats_url, config.subject.clone()).await?,
        )),
    }
}

#[derive(Clone, Debug)]
pub struct MemoryBus {
    sender: broadcast::Sender<LifecycleEvent>,
}

impl MemoryBus {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self { sender }
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<LifecycleEvent> {
        self.sender.subscribe()
    }
}

#[async_trait]
impl EventBus for MemoryBus {
    async fn publish(&self, event: &LifecycleEvent) -> Result<(), BusError> {
        let _receiver_count = self.sender.send(event.clone());
        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        "memory"
    }
}

#[derive(Clone)]
pub struct NatsBus {
    client: async_nats::Client,
    subject: String,
}

impl NatsBus {
    pub async fn connect(url: &str, subject: String) -> Result<Self, BusError> {
        let client = async_nats::connect(url).await?;
        Ok(Self { client, subject })
    }
}

#[async_trait]
impl EventBus for NatsBus {
    async fn publish(&self, event: &LifecycleEvent) -> Result<(), BusError> {
        let payload = serde_json::to_vec(event)?;
        self.client
            .publish(self.subject.clone(), payload.into())
            .await?;
        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        "nats"
    }
}
