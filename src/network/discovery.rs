use std::{
    collections::{HashMap, HashSet},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

use iroh::{EndpointAddr, EndpointId, address_lookup::DiscoveryEvent};
use n0_future::StreamExt;

use crate::network::{chat, codec::MoodPacket, friend, gossip, node, privacy};

#[derive(Debug, Clone)]
pub(crate) enum PeerEvent {
    SelfReady {
        node_id: String,
    },
    Discovered {
        node_id: String,
    },
    Expired {
        node_id: String,
    },
    PacketReceived {
        node_id: String,
        packet: MoodPacket,
    },
    DmReceived {
        node_id: String,
        from: String,
        body: String,
    },
    FriendRequestReceived {
        node_id: String,
        from_pet: String,
    },
    FriendAccepted {
        node_id: String,
        from_pet: String,
    },
    Connected {
        node_id: String,
    },
    Error {
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum PeerCommand {
    BroadcastMood(MoodPacket),
    SendDm { node_id: String, body: String },
    SendFriendRequest { node_id: String, from_pet: String },
    SendFriendAccept { node_id: String, from_pet: String },
    ConnectNode { node_id: String },
}

pub(crate) struct PeerNetworkHandle {
    pub events_rx: Receiver<PeerEvent>,
    pub cmd_tx: Sender<PeerCommand>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PeerNetworkOptions {
    pub enable_mdns: bool,
    pub enable_direct_nodeid_connect: bool,
}

pub(crate) fn start_peer_network_thread(options: PeerNetworkOptions) -> PeerNetworkHandle {
    let (events_tx, events_rx) = mpsc::channel::<PeerEvent>();
    let (cmd_tx, cmd_rx) = mpsc::channel::<PeerCommand>();

    thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(1)
            .build()
        {
            Ok(rt) => rt,
            Err(err) => {
                let _ = events_tx.send(PeerEvent::Error {
                    reason: format!("tokio runtime init failed: {err}"),
                });
                return;
            }
        };

        runtime.block_on(async move {
            if let Err(reason) = run_network(events_tx.clone(), cmd_rx, options).await {
                let _ = events_tx.send(PeerEvent::Error { reason });
            }
        });
    });

    PeerNetworkHandle { events_rx, cmd_tx }
}

async fn run_network(
    events_tx: Sender<PeerEvent>,
    cmd_rx: Receiver<PeerCommand>,
    options: PeerNetworkOptions,
) -> Result<(), String> {
    let peer_node = node::start_node(vec![
        gossip::GOSSIP_ALPN.to_vec(),
        chat::CHAT_ALPN.to_vec(),
        friend::FRIEND_ALPN.to_vec(),
    ])
    .await?;
    let endpoint = peer_node.endpoint.clone();
    let self_id = endpoint.id().to_string();
    let _ = events_tx.send(PeerEvent::SelfReady {
        node_id: self_id.clone(),
    });

    let accept_ep = endpoint.clone();
    let accept_events_tx = events_tx.clone();
    tokio::spawn(async move {
        run_accept_loop(accept_ep, accept_events_tx).await;
    });

    let mut known_addrs: HashMap<String, EndpointAddr> = HashMap::new();
    let mut known_nodes: HashSet<String> = HashSet::new();
    let mut mdns_events = if options.enable_mdns {
        Some(peer_node.mdns.subscribe().await)
    } else {
        None
    };

    loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                PeerCommand::BroadcastMood(packet) => {
                    if let Err(err) = privacy::validate_mood_packet(&packet) {
                        let _ = events_tx.send(PeerEvent::Error {
                            reason: format!("privacy validation failed (outbound): {err}"),
                        });
                        continue;
                    }
                    let bytes = match gossip::encode_packet(&packet) {
                        Ok(bytes) => bytes,
                        Err(err) => {
                            let _ = events_tx.send(PeerEvent::Error { reason: err });
                            continue;
                        }
                    };
                    for addr in known_addrs.values() {
                        let addr = addr.clone();
                        let ep = endpoint.clone();
                        let tx = events_tx.clone();
                        let payload = bytes.clone();
                        tokio::spawn(async move {
                            if let Err(err) =
                                send_packet(ep, addr, gossip::GOSSIP_ALPN, payload).await
                            {
                                let _ = tx.send(PeerEvent::Error {
                                    reason: format!("gossip send failed: {err}"),
                                });
                            }
                        });
                    }
                    for node_id in &known_nodes {
                        if !options.enable_direct_nodeid_connect {
                            continue;
                        }
                        let ep = endpoint.clone();
                        let tx = events_tx.clone();
                        let payload = bytes.clone();
                        let node_id = node_id.clone();
                        tokio::spawn(async move {
                            if let Err(err) =
                                send_packet_by_node_id(ep, &node_id, gossip::GOSSIP_ALPN, payload)
                                    .await
                            {
                                let _ = tx.send(PeerEvent::Error {
                                    reason: format!("gossip send failed to {node_id}: {err}"),
                                });
                            }
                        });
                    }
                }
                PeerCommand::SendDm { node_id, body } => {
                    let packet = chat::DmPacket {
                        version: 1,
                        from: self_id.clone(),
                        body,
                        ts_epoch: chrono::Utc::now().timestamp(),
                    };
                    let payload = match chat::encode_dm(&packet) {
                        Ok(p) => p,
                        Err(err) => {
                            let _ = events_tx.send(PeerEvent::Error { reason: err });
                            continue;
                        }
                    };
                    if let Some(addr) = known_addrs.get(&node_id).cloned() {
                        let ep = endpoint.clone();
                        let tx = events_tx.clone();
                        let direct_enabled = options.enable_direct_nodeid_connect;
                        let node_id_copy = node_id.clone();
                        tokio::spawn(async move {
                            if let Err(err) =
                                send_packet(ep.clone(), addr, chat::CHAT_ALPN, payload.clone())
                                    .await
                            {
                                if direct_enabled {
                                    if let Err(err2) = send_packet_by_node_id(
                                        ep,
                                        &node_id_copy,
                                        chat::CHAT_ALPN,
                                        payload,
                                    )
                                    .await
                                    {
                                        let _ = tx.send(PeerEvent::Error {
                                            reason: format!(
                                                "dm send failed via addr ({err}); fallback by node id failed for {node_id_copy}: {err2}"
                                            ),
                                        });
                                    }
                                } else {
                                    let _ = tx.send(PeerEvent::Error {
                                        reason: format!("dm send failed: {err}"),
                                    });
                                }
                            }
                        });
                    } else {
                        if !options.enable_direct_nodeid_connect {
                            let _ = events_tx.send(PeerEvent::Error {
                                reason: format!(
                                    "dm send failed: unknown peer {node_id} and direct connect disabled"
                                ),
                            });
                            continue;
                        }
                        let ep = endpoint.clone();
                        let tx = events_tx.clone();
                        let node_id_copy = node_id.clone();
                        tokio::spawn(async move {
                            if let Err(err) =
                                send_packet_by_node_id(ep, &node_id_copy, chat::CHAT_ALPN, payload)
                                    .await
                            {
                                let _ = tx.send(PeerEvent::Error {
                                    reason: format!("dm send failed to {node_id_copy}: {err}"),
                                });
                            }
                        });
                    }
                }
                PeerCommand::SendFriendRequest { node_id, from_pet } => {
                    let packet = friend::FriendPacket {
                        version: 1,
                        from: self_id.clone(),
                        from_pet,
                        kind: friend::FriendPacketKind::Request,
                        ts_epoch: chrono::Utc::now().timestamp(),
                    };
                    let payload = match friend::encode_friend(&packet) {
                        Ok(p) => p,
                        Err(err) => {
                            let _ = events_tx.send(PeerEvent::Error { reason: err });
                            continue;
                        }
                    };
                    if let Some(addr) = known_addrs.get(&node_id).cloned() {
                        let ep = endpoint.clone();
                        let tx = events_tx.clone();
                        tokio::spawn(async move {
                            if let Err(err) =
                                send_packet(ep, addr, friend::FRIEND_ALPN, payload).await
                            {
                                let _ = tx.send(PeerEvent::Error {
                                    reason: format!("friend request send failed: {err}"),
                                });
                            }
                        });
                    } else if options.enable_direct_nodeid_connect {
                        let ep = endpoint.clone();
                        let tx = events_tx.clone();
                        let node_id_copy = node_id.clone();
                        tokio::spawn(async move {
                            if let Err(err) = send_packet_by_node_id(
                                ep,
                                &node_id_copy,
                                friend::FRIEND_ALPN,
                                payload,
                            )
                            .await
                            {
                                let _ = tx.send(PeerEvent::Error {
                                    reason: format!(
                                        "friend request send failed to {node_id_copy}: {err}"
                                    ),
                                });
                            }
                        });
                    }
                }
                PeerCommand::SendFriendAccept { node_id, from_pet } => {
                    let packet = friend::FriendPacket {
                        version: 1,
                        from: self_id.clone(),
                        from_pet,
                        kind: friend::FriendPacketKind::Accept,
                        ts_epoch: chrono::Utc::now().timestamp(),
                    };
                    let payload = match friend::encode_friend(&packet) {
                        Ok(p) => p,
                        Err(err) => {
                            let _ = events_tx.send(PeerEvent::Error { reason: err });
                            continue;
                        }
                    };
                    if let Some(addr) = known_addrs.get(&node_id).cloned() {
                        let ep = endpoint.clone();
                        let tx = events_tx.clone();
                        tokio::spawn(async move {
                            if let Err(err) =
                                send_packet(ep, addr, friend::FRIEND_ALPN, payload).await
                            {
                                let _ = tx.send(PeerEvent::Error {
                                    reason: format!("friend accept send failed: {err}"),
                                });
                            }
                        });
                    } else if options.enable_direct_nodeid_connect {
                        let ep = endpoint.clone();
                        let tx = events_tx.clone();
                        let node_id_copy = node_id.clone();
                        tokio::spawn(async move {
                            if let Err(err) = send_packet_by_node_id(
                                ep,
                                &node_id_copy,
                                friend::FRIEND_ALPN,
                                payload,
                            )
                            .await
                            {
                                let _ = tx.send(PeerEvent::Error {
                                    reason: format!(
                                        "friend accept send failed to {node_id_copy}: {err}"
                                    ),
                                });
                            }
                        });
                    }
                }
                PeerCommand::ConnectNode { node_id } => {
                    if !options.enable_direct_nodeid_connect {
                        let _ = events_tx.send(PeerEvent::Error {
                            reason: "direct nodeid connect is disabled in config".to_string(),
                        });
                        continue;
                    }
                    known_nodes.insert(node_id.clone());
                    let ep = endpoint.clone();
                    let tx = events_tx.clone();
                    tokio::spawn(async move {
                        match probe_connect_node(ep, &node_id, gossip::GOSSIP_ALPN).await {
                            Ok(()) => {
                                let _ = tx.send(PeerEvent::Connected { node_id });
                            }
                            Err(err) => {
                                let _ = tx.send(PeerEvent::Error {
                                    reason: format!("connect failed for {node_id}: {err}"),
                                });
                            }
                        }
                    });
                }
            }
        }

        if let Some(events) = mdns_events.as_mut() {
            tokio::select! {
                maybe_event = events.next() => {
                    match maybe_event {
                        Some(DiscoveryEvent::Discovered { endpoint_info, .. }) => {
                            let node_id = endpoint_info.endpoint_id.to_string();
                            if node_id != self_id {
                                known_addrs.insert(node_id.clone(), endpoint_info.to_endpoint_addr());
                                known_nodes.insert(node_id.clone());
                                let _ = events_tx.send(PeerEvent::Discovered { node_id });
                            }
                        }
                        Some(DiscoveryEvent::Expired { endpoint_id }) => {
                            let node_id = endpoint_id.to_string();
                            known_addrs.remove(&node_id);
                            known_nodes.remove(&node_id);
                            if node_id != self_id {
                                let _ = events_tx.send(PeerEvent::Expired { node_id });
                            }
                        }
                        None => break,
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(120)) => {}
            }
        } else {
            tokio::time::sleep(Duration::from_millis(120)).await;
        }
    }

    Ok(())
}

