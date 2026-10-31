use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use kernel::model::LlmResponse;
use kernel::model::{
    Embedding, ModelChunk, ModelProvider, ModelProviderError, ModelRequest, ModelStreamItem,
};
use parking_lot::Mutex;
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub enum Plan {
    Chunks(Vec<ModelStreamItem>),
    Open(Vec<ModelStreamItem>),
    Wait {
        entered: Arc<Notify>,
        dropped: CancellationToken,
    },
    Fail,
}

#[derive(Debug)]
pub struct Provider {
    plans: Mutex<VecDeque<Plan>>,
    pub requests: Mutex<Vec<ModelRequest>>,
    pub held: Mutex<Vec<mpsc::Sender<ModelStreamItem>>>,
}

impl Provider {
    pub fn new(plans: Vec<Plan>) -> Arc<Self> {
        Arc::new(Self {
            plans: Mutex::new(plans.into()),
            requests: Mutex::new(Vec::new()),
            held: Mutex::new(Vec::new()),
        })
    }
}

pub fn chunks(chunks: Vec<ModelChunk>) -> Plan {
    Plan::Chunks(chunks.into_iter().map(Ok).collect())
}

pub fn failure() -> ModelProviderError {
    ModelProviderError::Request {
        message: "scripted failure".to_owned(),
        source: Box::new(std::io::Error::other("scripted")),
    }
}

#[async_trait]
impl ModelProvider for Provider {
    async fn stream(
        &self,
        request: ModelRequest,
    ) -> Result<mpsc::Receiver<ModelStreamItem>, ModelProviderError> {
        self.requests.lock().push(request);
        let plan = self.plans.lock().pop_front().ok_or_else(failure)?;
        let (items, keep_open) = match plan {
            Plan::Chunks(items) => (items, false),
            Plan::Open(items) => (items, true),
            Plan::Wait { entered, dropped } => {
                let _guard = dropped.drop_guard();
                entered.notify_one();
                return std::future::pending().await;
            }
            Plan::Fail => return Err(failure()),
        };
        let (sender, receiver) = mpsc::channel(items.len().max(1));
        for item in items {
            sender.try_send(item).map_err(|_| failure())?;
        }
        if keep_open {
            self.held.lock().push(sender);
        }
        Ok(receiver)
    }

    async fn chat(&self, _request: ModelRequest) -> Result<LlmResponse, ModelProviderError> {
        Err(failure())
    }
    async fn embed(&self, _texts: Vec<String>) -> Result<Vec<Embedding>, ModelProviderError> {
        Err(failure())
    }
    async fn list_models(&self) -> Result<Vec<String>, ModelProviderError> {
        Err(failure())
    }
}
