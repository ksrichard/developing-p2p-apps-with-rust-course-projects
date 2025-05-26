use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;
use tokio::{select, sync::Mutex, task::{JoinError, JoinSet}};
use tokio_util::sync::CancellationToken;

use super::P2pService;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Task join error: {0}")]
    TaskJoin(#[from] JoinError),
}

pub type ServerResult<T> = Result<T, Error>;

pub struct Server {
    cancel_token: CancellationToken,
    subtasks: Arc<Mutex<JoinSet<Result<(), Error>>>>,
}

#[async_trait]
pub trait Service: Send + Sync + 'static {
    async fn start(&self, cancel_token: CancellationToken) -> Result<(), Error>;
}

impl Server {
    pub fn new() -> Self {
        Self {cancel_token: CancellationToken::new(), subtasks: Arc::new(Mutex::new(JoinSet::new())) }
    }

    pub async fn start(&self) -> ServerResult<()> {
        // p2p service
        let p2p_service = P2pService::new();
        self.spawn_task(p2p_service).await?;

        Ok(())
    }

    async fn spawn_task<S: Service>(&self, service: S) -> ServerResult<()> {
        let mut join_set = self.subtasks.lock().await;
        let cancel_token = self.cancel_token.clone();
        join_set.spawn(async move {
            service.start(cancel_token).await
        });

        Ok(())
    }

    /// Stops the server.
    pub async fn stop(&self) -> ServerResult<()> {
        println!("Shutting down...");
        self.cancel_token.cancel();
        let mut tasks = self.subtasks.lock().await;
        while let Some(res) = tasks.join_next().await {
            res??;
        }
        Ok(())
    }
}