async fn probe_connect_node(
    endpoint: iroh::Endpoint,
    node_id: &str,
    alpn: &[u8],
) -> Result<(), String> {
    let endpoint_id = node_id
        .parse::<EndpointId>()
        .map_err(|e| format!("invalid node id '{node_id}': {e}"))?;
    let conn = endpoint
        .connect(endpoint_id, alpn)
        .await
        .map_err(|e| format!("connect failed: {e}"))?;
    conn.close(0u8.into(), b"probe");
    Ok(())
}

async fn run_accept_loop(endpoint: iroh::Endpoint, events_tx: Sender<PeerEvent>) {
    loop {
        let Some(incoming) = endpoint.accept().await else {
            return;
        };
        let events_tx = events_tx.clone();
        tokio::spawn(async move {
            let Ok(connecting) = incoming.accept() else {
                return;
            };
            let Ok(conn) = connecting.await else {
                return;
            };
            let node_id = conn.remote_id().to_string();
            let alpn = conn.alpn().to_vec();
            loop {
                let Ok(mut uni) = conn.accept_uni().await else {
                    break;
                };
                let Ok(payload) = uni.read_to_end(4096).await else {
                    continue;
                };
                if alpn.as_slice() == gossip::GOSSIP_ALPN {
                    if let Ok(packet) = gossip::decode_packet(&payload) {
                        if privacy::validate_mood_packet(&packet).is_ok() {
                            let _ = events_tx.send(PeerEvent::PacketReceived {
                                node_id: node_id.clone(),
                                packet,
                            });
                        } else {
                            let _ = events_tx.send(PeerEvent::Error {
                                reason: format!(
                                    "privacy validation failed (inbound) from {}",
                                    node_id
                                ),
                            });
                        }
                    }
                } else if alpn.as_slice() == chat::CHAT_ALPN
                    && let Ok(dm) = chat::decode_dm(&payload)
                {
                    let _ = events_tx.send(PeerEvent::DmReceived {
                        node_id: node_id.clone(),
                        from: dm.from,
                        body: dm.body,
                    });
                } else if alpn.as_slice() == friend::FRIEND_ALPN
                    && let Ok(pkt) = friend::decode_friend(&payload)
                {
                    match pkt.kind {
                        friend::FriendPacketKind::Request => {
                            let _ = events_tx.send(PeerEvent::FriendRequestReceived {
                                node_id: node_id.clone(),
                                from_pet: pkt.from_pet,
                            });
                        }
                        friend::FriendPacketKind::Accept => {
                            let _ = events_tx.send(PeerEvent::FriendAccepted {
                                node_id: node_id.clone(),
                                from_pet: pkt.from_pet,
                            });
                        }
                    }
                }
            }
        });
    }
}

