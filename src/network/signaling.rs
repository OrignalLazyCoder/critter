#![cfg(feature = "web")]

use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use axum::{
    Router,
    extract::{
        Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::{IntoResponse, Response},
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, mpsc};

#[derive(Debug, Clone)]
pub struct SignalingNodeOptions {
    pub host: String,
    pub port: u16,
    pub password: Option<String>,
}

type PeerMap = Arc<RwLock<HashMap<String, mpsc::UnboundedSender<Message>>>>;

#[derive(Clone)]
struct AppState {
    peers: PeerMap,
    password: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WsQuery {
    peer: Option<String>,
    token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SignalMessage {
    to: String,
    kind: String,
    #[serde(default)]
    payload: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct RelayMessage<'a> {
    from: &'a str,
    kind: &'a str,
    payload: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct ErrorMessage<'a> {
    r#type: &'a str,
    reason: &'a str,
}

pub fn run_signaling_node(options: SignalingNodeOptions) -> Result<(), Box<dyn std::error::Error>> {
    let host = options.host.clone();
    let addr: SocketAddr = format!("{}:{}", host, options.port).parse()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let state = AppState {
            peers: Arc::new(RwLock::new(HashMap::new())),
            password: options.password,
        };
        let app = Router::new()
            .route("/health", get(|| async { "ok" }))
            .route("/signal/peers", get(list_peers))
            .route("/signal/ws", get(ws_handler))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind(addr).await?;
        println!("signaling node listening on {}", addr);
        axum::serve(listener, app).await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })?;
    Ok(())
}

async fn list_peers(State(state): State<AppState>) -> axum::Json<Vec<String>> {
    let peers = state.peers.read().await;
    let mut out = peers.keys().cloned().collect::<Vec<_>>();
    out.sort_unstable();
    axum::Json(out)
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(query): Query<WsQuery>,
) -> Response {
    let Some(peer_id) = query.peer.as_deref().and_then(normalize_peer_id) else {
        return axum::http::StatusCode::BAD_REQUEST.into_response();
    };
    if !auth_ok(state.password.as_ref(), query.token.as_ref()) {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(state, socket, peer_id))
}

async fn handle_socket(state: AppState, socket: WebSocket, peer_id: String) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    {
        let mut peers = state.peers.write().await;
        peers.insert(peer_id.clone(), tx);
    }

    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sink.send(msg).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(msg)) = stream.next().await {
        let Message::Text(text) = msg else {
            continue;
        };
        if let Err(reason) = relay_message(&state, &peer_id, text.to_string()).await {
            let _ = send_error(&state, &peer_id, &reason).await;
        }
    }

    send_task.abort();
    let mut peers = state.peers.write().await;
    peers.remove(&peer_id);
}

async fn relay_message(state: &AppState, from_peer: &str, raw: String) -> Result<(), String> {
    let incoming: SignalMessage =
        serde_json::from_str(&raw).map_err(|e| format!("invalid signal payload: {e}"))?;
    let target = normalize_peer_id(incoming.to.as_str())
        .ok_or_else(|| "invalid target peer id".to_string())?;
    if target == from_peer {
        return Err("cannot signal self".to_string());
    }
    let outbound = RelayMessage {
        from: from_peer,
        kind: &incoming.kind,
        payload: incoming.payload,
    };
    let text = serde_json::to_string(&outbound)
        .map(Message::Text)
        .map_err(|e| format!("encode relay message failed: {e}"))?;

    let peers = state.peers.read().await;
    let target_tx = peers
        .get(&target)
        .ok_or_else(|| format!("target peer offline: {target}"))?;
    target_tx
        .send(text)
        .map_err(|_| format!("target connection closed: {target}"))?;
    Ok(())
}

async fn send_error(state: &AppState, peer_id: &str, reason: &str) -> Result<(), String> {
    let payload = serde_json::to_string(&ErrorMessage {
        r#type: "error",
        reason,
    })
    .map(Message::Text)
    .map_err(|e| format!("encode error message failed: {e}"))?;

    let peers = state.peers.read().await;
    let Some(tx) = peers.get(peer_id) else {
        return Ok(());
    };
    tx.send(payload)
        .map_err(|_| format!("failed sending error to {peer_id}"))?;
    Ok(())
}

fn normalize_peer_id(input: &str) -> Option<String> {
    let id = input.trim().to_string();
    if id.is_empty() {
        return None;
    }
    if id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Some(id);
    }
    None
}

fn auth_ok(expected: Option<&String>, got: Option<&String>) -> bool {
    match expected {
        Some(expected) => got.is_some_and(|token| token == expected),
        None => true,
    }
}
