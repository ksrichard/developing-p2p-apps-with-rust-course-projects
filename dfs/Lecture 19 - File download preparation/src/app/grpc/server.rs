use std::net::AddrParseError;

use async_trait::async_trait;
use log::{error, info};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tonic::transport::Server;
use tonic::{Request, Response};

use crate::app::dfs_grpc::dfs_server::DfsServer;
use crate::app::dfs_grpc::PublishFileResponse;
use crate::app::service::{MetadataDownloadRequest, P2pCommand};
use crate::app::{ServerError, Service};
use crate::file_processor::{self, FileProcessResult};

use super::dfs_grpc::dfs_server::Dfs;
use super::dfs_grpc::{DownloadRequest, DownloadResponse, PublishFileRequest};

const LOG_TARGET: &str = "grpc";

#[derive(Debug, Error)]
pub enum GrpcServerError {
    #[error("Failed to parse address: {0}")]
    AddressParse(#[from] AddrParseError),
}

#[derive(Debug)]
pub struct DfsGrpcService {
    file_publish_tx: mpsc::Sender<FileProcessResult>,
    p2p_command_sender: mpsc::Sender<P2pCommand>,
}

impl DfsGrpcService {
    pub fn new(
        file_publish_tx: mpsc::Sender<FileProcessResult>,
        p2p_command_sender: mpsc::Sender<P2pCommand>,
    ) -> Self {
        Self {
            file_publish_tx,
            p2p_command_sender,
        }
    }
}

#[tonic::async_trait]
impl Dfs for DfsGrpcService {
    async fn publish_file(
        &self,
        request: Request<PublishFileRequest>,
    ) -> Result<tonic::Response<PublishFileResponse>, tonic::Status> {
        let request = request.into_inner();
        info!(target: LOG_TARGET, "We got a new publish file request: {:?}", request);

        // file processing
        let file_processor = file_processor::Processor::new();
        let file_process_result = file_processor.process_file(&request).await?;

        // // TODO: remove, only for testing
        // let proof = file_process_result.merkle_proofs.get(&10).unwrap();
        // let proof = MerkleProof::<Sha256>::try_from(proof.as_slice()).unwrap();
        // let chunk_content = tokio::fs::read(file_process_result.chunks_directory.join("10.chunk"))
        //     .await
        //     .unwrap();
        // let chunk_hash = Sha256::hash(chunk_content.as_slice());
        // let valid = proof.verify(
        //     file_process_result.merkle_root,
        //     &[10],
        //     &[chunk_hash],
        //     file_process_result.number_of_chunks,
        // );
        // info!(target: LOG_TARGET, "10th chunk validity: {valid}");

        // TODO: start providing files on DHT
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

        info!(target: LOG_TARGET, "Downloaded metadata: {:?}", result);

        // TODO: add a new field to DownloadRequest about where to download resulting files
        // TODO: save metadata file to local DB (into a new column family for downloading chunks)
        // TODO: start parallel file download (provide files on DHT when downloaded and verified)

        // TODO: implement rest
        Ok(Response::new(DownloadResponse {
            success: true,
            error: "".to_string(),
        }))
    }
}

pub struct GrpcService {
    port: u16,
    file_publish_tx: mpsc::Sender<FileProcessResult>,
    p2p_command_sender: mpsc::Sender<P2pCommand>,
}

impl GrpcService {
    pub fn new(
        port: u16,
        file_publish_tx: mpsc::Sender<FileProcessResult>,
        p2p_command_sender: mpsc::Sender<P2pCommand>,
    ) -> Self {
        Self {
            port,
            file_publish_tx,
            p2p_command_sender,
        }
    }
}

#[async_trait]
impl Service for GrpcService {
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