async fn send_packet(
    endpoint: iroh::Endpoint,
    addr: EndpointAddr,
    alpn: &[u8],
    payload: Vec<u8>,
) -> Result<(), String> {
    let conn = endpoint
        .connect(addr, alpn)
        .await
        .map_err(|e| format!("connect failed: {e}"))?;
    let mut stream = conn
        .open_uni()
        .await
        .map_err(|e| format!("open uni failed: {e}"))?;
    stream
        .write_all(&payload)
        .await
        .map_err(|e| format!("write failed: {e}"))?;
    stream
        .finish()
        .map_err(|e| format!("stream finish failed: {e}"))?;
    let _ = stream.stopped().await;
    Ok(())
}

async fn send_packet_by_node_id(
    endpoint: iroh::Endpoint,
    node_id: &str,
    alpn: &[u8],
    payload: Vec<u8>,
) -> Result<(), String> {
    let endpoint_id = node_id
        .parse::<EndpointId>()
        .map_err(|e| format!("invalid node id '{node_id}': {e}"))?;
    let conn = endpoint
        .connect(endpoint_id, alpn)
        .await
        .map_err(|e| format!("connect failed: {e}"))?;
    let mut stream = conn
        .open_uni()
        .await
        .map_err(|e| format!("open uni failed: {e}"))?;
    stream
        .write_all(&payload)
        .await
        .map_err(|e| format!("write failed: {e}"))?;
    stream
        .finish()
        .map_err(|e| format!("stream finish failed: {e}"))?;
    let _ = stream.stopped().await;
    Ok(())
}
