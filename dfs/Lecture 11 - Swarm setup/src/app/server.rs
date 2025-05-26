use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;
use tokio::{select, sync::Mutex, task::{JoinError, JoinHandle, JoinSet}};
use tokio_util::sync::CancellationToken;

use super::{config::P2pServiceConfig, service::{P2pNetworkError, P2pService}};

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("Task join error: {0}")]
    TaskJoin(#[from] JoinError),
    #[error("P2P network error: {0}")]
    P2pNetwork(#[from] P2pNetworkError),
}

pub type ServerResult<T> = Result<T, ServerError>;

pub struct Server {
    cancel_token: CancellationToken,
    subtasks: Arc<Mutex<Vec<JoinHandle<Result<(), ServerError>>>>>,
}

#[async_trait]
pub trait Service: Send + Sync + 'static {
    async fn start(&self, cancel_token: CancellationToken) -> Result<(), ServerError>;
}

impl Server {
    pub fn new() -> Self {
        Self {cancel_token: CancellationToken::new(), subtasks: Arc::new(Mutex::new(vec![])) }
    }

    pub async fn start(&self) -> ServerResult<()> {
        // p2p service
        let p2p_service = P2pService::new(
            P2pServiceConfig::builder()
            .with_keypair_file("./keys.keypair")
            .build()
        );
        self.spawn_task(p2p_service).await?;

        Ok(())
    }

    async fn spawn_task<S: Service>(&self, service: S) -> ServerResult<()> {
        let mut handles = self.subtasks.lock().await;
        let cancel_token = self.cancel_token.clone();
        handles.push(
            tokio::spawn(async move {
                service.start(cancel_token).await
            })
    );

        Ok(())
    }

    /// Stops the server.
    pub async fn stop(&self) -> ServerResult<()> {
        println!("Shutting down...");
        self.cancel_token.cancel();
        let mut handles = self.subtasks.lock().await;
        for handle in handles.iter_mut() {
            handle.await??;
        }
        Ok(())
    }
}