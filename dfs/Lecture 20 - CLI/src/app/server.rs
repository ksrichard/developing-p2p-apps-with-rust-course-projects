use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use log::info;
use thiserror::Error;
use tokio::{
    io,
    sync::Mutex,
    task::{JoinError, JoinHandle},
};
use tokio_util::sync::CancellationToken;

use crate::{
    cli::Cli,
    file_processor::FileProcessResult,
    file_store::rocksdb::{RocksDb, RocksDbStoreError},
};

use super::{
    config::P2pServiceConfig,
    grpc::server::{GrpcServerError, GrpcService},
    service::{P2pCommand, P2pNetworkError, P2pService},
};

const LOG_TARGET: &str = "app::server";

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("Task join error: {0}")]
    TaskJoin(#[from] JoinError),
    #[error("P2P network error: {0}")]
    P2pNetwork(#[from] P2pNetworkError),
    #[error("Grpc server error: {0}")]
    GrpcServer(#[from] GrpcServerError),
    #[error("RocksDB store error: {0}")]
    RocksDBStore(#[from] RocksDbStoreError),
    #[error("Failed to initialize base directory at {0}")]
    InitBaseDir(PathBuf),
    #[error("I/O error: {0}")]
    IO(#[from] io::Error),
}

pub type ServerResult<T> = Result<T, ServerError>;

pub struct Server {
    cli: Cli,
    cancel_token: CancellationToken,
    subtasks: Arc<Mutex<Vec<JoinHandle<Result<(), ServerError>>>>>,
}

#[async_trait]
pub trait Service: Send + Sync + 'static {
    async fn start(&mut self, cancel_token: CancellationToken) -> Result<(), ServerError>;
}

impl Server {
    pub fn new(cli: Cli) -> Self {
        Self {
            cancel_token: CancellationToken::new(),
            subtasks: Arc::new(Mutex::new(vec![])),
            cli,
        }
    }

    async fn init_base_dir(&self) -> ServerResult<()> {
        // return dir if exists
        let metadata_result = tokio::fs::metadata(&self.cli.base_path).await;
        if let Ok(metadata) = metadata_result {
            if metadata.is_dir() {
                return Ok(());
            } else {
                return Err(ServerError::InitBaseDir(self.cli.base_path.clone()));
            }
        }

        // create dir if not exists
        tokio::fs::create_dir_all(&self.cli.base_path).await?;
        
        Ok(())
    }

    pub async fn start(&self) -> ServerResult<()> {
        self.init_base_dir().await?;
        let (file_publish_tx, file_publish_rx) =
            tokio::sync::mpsc::channel::<FileProcessResult>(100);
        let (p2p_command_tx, p2p_command_rx) = tokio::sync::mpsc::channel::<P2pCommand>(100);

        let file_store = RocksDb::new(self.cli.base_path.join("file_store"))?;

        // p2p service
        let p2p_service = P2pService::new(
            P2pServiceConfig::builder()
                .with_keypair_file(self.cli.base_path.join("keys.keypair"))
                .build(),
            file_publish_rx,
            file_store,
            p2p_command_rx,
        );
        self.spawn_task(p2p_service).await?;

        // grpc service
        let grpc_service = GrpcService::new(self.cli.grpc_port, file_publish_tx, p2p_command_tx.clone());
        self.spawn_task(grpc_service).await?;

        Ok(())
    }

    async fn spawn_task<S: Service>(&self, mut service: S) -> ServerResult<()> {
        let mut handles = self.subtasks.lock().await;
        let cancel_token = self.cancel_token.clone();
        handles.push(tokio::spawn(
            async move { service.start(cancel_token).await },
        ));

        Ok(())
    }

    /// Stops the server.
    pub async fn stop(&self) -> ServerResult<()> {
        info!(target: LOG_TARGET, "Shutting down...");
        self.cancel_token.cancel();
        let mut handles = self.subtasks.lock().await;
        for handle in handles.iter_mut() {
            handle.await??;
        }
        Ok(())
    }
}
