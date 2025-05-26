use core::error;
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use libp2p::{
    dcutr,
    futures::StreamExt,
    gossipsub::{self, IdentTopic, SubscriptionError},
    identify::{self, Info},
    identity::{DecodingError, Keypair},
    kad::{self, store::MemoryStore, Mode, Record},
    mdns,
    multiaddr::{self, Protocol},
    noise, ping, relay,
    request_response::{self, cbor},
    swarm::NetworkBehaviour,
    tcp, yamux, Multiaddr, PeerId, StreamProtocol, Swarm, TransportError,
};
use log::{debug, error, info, warn};
use rs_sha256::Sha256Hasher;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{io, select, sync::mpsc};
use tokio_util::sync::CancellationToken;

use crate::{
    app::{models::PublishedFile, ServerError, Service},
    file_processor::FileProcessResult,
};

use super::config::P2pServiceConfig;

const LOG_TARGET: &str = "app::p2p::P2pService";

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
    #[error("Parsing libp2p multiaddress error: {0}")]
    Libp2pMultiAddrParse(#[from] multiaddr::Error),
    #[error("Libp2p transport error: {0}")]
    Libp2pTransport(#[from] TransportError<io::Error>),
    #[error("Libp2p gossipsub subscription error: {0}")]
    Libp2pGossipsubSubscription(#[from] SubscriptionError),
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
    file_publish_rx: mpsc::Receiver<FileProcessResult>,
}

impl P2pService {
    pub fn new(
        config: P2pServiceConfig,
        file_publish_rx: mpsc::Receiver<FileProcessResult>,
    ) -> Self {
        Self {
            config,
            file_publish_rx,
        }
    }

    async fn keypair(&self) -> Result<Keypair, P2pNetworkError> {
        match tokio::fs::read(&self.config.keypair_file).await {
            Ok(data) => Ok(Keypair::from_protobuf_encoding(data.as_slice())?),
            Err(_) => {
                let keypair = Keypair::generate_ed25519();
                let encoded = keypair.to_protobuf_encoding()?;
                let dir = self.config.keypair_file.parent().ok_or(
                    P2pNetworkError::FailedToGetKeypairFileDir(
                        self.config.keypair_file.to_path_buf(),
                    ),
                )?;
                let _ = tokio::fs::remove_file(&self.config.keypair_file).await;
                tokio::fs::create_dir_all(dir).await?;
                tokio::fs::write(&self.config.keypair_file, encoded).await?;
                Ok(keypair)
            }
        }
    }

    /// Creating swarm with all configurations
    async fn swarm(&self) -> Result<Swarm<P2pNetworkBehaviour>, P2pNetworkError> {
        let keypair = self.keypair().await?;
        let swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_quic()
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
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_millis();
                        now.to_string().hash(&mut hasher);
                        gossipsub::MessageId::from(hasher.finish().to_string())
                    })
                    .build()?;

                Ok(P2pNetworkBehaviour {
                    ping: ping::Behaviour::new(ping::Config::default()),
                    identify: identify::Behaviour::new(identify::Config::new(
                        "/dfs/1.0.0".to_string(),
                        key_pair.public(),
                    )),
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
                    relay_server: relay::Behaviour::new(
                        key_pair.public().to_peer_id(),
                        relay::Config::default(),
                    ),
                    relay_client,
                    dcutr: dcutr::Behaviour::new(key_pair.public().to_peer_id()),
                    file_download: cbor::Behaviour::new(
                        [(
                            StreamProtocol::new("/dfs/1.0.0/file-download"),
                            request_response::ProtocolSupport::Full,
                        )],
                        request_response::Config::default(),
                    ),
                })
            })
            .map_err(|error| P2pNetworkError::Libp2pSwarmBuilder(error.to_string()))?
            .with_swarm_config(|config| {
                config.with_idle_connection_timeout(Duration::from_secs(30))
            })
            .build();

        Ok(swarm)
    }

    fn handle_identify_received(
        &self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        peer_id: PeerId,
        info: Info,
    ) -> Result<(), ServerError> {
        let is_relay = info
            .protocols
            .iter()
            .any(|protocol| *protocol == relay::HOP_PROTOCOL_NAME);

        for addr in info.listen_addrs {
            swarm
                .behaviour_mut()
                .kademlia
                .add_address(&peer_id, addr.clone());
            swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);

            if is_relay {
                let listen_addr = addr
                    .clone()
                    .with_p2p(peer_id)
                    .unwrap()
                    .with(Protocol::P2pCircuit);
                info!(target: LOG_TARGET, "Trying to listen on relay with address {}", listen_addr);
                if let Err(error) = swarm.listen_on(listen_addr.clone()) {
                    warn!(target: LOG_TARGET, "Failed to listen on relay ({listen_addr}): {error}");
                }
            }
        }

        Ok(())
    }

    fn handle_mdns_discovered(
        &self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        new_peers: Vec<(PeerId, Multiaddr)>,
    ) {
        for (peer_id, addr) in new_peers {
            info!(target: LOG_TARGET, "[mDNS] Discovered {peer_id} at {addr}");
            swarm.add_peer_address(peer_id, addr.clone());
            swarm
                .behaviour_mut()
                .kademlia
                .add_address(&peer_id, addr.clone());
            swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
        }
    }

    fn handle_file_download_message(
        &self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        peer_id: PeerId,
        message: libp2p::request_response::Message<FileChunkRequest, FileChunkResponse>,
    ) {
        match message {
            request_response::Message::Request {
                request_id,
                request,
                channel,
            } => {
                info!(target: LOG_TARGET, "File download request: {request:?}");
                // TODO: implement
            }
            request_response::Message::Response {
                request_id,
                response,
            } => {
                info!(target: LOG_TARGET, "File download response received: {response:?}");
                // TODO: implement
            }
        }
    }

    fn handle_gossipsub_message(
        &self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        message: libp2p::gossipsub::Message,
    ) {
        info!(target: LOG_TARGET, "[gossipsub] New message: {message:?}");
        // TODO: implement
    }

    fn handle_file_publish(
        &self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        file_process_result: FileProcessResult,
    ) {
        info!(target: LOG_TARGET, "P2P new file publish request received ({}) with {} chunks", file_process_result.original_file_name, file_process_result.number_of_chunks);
        let mut hasher = Sha256Hasher::default();
        file_process_result.hash(&mut hasher);
        let raw_key = hasher.finish();
        info!(target: LOG_TARGET, "New file key {} on DHT: {}", file_process_result.original_file_name, raw_key);
        let key = raw_key.to_be_bytes().to_vec();
        match serde_cbor::to_vec(&PublishedFile::new(
            file_process_result.number_of_chunks,
            file_process_result.merkle_root,
        )) {
            Ok(value) => {
                let record = Record::new(key, value);
                let record_key = record.key.clone();
                if let Err(error) = swarm
                    .behaviour_mut()
                    .kademlia
                    .put_record(record, kad::Quorum::Majority)
                {
                    error!(target: LOG_TARGET, "Failed to put a new record to DHT: {error}");
                }
                if let Err(error) = swarm.behaviour_mut().kademlia.start_providing(record_key) {
                    error!(target: LOG_TARGET, "Failed to start providing a new record to DHT: {error}");
                }
                // TODO: putting all chunks as new records and start providing them (the same should be done at other peers who are downloaded a chunk)
                // TODO: start publishing new file periodically to other peers via gossipsub if file_process_result.public == true
            }
            Err(error) => {
                error!(target: LOG_TARGET, "Failed to convert file process result: {error:?}")
            }
        }
    }

    fn log_debug<T: std::fmt::Debug>(&self, event: T) {
        debug!(target: LOG_TARGET, "{:?}", event);
    }

    fn log_info<T: std::fmt::Debug>(&self, event: T) {
        debug!(target: LOG_TARGET, "{:?}", event);
    }
}

