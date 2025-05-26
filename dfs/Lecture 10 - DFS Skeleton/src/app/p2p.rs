use async_trait::async_trait;
use libp2p::{dcutr, gossipsub, identify, kad::{self, store::MemoryStore}, mdns, ping, relay, request_response::cbor, swarm::NetworkBehaviour};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::{Error, Service};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileChunkRequest {
    pub file_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileChunkResponse {
    pub data: Vec<u8>,
}

#[derive(NetworkBehaviour)]
pub struct P2pNetworkBehaviour {
    ping: ping::Behaviour,
    identify: identify::Behaviour,
    mdns: mdns::Behaviour<mdns::tokio::Tokio>,
    kademlia: kad::Behaviour<MemoryStore>,
    gossipsub: gossipsub::Behaviour,
    relay_server: relay::Behaviour,
    relay_client: relay::client::Behaviour,
    dcutr: dcutr::Behaviour,
    file_download: cbor::Behaviour<FileChunkRequest, FileChunkResponse>,
}

pub struct P2pService {
    
}

impl P2pService {
    pub fn new() -> Self {
        Self{}
    }
}

#[async_trait]
impl Service for P2pService {
    async fn start(&self, cancel_token: CancellationToken) -> Result<(), Error> {
        // TODO: implement
        Ok(())
    }
}