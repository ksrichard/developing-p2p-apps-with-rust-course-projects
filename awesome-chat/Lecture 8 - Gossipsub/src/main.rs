use anyhow::anyhow;
use libp2p::futures::StreamExt;
use libp2p::kad::store::MemoryStore;
use libp2p::kad::Mode;
use libp2p::mdns::tokio::Tokio;
use libp2p::multiaddr::Protocol;
use libp2p::ping::Config;
use libp2p::request_response::json;
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{autonat, dcutr, gossipsub, identify, kad, mdns, noise, ping, relay, request_response, tcp, yamux, Multiaddr, PeerId, StreamProtocol};
use serde::{Deserialize, Serialize};
use std::env;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use libp2p::gossipsub::{IdentTopic, MessageAuthenticity, Topic, TopicHash, ValidationMode};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::{io, select};

const CHAT_TOPIC: &str = "chat";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MessageRequest {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MessageResponse {
    pub ack: bool,
}

#[derive(NetworkBehaviour)]
struct ChatBehaviour {
    ping: ping::Behaviour,
    messaging: json::Behaviour<MessageRequest, MessageResponse>,
    mdns: Toggle<mdns::Behaviour<Tokio>>,
    identify: identify::Behaviour,
    kademlia: kad::Behaviour<MemoryStore>,
    autonat: autonat::Behaviour,
    relay_server: relay::Behaviour,
    relay_client: relay::client::Behaviour,
    dcutr: dcutr::Behaviour,
    gossipsub: gossipsub::Behaviour,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let chat_topic = IdentTopic::new(CHAT_TOPIC);
    let mdns_enabled = env::var("CHAT_MDNS_ENABLED")?.parse::<bool>()?;
    let bootstrap_peers = env::var("CHAT_BOOTSTRAP_PEERS")
        .map(|peers| peers.split(',').map(|s| s.to_string()).collect::<Vec<String>>());

    let mut swarm =
        libp2p::SwarmBuilder::with_new_identity()
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_relay_client(
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_behaviour(|key_pair, relay_client| {
                let mdns = if mdns_enabled {
                    Toggle::from(Some(mdns::Behaviour::new(mdns::Config::default(), key_pair.public().to_peer_id())?))
                } else {
                    Toggle::from(None)
                };

                let mut kad_config = kad::Config::new(StreamProtocol::new("/awesome-chat/kad/1.0.0"));
                kad_config.set_periodic_bootstrap_interval(Some(Duration::from_secs(10)));

                let gossipsub_config = gossipsub::ConfigBuilder::default()
                    .heartbeat_interval(Duration::from_secs(10))
                    .validation_mode(ValidationMode::Strict)
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
                    ChatBehaviour {
                        ping: ping::Behaviour::new(Config::new().with_interval(Duration::from_secs(10))),
                        messaging: json::Behaviour::new(
                            [(
                                StreamProtocol::new("/awesome-chat/1"),
                                request_response::ProtocolSupport::Full,
                            )],
                            request_response::Config::default(),
                        ),
                        mdns,
                        identify: identify::Behaviour::new(identify::Config::new("1.0.0".to_string(), key_pair.public())),
                        kademlia: kad::Behaviour::with_config(
                            key_pair.public().to_peer_id(),
                            MemoryStore::new(key_pair.public().to_peer_id()),
                            kad_config,
                        ),
                        autonat: autonat::Behaviour::new(key_pair.public().to_peer_id(), autonat::Config::default()),
                        relay_server: relay::Behaviour::new(key_pair.public().to_peer_id(), relay::Config::default()),
                        relay_client,
                        dcutr: dcutr::Behaviour::new(key_pair.public().to_peer_id()),
                        gossipsub: gossipsub::Behaviour::new(
                            MessageAuthenticity::Signed(key_pair.clone()),
                            gossipsub_config,
                        )?,
                    }
                )
            })?
            .with_swarm_config(|config| { config.with_idle_connection_timeout(Duration::from_secs(30)) })
            .build();

    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    swarm.behaviour_mut().kademlia.set_mode(Some(Mode::Server));

    println!("Peer ID: {:?}", swarm.local_peer_id());

    // initialize bootstrap peers
    if let Ok(bootstrap_peers) = bootstrap_peers {
        for bootstrap_peer in bootstrap_peers {
            let addr: Multiaddr = bootstrap_peer.parse()?;
            let peer_id = addr.iter().map(|addr_str| {
                if let Some(Protocol::P2p(peer_id)) = addr.iter().last() {
                    return Some(peer_id);
                }
                None
            })
                .filter(Option::is_some)
                .last()
                .ok_or(anyhow!("No Peer ID found in address!"))?
                .ok_or(anyhow!("Bootstrap peer address {bootstrap_peer} is wrong!"))?;
            swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
            swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
        }
    }

    // subscribe to our chat room
    swarm.behaviour_mut().gossipsub.subscribe(&chat_topic)?;

    let mut stdin = BufReader::new(io::stdin()).lines();

    loop {
        select! {
            event = swarm.select_next_some() => match event {
                SwarmEvent::NewListenAddr { address, .. } => {
                    println!("Listening on {:?}", address);
                }
                SwarmEvent::ConnectionEstablished{peer_id, ..} => {
                    println!("Connection established with peer {:?}!", peer_id);
                }
                SwarmEvent::Behaviour(event) => match event {
                    ChatBehaviourEvent::Ping(event) => {
                        println!("Ping: {:?}", event);
                    },
                    ChatBehaviourEvent::Messaging(event) => match event {
                        request_response::Event::Message{peer,message  } => match message {
                            request_response::Message::Request{request_id,request,channel  } => {
                                println!("{peer} {:?}", request.message);
                                if let Err(error) = swarm.behaviour_mut().messaging.send_response(channel, MessageResponse { ack: true }) {
                                    println!("Error sending response: {:?}", error);
                                }
                            }
                            request_response::Message::Response{request_id,response  } => {
                                println!("{peer} Response ACK: {:?}", response.ack);
                            },
                        },
                        request_response::Event::OutboundFailure{peer,request_id,error  } => {
                            println!("OutboundFailure from {:?} to {:?}: {:?}", peer, request_id, error);
                        },
                        request_response::Event::InboundFailure{peer,request_id,error  } => {
                            println!("InboundFailure from {:?} to {:?}: {:?}", peer, request_id, error);
                        },
                        request_response::Event::ResponseSent{ .. } => {},
                    },
                    ChatBehaviourEvent::Mdns(event) => match event {
                        mdns::Event::Discovered(new_peers) => {
                            for (peer_id, addr) in new_peers {
                                println!("Discovered {peer_id} at {addr}!");
                                swarm.add_peer_address(peer_id, addr.clone());
                                swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
                                swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                            }
                        }
                        mdns::Event::Expired(_) => {}
                    },
                    ChatBehaviourEvent::Identify(event) => match event {
                        identify::Event::Received{connection_id,peer_id,info  } => {
                            println!("New identify received: {peer_id} - {info:?}");
                            let is_relay = info.protocols.iter().any(|protocol| *protocol == relay::HOP_PROTOCOL_NAME);
                            
                            for addr in info.listen_addrs {
                                swarm.behaviour_mut().kademlia.add_address(&peer_id, addr.clone());
                                swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                                
                                if is_relay
                                {
                                    let listen_addr = addr.clone().with_p2p(peer_id).unwrap().with(Protocol::P2pCircuit);
                                    println!("Trying to listen on {:?}", listen_addr);
                                    swarm.listen_on(listen_addr)?;
                                }
                            }
                        },
                        identify::Event::Sent{ .. } => {},
                        identify::Event::Pushed{ .. } => {}
                        identify::Event::Error{ .. } => {},
                    },
                    ChatBehaviourEvent::Kademlia(event) => match event {
                        kad::Event::InboundRequest{ .. } => {},
                        kad::Event::OutboundQueryProgressed{ .. } => {},
                        kad::Event::RoutingUpdated{peer,is_new_peer,addresses,bucket_range,old_peer  } => {
                            println!("New routing update! Peer: {peer} - {addresses:?}");
                            // addresses.iter().for_each(|addr| {
                            //     if let Err(error) = swarm.dial(addr.clone()) {
                            //         println!("Error dialing address {:?}: {:?}", addr, error);
                            //     }
                            // })
                        },
                        kad::Event::UnroutablePeer{ .. } => {},
                        kad::Event::RoutablePeer{ .. } => {},
                        kad::Event::PendingRoutablePeer{ .. } => {},
                        kad::Event::ModeChanged{ .. } => {}},
                    ChatBehaviourEvent::Autonat(event) => match event {
                        autonat::Event::InboundProbe(event) => {
                            println!("Inbound probe: {:?}", event);
                        },
                        autonat::Event::OutboundProbe(event) => {
                            println!("Outbound probe: {:?}", event);
                        },
                        autonat::Event::StatusChanged{old,new} => {
                            println!("Status changed from {:?} to {:?}", old, new);
                        }
                    },
                    ChatBehaviourEvent::RelayServer(event) => {
                        println!("Relay server: {:?}", event);
                    },
                    ChatBehaviourEvent::RelayClient(event) => {
                        println!("Relay client: {:?}", event);
                    },
                    ChatBehaviourEvent::Dcutr(event) => {
                        println!("Dcutr: {:?}", event);
                    },
                    ChatBehaviourEvent::Gossipsub(event) => {
                        println!("Gossipsub: {event:?}");
                    }
                }
                _ => {}
            },
            Ok(Some(line)) = stdin.next_line() => {
                match swarm.behaviour_mut().gossipsub.publish(chat_topic.clone(), line.as_bytes()) {
                    Ok(_) => {
                        println!("{} {line:?}", swarm.local_peer_id());
                    }
                    Err(error) => {
                        println!("Error publishing to chat: {:?}", error);
                    }
                }
                // let peers: Vec<PeerId> = swarm.connected_peers().copied().collect();
                // for peer_id in peers {
                    // swarm.behaviour_mut().messaging.send_request(&peer_id, MessageRequest { message: line.clone() });
                    // println!("{} {line:?}", swarm.local_peer_id());
                // }
            },
        }
    }
}
