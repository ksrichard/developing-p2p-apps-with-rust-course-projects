use core::error;
use std::{hash::{DefaultHasher, Hash, Hasher}, path::{Path, PathBuf}, time::{Duration, SystemTime, UNIX_EPOCH}};

use async_trait::async_trait;
use libp2p::{dcutr, gossipsub, identify, identity::{DecodingError, Keypair}, kad::{self, store::MemoryStore}, mdns, noise, ping, relay, request_response::{self, cbor}, swarm::NetworkBehaviour, tcp, yamux, StreamProtocol, Swarm};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io;
use tokio_util::sync::CancellationToken;

use crate::app::{ServerError, Service};

use super::{config::P2pServiceConfig};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileChunkRequest {
    pub file_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileChunkResponse {
    pub data: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum P2pNetworkError {
    #[error("Failed to get directory of the keypair file: {0}")]
    FailedToGetKeypairFileDir(PathBuf),
    #[error("I/O error: {0}")]
    IO(#[from] io::Error),
    #[error("Keypair decoding error: {0}")]
    KeypairDecoding(#[from] DecodingError),
    #[error("Libp2p noise error: {0}")]
    Libp2pNoise(#[from] libp2p::noise::Error),
    #[error("Libp2p swarm builder error: {0}")]
    Libp2pSwarmBuilder(String),
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

#[derive(Debug)]
pub struct P2pService {
    config: P2pServiceConfig,
}

impl P2pService {
    pub fn new(config: P2pServiceConfig) -> Self {
        Self{
            config
        }
    }

    async fn keypair(&self) -> Result<Keypair, P2pNetworkError> {
        match tokio::fs::read(&self.config.keypair_file).await {
            Ok(data) => {
                Ok(
                    Keypair::from_protobuf_encoding(data.as_slice())?
                )
            },
            Err(_) => {
                let keypair = Keypair::generate_ed25519();
                let encoded = keypair.to_protobuf_encoding()?;
                let dir = self.config.keypair_file.parent()
                .ok_or(P2pNetworkError::FailedToGetKeypairFileDir(self.config.keypair_file.to_path_buf()))?;
                let _ = tokio::fs::remove_file(&self.config.keypair_file).await;
                tokio::fs::create_dir_all(dir).await?;
                tokio::fs::write(&self.config.keypair_file, encoded).await?;
                Ok(keypair)
            },
        }
    }

    async fn swarm(&self) -> Result<Swarm<P2pNetworkBehaviour>, P2pNetworkError> {
        let keypair = self.keypair().await?;
        let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
             noise::Config::new,
              yamux::Config::default)?
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(|key_pair, relay_client| {
            // kademlia config
            let mut kad_config = kad::Config::new(StreamProtocol::new("/dfs/1.0.0/kad"));
            kad_config.set_periodic_bootstrap_interval(Some(Duration::from_secs(30)));

            // gossipsub config
            let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(Duration::from_secs(10))
            .validation_mode(gossipsub::ValidationMode::Strict)
            .message_id_fn(|message| {
                let mut hasher = DefaultHasher::new();
                message.data.hash(&mut hasher);
                message.topic.hash(&mut hasher);
                if let Some(peer_id) = message.source {
                    peer_id.hash(&mut hasher);
                }
                let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
                now.to_string().hash(&mut hasher);
                gossipsub::MessageId::from(hasher.finish().to_string())
            })
            .build()?;

            Ok(
                P2pNetworkBehaviour {
                    ping: ping::Behaviour::new(ping::Config::default()),
                    identify: identify::Behaviour::new(
                        identify::Config::new("/dfs/1.0.0".to_string(), key_pair.public()),
                    ),
                    mdns: mdns::Behaviour::<mdns::tokio::Tokio>::new(
                        mdns::Config::default(), 
                        key_pair.public().to_peer_id(),
                    )?,
                    kademlia: kad::Behaviour::with_config(
                        key_pair.public().to_peer_id(), 
                        MemoryStore::new(key_pair.public().to_peer_id()),
                        kad_config,
                    ),
                    gossipsub: gossipsub::Behaviour::new(
                        gossipsub::MessageAuthenticity::Signed(key_pair.clone()),
                        gossipsub_config,
                    )?,
                    relay_server: relay::Behaviour::new(key_pair.public().to_peer_id(), relay::Config::default()),
                    relay_client,
                    dcutr: dcutr::Behaviour::new(key_pair.public().to_peer_id()),
                    file_download: cbor::Behaviour::new([(
                        StreamProtocol::new("/dfs/1.0.0/file-download"),
                        request_response::ProtocolSupport::Full,
                    )], request_response::Config::default(),
                ),
                }
            )
        }).map_err(|error| P2pNetworkError::Libp2pSwarmBuilder(error.to_string()))?
        .with_swarm_config(|config| config.with_idle_connection_timeout(Duration::from_secs(30)))
        .build();
    
    Ok(swarm)
}
}

#[async_trait]
impl Service for P2pService {
    async fn start(&self, cancel_token: CancellationToken) -> Result<(), ServerError> {
        let swarm = self.swarm().await?;

        // TODO: implement rest

        Ok(())
    }
}