#[async_trait]
impl Service for P2pService {
    async fn start(&mut self, cancel_token: CancellationToken) -> Result<(), ServerError> {
        let mut swarm = self.swarm().await?;
        swarm
            .listen_on("/ip4/0.0.0.0/tcp/0".parse().map_err(|error| {
                ServerError::P2pNetwork(P2pNetworkError::Libp2pMultiAddrParse(error))
            })?)
            .map_err(|error| ServerError::P2pNetwork(P2pNetworkError::Libp2pTransport(error)))?;
        swarm
            .listen_on("/ip4/0.0.0.0/udp/0/quic-v1".parse().map_err(|error| {
                ServerError::P2pNetwork(P2pNetworkError::Libp2pMultiAddrParse(error))
            })?)
            .map_err(|error| ServerError::P2pNetwork(P2pNetworkError::Libp2pTransport(error)))?;

        swarm.behaviour_mut().kademlia.set_mode(Some(Mode::Server));

        info!(target: LOG_TARGET, "Peer ID: {}", swarm.local_peer_id());

        let file_owners_topic = IdentTopic::new("available_files");
        swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&file_owners_topic)
            .map_err(|error| {
                ServerError::P2pNetwork(P2pNetworkError::Libp2pGossipsubSubscription(error))
            })?;

        // TODO: add bootstrap peers

        loop {
            select! {
                event = swarm.select_next_some() => match event {
                    libp2p::swarm::SwarmEvent::Behaviour(event) => match event {
                        P2pNetworkBehaviourEvent::Identify(event) => match event {
                            identify::Event::Received { connection_id: _connection_id, peer_id, info } => self.handle_identify_received(&mut swarm, peer_id, info)?,
                            _ => self.log_debug(event),
                        },
                        P2pNetworkBehaviourEvent::Mdns(event) => match event {
                            mdns::Event::Discovered(new_peers) => self.handle_mdns_discovered(&mut swarm, new_peers),
                            _ => self.log_debug(event),
                        },
                        P2pNetworkBehaviourEvent::Kademlia(event) => self.log_info(event),
                        P2pNetworkBehaviourEvent::Gossipsub(event) => match event {
                            gossipsub::Event::Message { propagation_source: _propagation_source, message_id: _message_id, message } => self.handle_gossipsub_message(&mut swarm, message),
                            _ => self.log_debug(event),
                        },
                        P2pNetworkBehaviourEvent::RelayServer(event) => self.log_debug(event),
                        P2pNetworkBehaviourEvent::RelayClient(event) => self.log_debug(event),
                        P2pNetworkBehaviourEvent::Dcutr(event) => self.log_debug(event),
                        P2pNetworkBehaviourEvent::FileDownload(event) => match event {
                            request_response::Event::Message { peer, message } => self.handle_file_download_message(&mut swarm, peer, message),
                            _ => self.log_debug(event),
                        },
                        _ => self.log_debug(event),
                    },
                    libp2p::swarm::SwarmEvent::NewListenAddr { listener_id: _listener_id, address } => {
                        info!(target: LOG_TARGET, "Listening on {}", address);
                    },
                    _ => self.log_debug(event),
                },
                file_publish_result = self.file_publish_rx.recv() => {
                    if let Some(new_file_publish) = file_publish_result {
                        self.handle_file_publish(&mut swarm, new_file_publish);
                    }
                },
                _ = cancel_token.cancelled() => {
                    info!(target: LOG_TARGET, "P2P networking service shutting down...");
                    break;
                }
            }
        }

        Ok(())
    }
}
