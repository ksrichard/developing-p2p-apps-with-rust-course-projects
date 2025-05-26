use std::net::AddrParseError;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use log::{error, info};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;
use tokio_util::sync::CancellationToken;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

use crate::app::dfs_grpc::dfs_server::DfsServer;
use crate::app::dfs_grpc::PublishFileResponse;
use crate::app::models::PublicPublishedFile;
use crate::app::service::{MetadataDownloadRequest, P2pCommand};
use crate::app::{ServerError, Service};
use crate::file_processor::{
    self, FileProcessResult, FileProcessResultHash, PROCESSING_RESULT_FILE_NAME,
};
use crate::file_store::{self, PendingDownloadRecord};

use super::dfs_grpc::dfs_server::Dfs;
use super::dfs_grpc::{
    DownloadRequest, DownloadResponse, GetPublicFileResponse, GetPublicFilesRequest,
    PublishFileRequest,
};

const LOG_TARGET: &str = "grpc";

#[derive(Debug, Error)]
pub enum GrpcServerError {
    #[error("Failed to parse address: {0}")]
    AddressParse(#[from] AddrParseError),
}

#[derive(Debug)]
pub struct DfsGrpcService<F: file_store::Store + Send + Sync + 'static> {
    file_publish_tx: mpsc::Sender<FileProcessResult>,
    p2p_command_sender: mpsc::Sender<P2pCommand>,
    file_store: Arc<F>,
}

impl<F: file_store::Store + Send + Sync + 'static> DfsGrpcService<F> {
    pub fn new(
        file_publish_tx: mpsc::Sender<FileProcessResult>,
        p2p_command_sender: mpsc::Sender<P2pCommand>,
        file_store: Arc<F>,
    ) -> Self {
        Self {
            file_publish_tx,
            p2p_command_sender,
            file_store,
        }
    }
}

type ResponseStream = Pin<Box<dyn Stream<Item = Result<GetPublicFileResponse, Status>> + Send>>;

#[tonic::async_trait]
impl<F: file_store::Store + Send + Sync + 'static> Dfs for DfsGrpcService<F> {
    type GetPublicFilesStream = ResponseStream;

    async fn publish_file(
        &self,
        request: Request<PublishFileRequest>,
    ) -> Result<tonic::Response<PublishFileResponse>, tonic::Status> {
        let request = request.into_inner();
        info!(target: LOG_TARGET, "We got a new publish file request: {:?}", request);

        // file processing
        let file_processor = file_processor::Processor::new();
        let file_process_result = file_processor.process_file(&request).await?;

        // TODO: start broadcasting of this file on gossipsub periodically if it's public

        self.file_publish_tx
            .send(file_process_result)
            .await
            .map_err(|error| {
                tonic::Status::internal("Failed to send processed file details internally!")
            })?;

        Ok(Response::new(PublishFileResponse {
            success: true,
            error: String::new(),
        }))
    }

    async fn download(
        &self,
        request: Request<DownloadRequest>,
    ) -> Result<tonic::Response<DownloadResponse>, tonic::Status> {
        // downloading metadata from any peers
        let req = request.into_inner();
        let (tx, rx) = oneshot::channel();
        self.p2p_command_sender
            .send(P2pCommand::RequestMetadata {
                request: MetadataDownloadRequest {
                    file_id: req.file_id,
                },
                result: tx,
            })
            .await
            .map_err(|error| {
                tonic::Status::internal(format!("Failed to get file metadata: {error:?}"))
            })?;
        let result = rx.await.map_err(|error| {
            tonic::Status::internal(format!("Failed to get file metadata: {error:?}"))
        })?;
        let result = result.ok_or(tonic::Status::internal(
            "Failed to download metadata for file!",
        ))?;

        // saving metadata locally
        let download_path = PathBuf::from(req.download_path.clone()).join(format!(
            "{}_{}",
            result.hash_sha256().raw_hash(),
            result.original_file_name.replace(".", "_")
        ));
        tokio::fs::create_dir_all(&download_path).await?;
        let metadata_file = std::fs::File::create(download_path.join(PROCESSING_RESULT_FILE_NAME))?;
        serde_cbor::to_writer(metadata_file, &result).map_err(|error| {
            tonic::Status::internal(format!("Failed to save metadata file locally: {error}"))
        })?;
        self.file_store
            .add_pending_download(PendingDownloadRecord::new(
                FileProcessResultHash::new(req.file_id),
                result.original_file_name.clone(),
                download_path,
                result.number_of_chunks as u64,
            ))
            .map_err(|error| tonic::Status::from_error(Box::new(error)))?;

        // TODO: start parallel file download (provide files on DHT when downloaded and verified)

        // TODO: implement rest
        Ok(Response::new(DownloadResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn get_public_files(
        &self,
        request: tonic::Request<GetPublicFilesRequest>,
    ) -> Result<Response<ResponseStream>, tonic::Status> {
        let (response_tx, mut response_rx) =
            tokio::sync::mpsc::channel::<Result<GetPublicFileResponse, Status>>(100);
        let (result_tx, mut result_rx) = tokio::sync::mpsc::channel::<PublicPublishedFile>(100);
        self.p2p_command_sender
            .send(P2pCommand::GetPublicFiles { result: result_tx })
            .await
            .map_err(|error| {
                tonic::Status::internal(format!(
                    "Failed to send P2P command to get public files available: {error:?}"
                ))
            })?;
        if let Some(result) = result_rx.recv().await {
            response_tx
                .send(Ok(GetPublicFileResponse {
                    id: result.id.raw_hash(),
                    original_file_name: result.original_file_name.clone(),
                }))
                .await
                .map_err(|error| {
                    tonic::Status::internal(format!("Failed to send result: {error:?}"))
                })?;
        }

        let output_stream = ReceiverStream::new(response_rx);
        Ok(Response::new(
            Box::pin(output_stream) as Self::GetPublicFilesStream
        ))
    }
}

pub struct GrpcService<F: file_store::Store + Send + Sync + 'static> {
    port: u16,
    file_publish_tx: mpsc::Sender<FileProcessResult>,
    p2p_command_sender: mpsc::Sender<P2pCommand>,
    file_store: Arc<F>,
}

impl<F: file_store::Store + Send + Sync + 'static> GrpcService<F> {
    pub fn new(
        port: u16,
        file_publish_tx: mpsc::Sender<FileProcessResult>,
        p2p_command_sender: mpsc::Sender<P2pCommand>,
        file_store: Arc<F>,
    ) -> Self {
        Self {
            port,
            file_publish_tx,
            p2p_command_sender,
            file_store,
        }
    }
}

#[async_trait]
impl<F: file_store::Store + Send + Sync + 'static> Service for GrpcService<F> {
    async fn start(&mut self, cancel_token: CancellationToken) -> Result<(), ServerError> {
        let grpc_address = format!("127.0.0.1:{}", self.port)
            .as_str()
            .parse()
            .map_err(|error| GrpcServerError::AddressParse(error))?;
        info!(target: LOG_TARGET, "Grpc server is starting at {grpc_address}!");
        if let Err(error) = Server::builder()
            .add_service(DfsServer::new(DfsGrpcService::new(
                self.file_publish_tx.clone(),
                self.p2p_command_sender.clone(),
                self.file_store.clone(),
            )))
            .serve_with_shutdown(grpc_address, cancel_token.cancelled())
            .await
        {
            error!(target: LOG_TARGET, "Error during Grpc server run: {error:?}");
        }

        info!(target: LOG_TARGET, "Shutting down Grpc server...");

        Ok(())
    }
}
