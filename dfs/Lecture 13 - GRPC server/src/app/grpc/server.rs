use std::net::AddrParseError;

use async_trait::async_trait;
use log::{error, info};
use publish::publish_server::{Publish, PublishServer};
use publish::{PublishFileRequest, PublishFileResponse};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tonic::transport::Server;
use tonic::{Request, Response};

use crate::app::{ServerError, Service};

const LOG_TARGET: &str = "grpc::publish";

#[derive(Debug, Error)]
pub enum GrpcServerError {
    #[error("Failed to parse address: {0}")]
    AddressParse(#[from] AddrParseError),
}

pub mod publish {
    tonic::include_proto!("publish");
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
        info!(target: LOG_TARGET, "We got a new publish file request: {:?}", request);
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
