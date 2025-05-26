use std::fmt::format;
use std::net::AddrParseError;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use log::{error, info};
use rs_merkle::algorithms::Sha256;
use rs_merkle::{Hasher, MerkleProof};
use thiserror::Error;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, BufReader};
use tokio_util::sync::CancellationToken;
use tonic::transport::Server;
use tonic::{IntoRequest, Request, Response, Status};

use crate::app::publish::publish_server::PublishServer;
use crate::app::publish::PublishFileResponse;
use crate::app::{ServerError, Service};
use crate::file_processor;

use super::publish::publish_server::Publish;
use super::publish::PublishFileRequest;

const LOG_TARGET: &str = "grpc::publish";

#[derive(Debug, Error)]
pub enum GrpcServerError {
    #[error("Failed to parse address: {0}")]
    AddressParse(#[from] AddrParseError),
}

#[derive(Debug)]
pub struct PublishService {}

impl PublishService {
    pub fn new() -> Self {
        Self {}
    }
}

#[tonic::async_trait]
impl Publish for PublishService {
    async fn publish_file(
        &self,
        request: Request<PublishFileRequest>,
    ) -> Result<tonic::Response<PublishFileResponse>, tonic::Status> {
        let request = request.into_inner();
        info!(target: LOG_TARGET, "We got a new publish file request: {:?}", request);

        // file processing
        let file_processor = file_processor::Processor::new();
        let file_process_result = file_processor.process_file(&request).await?;
        // info!(target: LOG_TARGET, "File processing result: {file_process_result:?}");

        // TODO: remove, only for testing
        let proof = file_process_result.merkle_proofs.get(&10).unwrap();
        let proof = MerkleProof::<Sha256>::try_from(proof.as_slice()).unwrap();
        let chunk_content = tokio::fs::read(file_process_result.chunks_directory.join("10.chunk"))
            .await
            .unwrap();
        let chunk_hash = Sha256::hash(chunk_content.as_slice());
        let valid = proof.verify(
            file_process_result.merkle_root,
            &[10],
            &[chunk_hash],
            file_process_result.number_of_chunks,
        );
        info!(target: LOG_TARGET, "10th chunk validity: {valid}");

        // TODO: start providing files on DHT

        // TODO: start broadcasting of this file on gossipsub periodically if it's public

        Ok(Response::new(PublishFileResponse {
            success: true,
            error: String::new(),
        }))
    }
}

pub struct GrpcService {
    port: u16,
}

impl GrpcService {
    pub fn new(port: u16) -> Self {
        Self { port }
    }
}

#[async_trait]
impl Service for GrpcService {
    async fn start(&self, cancel_token: CancellationToken) -> Result<(), ServerError> {
        let grpc_address = format!("127.0.0.1:{}", self.port)
            .as_str()
            .parse()
            .map_err(|error| GrpcServerError::AddressParse(error))?;
        info!(target: LOG_TARGET, "Grpc server is starting at {grpc_address}!");
        if let Err(error) = Server::builder()
            .add_service(PublishServer::new(PublishService::new()))
            .serve_with_shutdown(grpc_address, cancel_token.cancelled())
            .await
        {
            error!(target: LOG_TARGET, "Error during Grpc server run: {error:?}");
        }

        info!(target: LOG_TARGET, "Shutting down Grpc server...");

        Ok(())
    }
}
