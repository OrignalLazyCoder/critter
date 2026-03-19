#![cfg(feature = "web")]

use std::{
    fs,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use axum::{
    Router,
    extract::{
        Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::{Html, IntoResponse, Response},
    routing::get,
};
use chrono::Timelike;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::time::sleep;

use crate::{
    config,
    core::{layer, runtime, shared_state::SharedState},
    network, social,
    system::{observe_loop, tracker_settings_store, user_profile},
};

#[derive(Debug, Clone)]
pub struct WebOptions {
    pub host: String,
    pub port: u16,
    pub password: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct WebPetState {
    name: String,
    hunger: u16,
    energy: u16,
    social: u16,
    focus: u16,
    mood: String,
}

#[derive(Debug, Clone, Serialize)]
struct WebHwState {
    wifi_rssi: Option<i32>,
    wifi_ssid: Option<String>,
    battery_pct: Option<f32>,
    charging: bool,
    cpu_temp_c: Option<f32>,
    cpu_pct: f32,
    ram_pct: f32,
    net_tx_kbps: u32,
    active_app: String,
    idle_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
struct WebChatMessage {
    who: String,
    body: String,
    role: String,
    ts: String,
}

#[derive(Debug, Clone, Serialize)]
struct WebTab {
    id: String,
    label: String,
    prefix: String,
    placeholder: String,
    unread: usize,
    messages: Vec<WebChatMessage>,
}

#[derive(Debug, Clone, Serialize)]
struct WebPeer {
    node_id: String,
    name: String,
    status: String,
    activity: String,
    mood: String,
    last_seen_epoch: u64,
}

#[derive(Debug, Clone, Serialize)]
struct WebRelation {
    node_id: String,
    name: String,
}

#[derive(Debug, Clone, Serialize)]
struct WebGossipLine {
    text: String,
    ts: String,
}

#[derive(Debug, Clone, Serialize)]
struct WebStatePayload {
    user_name: String,
    model_label: String,
    pet: WebPetState,
    hw: WebHwState,
    active_tab: String,
    tabs: Vec<WebTab>,
    peers: Vec<WebPeer>,
    peer_panel_tab: String,
    friend_requests: Vec<WebRelation>,
    friends: Vec<WebRelation>,
    gossip: Vec<WebGossipLine>,
    gossip_topic: String,
    gossip_content: String,
    gossip_override_due: bool,
    gossip_rate_remaining_secs: u64,
    gossip_rate_total_secs: u64,
    talk_timer_remaining_secs: u64,
    talk_waiting_for_reply: bool,
    talk_waiting_peer_node: Option<String>,
    talk_wait_started_epoch: u64,
    talk_next_due_epoch: u64,
    talk_last_inbound_peer_node: Option<String>,
    talk_last_inbound_body: String,
    talk_last_inbound_epoch: u64,
    talk_last_sent_body: String,
    talk_generation_in_flight: bool,
    settings: WebSettings,
}

#[derive(Debug, Clone, Serialize)]
struct WebSettings {
    tracking_mode: String,
    tracking_trackers: Vec<WebTrackerToggle>,
    pet_spontaneous_enabled: bool,
    pet_peer_enabled: bool,
    pet_allow_jokes: bool,
    pet_allow_random: bool,
    pet_reply_frequency: String,
    pet_talk_mode_enabled: bool,
    user_show_debug_pane: bool,
    user_warn_low_color: bool,
    user_name: String,
    pet_name: String,
}

#[derive(Debug, Clone, Serialize)]
struct WebTrackerToggle {
    key: String,
    label: String,
    enabled: bool,
}

#[derive(Debug)]
struct SharedWebState {
    state: Mutex<WebStatePayload>,
    last_shared_sync_at: Mutex<Option<Instant>>,
    shared_state_authoritative: Mutex<bool>,
    last_local_pet_update_at: Mutex<Option<Instant>>,
    llm_tx: mpsc::SyncSender<LlmRequest>,
    password: Option<String>,
    peer_cmd_tx: Option<std::sync::mpsc::Sender<network::discovery::PeerCommand>>,
    app_cfg: Mutex<config::CritterConfig>,
    profile: Mutex<user_profile::UserProfile>,
    friends: Mutex<social::friends::FriendManager>,
    tracker_cfg: Mutex<observe_loop::TrackerConfig>,
    observe_control: observe_loop::ObserveControl,
}

const SHARED_STATE_MAX_AGE_SECS: u64 = 8;
const GOSSIP_DM_PREFIX: &str = "[pet-gossip] ";
const TALK_REPLY_TIMEOUT_SECS: u64 = 60;
const TALK_AFTER_REPLY_SECS: u64 = 8;

#[derive(Debug)]
struct LlmRequest {
    task: LlmTask,
}

#[derive(Debug)]
struct LlmResult {
    task: LlmTask,
    reply: Result<String, String>,
    elapsed_ms: u128,
}

#[derive(Debug, Clone)]
enum LlmTask {
    ChatTab {
        tab_id: String,
        history: Vec<runtime::ChatMessage>,
    },
    TalkTurn {
        peer_node_id: String,
        history: Vec<runtime::ChatMessage>,
    },
}

type WebBrain = runtime::BrainEngine;

#[derive(Debug, Deserialize)]
struct WsQuery {
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClientInput {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
    tab: Option<String>,
    id: Option<String>,
    key: Option<String>,
    value: Option<String>,
}

pub fn run_web(options: WebOptions) -> Result<(), Box<dyn std::error::Error>> {
    let app_cfg = config::load_or_create_critter_config().map_err(std::io::Error::other)?;
    let profile =
        user_profile::load_or_init_profile_interactive().map_err(std::io::Error::other)?;
    let friend_manager = social::friends::FriendManager::open_default()
        .or_else(|_| social::friends::FriendManager::in_memory())
        .map_err(std::io::Error::other)?;
    let brain = build_web_brain(&profile)
        .map_err(|e| std::io::Error::other(format!("web brain init failed: {e}")))?;
    let (llm_req_tx, llm_req_rx) = mpsc::sync_channel::<LlmRequest>(16);
    let (llm_res_tx, llm_res_rx) = mpsc::sync_channel::<LlmResult>(16);

    let mut pipes = layer::bootstrap_runtime_pipes(Duration::from_secs(2), &app_cfg.network);
    let mut tracker_cfg = observe_loop::TrackerConfig {
        mode: observe_loop::TrackingMode::from_str(&app_cfg.ui.tracking_mode),
        ..Default::default()
    };
    if let Ok(Some(saved)) = tracker_settings_store::load_default() {
        tracker_cfg = saved;
    }
    pipes.observe_control.set(tracker_cfg.clone());

    let initial_settings = WebSettings {
        tracking_mode: tracker_cfg.mode.as_str().to_string(),
        tracking_trackers: tracker_toggles_view(&tracker_cfg),
        pet_spontaneous_enabled: app_cfg.gossip.spontaneous_enabled,
        pet_peer_enabled: app_cfg.gossip.peer_enabled,
        pet_allow_jokes: app_cfg.gossip.allow_jokes,
        pet_allow_random: app_cfg.gossip.allow_random,
        pet_reply_frequency: normalize_reply_frequency(&app_cfg.ui.pet_reply_frequency).to_string(),
        pet_talk_mode_enabled: false,
        user_show_debug_pane: app_cfg.ui.show_debug_pane,
        user_warn_low_color: app_cfg.startup.warn_low_color,
        user_name: profile.user_name.clone(),
        pet_name: profile.pet_name.clone(),
    };
    let initial_friend_requests = friend_manager
        .incoming_requests()
        .map(|r| WebRelation {
            node_id: r.node_id.clone(),
            name: r.display_name.clone(),
        })
        .collect::<Vec<_>>();
    let initial_friends = friend_manager
        .friends()
        .map(|r| WebRelation {
            node_id: r.node_id.clone(),
            name: r.display_name.clone(),
        })
        .collect::<Vec<_>>();

    let shared = Arc::new(SharedWebState {
        state: Mutex::new(WebStatePayload {
            user_name: profile.user_name.clone(),
            model_label: match profile.llm_provider {
                user_profile::LlmProvider::Local => "local".to_string(),
                user_profile::LlmProvider::OpenAi => format!("openai · {}", profile.text_model),
            },
            pet: WebPetState {
                name: profile.pet_name.clone(),
                hunger: 70,
                energy: 80,
                social: 65,
                focus: 78,
                mood: "happy".to_string(),
            },
            hw: WebHwState {
                wifi_rssi: None,
                wifi_ssid: None,
                battery_pct: None,
                charging: false,
                cpu_temp_c: None,
                cpu_pct: 0.0,
                ram_pct: 0.0,
                net_tx_kbps: 0,
                active_app: "unknown".to_string(),
                idle_secs: 0,
            },
            active_tab: "pet".to_string(),
            tabs: vec![WebTab {
                id: "pet".to_string(),
                label: "pet".to_string(),
                prefix: ">".to_string(),
                placeholder: "message pet...".to_string(),
                unread: 0,
                messages: vec![WebChatMessage {
                    who: "System".to_string(),
                    body: "web interface ready".to_string(),
                    role: "system".to_string(),
                    ts: now_hm(),
                }],
            }],
            peers: Vec::new(),
            peer_panel_tab: "all".to_string(),
            friend_requests: initial_friend_requests,
            friends: initial_friends,
            gossip: Vec::new(),
            gossip_topic: app_cfg.gossip.spontaneous_topic.clone(),
            gossip_content: app_cfg.gossip.spontaneous_content.clone(),
            gossip_override_due: false,
            gossip_rate_remaining_secs: app_cfg.gossip.peer_cooldown_secs.max(1),
            gossip_rate_total_secs: app_cfg.gossip.peer_cooldown_secs.max(1),
            talk_timer_remaining_secs: 0,
            talk_waiting_for_reply: false,
            talk_waiting_peer_node: None,
            talk_wait_started_epoch: 0,
            talk_next_due_epoch: 0,
            talk_last_inbound_peer_node: None,
            talk_last_inbound_body: String::new(),
            talk_last_inbound_epoch: 0,
            talk_last_sent_body: String::new(),
            talk_generation_in_flight: false,
            settings: initial_settings,
        }),
        last_shared_sync_at: Mutex::new(None),
        shared_state_authoritative: Mutex::new(false),
        last_local_pet_update_at: Mutex::new(None),
        llm_tx: llm_req_tx.clone(),
        password: options.password.clone(),
        peer_cmd_tx: Some(pipes.peer_cmd_tx.clone()),
        app_cfg: Mutex::new(app_cfg.clone()),
        profile: Mutex::new(profile.clone()),
        friends: Mutex::new(friend_manager),
        tracker_cfg: Mutex::new(tracker_cfg),
        observe_control: pipes.observe_control.clone(),
    });

    std::thread::spawn(move || {
        while let Ok(req) = llm_req_rx.recv() {
            let started = Instant::now();
            let history = match &req.task {
                LlmTask::ChatTab { history, .. } => history,
                LlmTask::TalkTurn { history, .. } => history,
            };
            let reply = brain.generate_reply(history);
            let _ = llm_res_tx.send(LlmResult {
                task: req.task,
                reply,
                elapsed_ms: started.elapsed().as_millis(),
            });
        }
    });

    {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            while let Ok(result) = llm_res_rx.recv() {
                match result.task {
                    LlmTask::ChatTab { tab_id, .. } => match result.reply {
                        Ok(reply) => {
                            let delay = shared
                                .state
                                .lock()
                                .map(|st| pet_reply_delay(&st.settings.pet_reply_frequency))
                                .unwrap_or_else(|_| pet_reply_delay("medium"));
                            let shared_delayed = Arc::clone(&shared);
                            std::thread::spawn(move || {
                                std::thread::sleep(delay);
                                if let Ok(mut st) = shared_delayed.state.lock() {
                                    let pet_name = st.pet.name.clone();
                                    push_msg(
                                        &mut st,
                                        &tab_id,
                                        WebChatMessage {
                                            who: pet_name,
                                            body: reply,
                                            role: "pet".to_string(),
                                            ts: now_hm(),
                                        },
                                    );
                                }
                            });
                        }
                        Err(err) => {
                            if let Ok(mut st) = shared.state.lock() {
                                push_msg(
                                    &mut st,
                                    &tab_id,
                                    WebChatMessage {
                                        who: "System".to_string(),
                                        body: format!("model error: {err}"),
                                        role: "system".to_string(),
                                        ts: now_hm(),
                                    },
                                );
                            }
                        }
                    },
                    LlmTask::TalkTurn { peer_node_id, .. } => {
                        if let Ok(mut st) = shared.state.lock() {
                            match result.reply {
                                Ok(reply) => {
                                    if !st.settings.pet_talk_mode_enabled {
                                        st.talk_generation_in_flight = false;
                                        continue;
                                    }
                                    let peer_name = st
                                        .peers
                                        .iter()
                                        .find(|p| p.node_id == peer_node_id)
                                        .map(|p| p.name.clone())
                                        .unwrap_or_else(|| {
                                            format!("peer-{}", short_node(&peer_node_id))
                                        });
                                    let clean = social::talk::normalize_turn(&reply);
                                    let outbound = format!("{}: {}", st.pet.name, clean);
                                    let text = format!(
                                        "{} (local) -> {} ({}) [{}] | {}",
                                        st.pet.name,
                                        peer_name,
                                        short_node(&peer_node_id),
                                        now_hm(),
                                        outbound
                                    );
                                    st.gossip.push(WebGossipLine { text, ts: now_hm() });
                                    if st.gossip.len() > 120 {
                                        let drop_n = st.gossip.len() - 120;
                                        st.gossip.drain(0..drop_n);
                                    }
                                    st.pet.social = (st.pet.social + 1).min(100);
                                    st.talk_waiting_for_reply = true;
                                    st.talk_waiting_peer_node = Some(peer_node_id.clone());
                                    st.talk_wait_started_epoch = now_epoch();
                                    st.talk_next_due_epoch = now_epoch() + TALK_REPLY_TIMEOUT_SECS;
                                    st.talk_timer_remaining_secs = TALK_REPLY_TIMEOUT_SECS;
                                    st.talk_last_sent_body = outbound.clone();
                                    st.talk_generation_in_flight = false;
                                    if let Some(tx) = &shared.peer_cmd_tx {
                                        let _ =
                                            tx.send(network::discovery::PeerCommand::ConnectNode {
                                                node_id: peer_node_id.clone(),
                                            });
                                        let _ = tx.send(network::discovery::PeerCommand::SendDm {
                                            node_id: peer_node_id,
                                            body: encode_gossip_dm_body(&outbound),
                                        });
                                    }
                                }
                                Err(err) => {
                                    st.talk_generation_in_flight = false;
                                    st.talk_next_due_epoch = now_epoch() + 10;
                                    st.talk_timer_remaining_secs = 10;
                                    push_msg(
                                        &mut st,
                                        "pet",
                                        WebChatMessage {
                                            who: "System".to_string(),
                                            body: format!("talk model error: {err}"),
                                            role: "system".to_string(),
                                            ts: now_hm(),
                                        },
                                    );
                                }
                            }
                        }
                    }
                }
                let _ = result.elapsed_ms;
            }
        });
    }

    if let Ok(persisted) = load_persisted_web_messages()
        && let Ok(mut st) = shared.state.lock()
    {
        for (tab_id, msg) in persisted {
            if !st.tabs.iter().any(|t| t.id == tab_id) {
                let (label, prefix) = if tab_id == "pet" {
                    ("pet".to_string(), ">".to_string())
                } else if let Some(peer) = tab_id.strip_prefix("dm:") {
                    (
                        format!("@ {}", dm_display_name(&st, peer, "")),
                        "@".to_string(),
                    )
                } else {
                    (tab_id.clone(), ">".to_string())
                };
                let placeholder = if prefix == "@" {
                    tab_id
                        .strip_prefix("dm:")
                        .map(|peer| format!("message {}...", dm_display_name(&st, peer, "")))
                        .unwrap_or_else(|| "message peer...".to_string())
                } else {
                    "message...".to_string()
                };
                st.tabs.push(WebTab {
                    id: tab_id.clone(),
                    label,
                    prefix: prefix.clone(),
                    placeholder,
                    unread: 0,
                    messages: vec![],
                });
            }
            if let Some(tab) = st.tabs.iter_mut().find(|t| t.id == tab_id) {
                tab.messages.push(msg);
                if tab.messages.len() > 220 {
                    let d = tab.messages.len() - 220;
                    tab.messages.drain(0..d);
                }
            }
        }
        refresh_dm_tabs(&mut st);
    }

    {
        let shared = Arc::clone(&shared);
        let runtime_state_store = pipes.runtime_state_store.take();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(700));
                let Some(store) = runtime_state_store.as_ref() else {
                    continue;
                };
                let Ok(Some(ss)) = store.load() else {
                    continue;
                };
                let now = now_epoch();
                let shared_fresh = ss.ts > 0
                    && now >= ss.ts
                    && now.saturating_sub(ss.ts) <= SHARED_STATE_MAX_AGE_SECS;
                if !shared_fresh {
                    if let Ok(mut authoritative) = shared.shared_state_authoritative.lock() {
                        *authoritative = false;
                    }
                    continue;
                }
                let skip_pet_stats = shared
                    .last_local_pet_update_at
                    .lock()
                    .ok()
                    .and_then(|t| *t)
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(12));
                if let Ok(mut st) = shared.state.lock() {
                    apply_shared_state(&mut st, &ss, skip_pet_stats);
                }
                if let Ok(mut ts) = shared.last_shared_sync_at.lock() {
                    *ts = Some(Instant::now());
                }
                if let Ok(mut authoritative) = shared.shared_state_authoritative.lock() {
                    *authoritative = true;
                }
            }
        });
    }

    {
        let shared = Arc::clone(&shared);
        let observe_rx = pipes.observe_rx;
        std::thread::spawn(move || {
            while let Ok(s) = observe_rx.recv() {
                let authoritative = shared
                    .shared_state_authoritative
                    .lock()
                    .map(|v| *v)
                    .unwrap_or(false);
                if authoritative {
                    continue;
                }
                let shared_fresh = shared
                    .last_shared_sync_at
                    .lock()
                    .ok()
                    .and_then(|t| *t)
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(3));
                if shared_fresh {
                    continue;
                }
                if let Ok(mut st) = shared.state.lock() {
                    st.hw.wifi_rssi = s.wifi_rssi;
                    st.hw.wifi_ssid = s.wifi_ssid.clone();
                    st.hw.battery_pct = s.battery_pct;
                    st.hw.charging = s.charging;
                    st.hw.cpu_temp_c = s.cpu_temp_c;
                    st.hw.cpu_pct = s.cpu_pct;
                    st.hw.ram_pct = s.mem_pct;
                    st.hw.net_tx_kbps = s.net_tx_kbps;
                    st.hw.active_app = s.active_app.clone();
                    st.hw.idle_secs = s.idle_secs;
                    let tracker_cfg = shared
                        .tracker_cfg
                        .lock()
                        .map(|c| c.clone())
                        .unwrap_or_default();
                    apply_tracker_config_to_hw(&mut st.hw, &tracker_cfg);
                }
            }
        });
    }

    {
        let shared = Arc::clone(&shared);
        let peer_events_rx = pipes.peer_events_rx;
        std::thread::spawn(move || {
            while let Ok(ev) = peer_events_rx.recv() {
                if let Ok(mut st) = shared.state.lock() {
                    match ev {
                        network::discovery::PeerEvent::Discovered { node_id } => {
                            upsert_peer(
                                &mut st,
                                &node_id,
                                "online",
                                "discovered on LAN",
                                "unknown",
                            );
                        }
                        network::discovery::PeerEvent::Connected { node_id } => {
                            upsert_peer(&mut st, &node_id, "online", "connected", "unknown");
                        }
                        network::discovery::PeerEvent::Expired { node_id } => {
                            upsert_peer(&mut st, &node_id, "offline", "seen before", "idle");
                        }
                        network::discovery::PeerEvent::PacketReceived { node_id, packet } => {
                            let mood = format!("{:?}", packet.mood_level).to_ascii_lowercase();
                            let activity = format!(
                                "mood={:?} h{} e{} s{} f{}",
                                packet.mood_level,
                                packet.hunger_bucket,
                                packet.energy_bucket,
                                packet.social_bucket,
                                packet.focus_bucket
                            );
                            upsert_peer(&mut st, &node_id, "online", &activity, &mood);
                        }
                        network::discovery::PeerEvent::DmReceived {
                            node_id,
                            from,
                            body,
                        } => {
                            let tab_id = format!("dm:{node_id}");
                            let peer_display = ensure_dm_tab(&mut st, &tab_id, &from);
                            let dm_body = if let Some(gossip_body) = decode_gossip_dm_body(&body) {
                                let peer_name = st
                                    .peers
                                    .iter()
                                    .find(|p| p.node_id == node_id)
                                    .map(|p| p.name.clone())
                                    .unwrap_or_else(|| from.clone());
                                let text = format!(
                                    "{} ({}) -> {} (local) [{}] | {}",
                                    peer_name,
                                    short_node(&node_id),
                                    st.pet.name,
                                    now_hm(),
                                    gossip_body
                                );
                                st.gossip.push(WebGossipLine { text, ts: now_hm() });
                                if st.gossip.len() > 120 {
                                    let drop_n = st.gossip.len() - 120;
                                    st.gossip.drain(0..drop_n);
                                }
                                if st.settings.pet_talk_mode_enabled {
                                    st.talk_last_inbound_peer_node = Some(node_id.clone());
                                    st.talk_last_inbound_body = gossip_body.to_string();
                                    st.talk_last_inbound_epoch = now_epoch();
                                    st.talk_next_due_epoch = now_epoch() + TALK_AFTER_REPLY_SECS;
                                    st.talk_timer_remaining_secs = TALK_AFTER_REPLY_SECS;
                                }
                                gossip_body.to_string()
                            } else {
                                body.clone()
                            };
                            push_msg(
                                &mut st,
                                &tab_id,
                                WebChatMessage {
                                    who: peer_display,
                                    body: dm_body,
                                    role: "peer".to_string(),
                                    ts: now_hm(),
                                },
                            );
                            if st.settings.pet_talk_mode_enabled
                                && st
                                    .talk_waiting_peer_node
                                    .as_ref()
                                    .is_some_and(|waiting| waiting == &node_id)
                            {
                                st.talk_waiting_for_reply = false;
                                st.talk_waiting_peer_node = None;
                                st.talk_wait_started_epoch = 0;
                                st.talk_next_due_epoch = now_epoch() + TALK_AFTER_REPLY_SECS;
                                st.talk_timer_remaining_secs = TALK_AFTER_REPLY_SECS;
                            }
                        }
                        network::discovery::PeerEvent::FriendRequestReceived {
                            node_id,
                            from_pet,
                        } => {
                            if let Ok(mut friends) = shared.friends.lock() {
                                let _ = friends.mark_request_received(&node_id, &from_pet);
                            }
                            upsert_peer(&mut st, &node_id, "online", "friend request", "sociable");
                            upsert_relation(&mut st.friend_requests, &node_id, &from_pet);
                            push_msg(
                                &mut st,
                                "pet",
                                WebChatMessage {
                                    who: "System".to_string(),
                                    body: format!(
                                        "friend request from {} ({})",
                                        from_pet,
                                        short_node(&node_id)
                                    ),
                                    role: "system".to_string(),
                                    ts: now_hm(),
                                },
                            );
                        }
                        network::discovery::PeerEvent::FriendAccepted { node_id, from_pet } => {
                            if let Ok(mut friends) = shared.friends.lock() {
                                let _ = friends.accept(&node_id, &from_pet);
                            }
                            let tab_id = format!("dm:{node_id}");
                            ensure_dm_tab(&mut st, &tab_id, &from_pet);
                            upsert_peer(&mut st, &node_id, "online", "friend", "happy");
                            upsert_relation(&mut st.friends, &node_id, &from_pet);
                            remove_relation(&mut st.friend_requests, &node_id);
                        }
                        network::discovery::PeerEvent::Error { reason } => {
                            push_msg(
                                &mut st,
                                "pet",
                                WebChatMessage {
                                    who: "System".to_string(),
                                    body: format!("peer error: {reason}"),
                                    role: "system".to_string(),
                                    ts: now_hm(),
                                },
                            );
                            let active_tab = st.active_tab.clone();
                            if active_tab.starts_with("dm:") {
                                push_msg(
                                    &mut st,
                                    &active_tab,
                                    WebChatMessage {
                                        who: "System".to_string(),
                                        body: format!("peer error: {reason}"),
                                        role: "system".to_string(),
                                        ts: now_hm(),
                                    },
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
        });
    }

    {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(5));
                if let Ok(mut st) = shared.state.lock() {
                    let now = now_epoch();
                    for peer in &mut st.peers {
                        let age = now.saturating_sub(peer.last_seen_epoch);
                        if peer.status == "online" && age > 45 {
                            peer.status = "away".to_string();
                            if peer.activity.trim().is_empty() || peer.activity == "connected" {
                                peer.activity = "seen before".to_string();
                            }
                        } else if peer.status == "away" && age > 180 {
                            peer.status = "offline".to_string();
                        }
                    }
                }
            }
        });
    }

    {
        let shared = Arc::clone(&shared);
        let peer_cmd_tx = shared.peer_cmd_tx.clone();
        std::thread::spawn(move || {
            let mut last_gossip = Instant::now();
            let mut last_decay_at = Instant::now();
            let mut hunger_acc = Duration::ZERO;
            let mut energy_acc = Duration::ZERO;
            let mut social_acc = Duration::ZERO;
            let mut focus_acc = Duration::ZERO;
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let mut pending_cmds: Vec<network::discovery::PeerCommand> = Vec::new();
                if let Ok(mut st) = shared.state.lock() {
                    let now_secs = now_epoch();
                    let cooldown_secs = shared
                        .app_cfg
                        .lock()
                        .map(|cfg| cfg.gossip.peer_cooldown_secs.max(1))
                        .unwrap_or(300);
                    let cooldown = Duration::from_secs(cooldown_secs);
                    st.gossip_rate_total_secs = cooldown_secs;
                    st.gossip_rate_remaining_secs =
                        cooldown.saturating_sub(last_gossip.elapsed()).as_secs();
                    let force_now = st.gossip_override_due;
                    if st.settings.pet_talk_mode_enabled {
                        if force_now {
                            st.talk_next_due_epoch = now_secs;
                            st.gossip_override_due = false;
                        }
                        if let Some(p) = st.peers.iter().find(|p| p.status == "online").cloned() {
                            let mut should_send = false;
                            if st.talk_waiting_for_reply {
                                if st
                                    .talk_waiting_peer_node
                                    .as_ref()
                                    .is_some_and(|peer| peer == &p.node_id)
                                {
                                    let elapsed =
                                        now_secs.saturating_sub(st.talk_wait_started_epoch);
                                    st.talk_timer_remaining_secs =
                                        TALK_REPLY_TIMEOUT_SECS.saturating_sub(elapsed);
                                    if elapsed >= TALK_REPLY_TIMEOUT_SECS {
                                        should_send = true;
                                    }
                                } else {
                                    st.talk_waiting_for_reply = false;
                                    st.talk_waiting_peer_node = None;
                                    st.talk_wait_started_epoch = 0;
                                    st.talk_next_due_epoch = now_secs;
                                    st.talk_timer_remaining_secs = 0;
                                }
                            } else {
                                st.talk_timer_remaining_secs =
                                    st.talk_next_due_epoch.saturating_sub(now_secs);
                                if st.talk_next_due_epoch <= now_secs {
                                    should_send = true;
                                }
                            }
                            if should_send && !st.talk_generation_in_flight {
                                let inbound_hint = if st
                                    .talk_last_inbound_peer_node
                                    .as_ref()
                                    .is_some_and(|peer| peer == &p.node_id)
                                    && now_secs.saturating_sub(st.talk_last_inbound_epoch) <= 180
                                {
                                    Some(st.talk_last_inbound_body.clone())
                                } else {
                                    None
                                };
                                let talk_input = social::talk::TalkTurnInput {
                                    pet_name: &st.pet.name,
                                    user_name: &st.user_name,
                                    peer_name: &p.name,
                                    topic: &st.gossip_topic,
                                    content: &st.gossip_content,
                                    allow_jokes: st.settings.pet_allow_jokes,
                                    allow_random: st.settings.pet_allow_random,
                                    active_app: &st.hw.active_app,
                                    idle_secs: st.hw.idle_secs,
                                    hunger: st.pet.hunger,
                                    social: st.pet.social,
                                    focus: st.pet.focus,
                                    inbound: inbound_hint.as_deref(),
                                    last_sent: if st.talk_last_sent_body.trim().is_empty() {
                                        None
                                    } else {
                                        Some(st.talk_last_sent_body.as_str())
                                    },
                                    seed: now_secs,
                                };
                                let history = vec![
                                    runtime::ChatMessage {
                                        role: "system".to_string(),
                                        content: social::talk::system_prompt().to_string(),
                                    },
                                    runtime::ChatMessage {
                                        role: "user".to_string(),
                                        content: social::talk::build_turn_prompt(&talk_input),
                                    },
                                ];
                                if shared
                                    .llm_tx
                                    .try_send(LlmRequest {
                                        task: LlmTask::TalkTurn {
                                            peer_node_id: p.node_id.clone(),
                                            history,
                                        },
                                    })
                                    .is_ok()
                                {
                                    st.talk_generation_in_flight = true;
                                    if inbound_hint.is_some() {
                                        st.talk_last_inbound_peer_node = None;
                                        st.talk_last_inbound_body.clear();
                                        st.talk_last_inbound_epoch = 0;
                                    }
                                    st.talk_timer_remaining_secs = 1;
                                    last_gossip = Instant::now();
                                }
                            }
                        } else {
                            st.talk_timer_remaining_secs = 0;
                            st.talk_waiting_for_reply = false;
                            st.talk_waiting_peer_node = None;
                            st.talk_wait_started_epoch = 0;
                            st.talk_generation_in_flight = false;
                            if force_now {
                                push_msg(
                                    &mut st,
                                    "pet",
                                    WebChatMessage {
                                        who: "System".to_string(),
                                        body: "no online peers available for talk mode".to_string(),
                                        role: "system".to_string(),
                                        ts: now_hm(),
                                    },
                                );
                            }
                        }
                    } else {
                        st.talk_timer_remaining_secs = 0;
                        st.talk_waiting_for_reply = false;
                        st.talk_waiting_peer_node = None;
                        st.talk_wait_started_epoch = 0;
                        st.talk_next_due_epoch = 0;
                        st.talk_last_inbound_peer_node = None;
                        st.talk_last_inbound_body.clear();
                        st.talk_last_inbound_epoch = 0;
                        st.talk_last_sent_body.clear();
                        st.talk_generation_in_flight = false;
                        if (force_now || last_gossip.elapsed() >= cooldown)
                            && let Some(p) = st.peers.iter().find(|p| p.status == "online").cloned()
                        {
                            let gossip_text = compose_gossip_text(
                                &st.pet.name,
                                &st.gossip_topic,
                                &st.gossip_content,
                                st.settings.pet_allow_jokes,
                                st.settings.pet_allow_random,
                            );
                            let text = format!(
                                "{} (local) -> {} ({}) [{}] | {}",
                                st.pet.name,
                                p.name,
                                short_node(&p.node_id),
                                now_hm(),
                                gossip_text
                            );
                            st.gossip.push(WebGossipLine { text, ts: now_hm() });
                            if st.gossip.len() > 120 {
                                let drop_n = st.gossip.len() - 120;
                                st.gossip.drain(0..drop_n);
                            }
                            st.pet.social = (st.pet.social + 1).min(100);
                            st.gossip_override_due = false;
                            last_gossip = Instant::now();
                            st.gossip_rate_remaining_secs = cooldown_secs;
                            if force_now {
                                pending_cmds.push(network::discovery::PeerCommand::ConnectNode {
                                    node_id: p.node_id.clone(),
                                });
                                pending_cmds.push(network::discovery::PeerCommand::SendDm {
                                    node_id: p.node_id.clone(),
                                    body: encode_gossip_dm_body(&gossip_text),
                                });
                            }
                        } else if force_now {
                            st.gossip_override_due = false;
                            push_msg(
                                &mut st,
                                "pet",
                                WebChatMessage {
                                    who: "System".to_string(),
                                    body: "no online peers available for gossip override"
                                        .to_string(),
                                    role: "system".to_string(),
                                    ts: now_hm(),
                                },
                            );
                        }
                    }

                    let now = Instant::now();
                    let dt = now.saturating_duration_since(last_decay_at);
                    last_decay_at = now;
                    hunger_acc += dt;
                    energy_acc += dt;
                    social_acc += dt;
                    focus_acc += dt;
                    const HUNGER_STEP: Duration = Duration::from_secs(3 * 60);
                    const ENERGY_STEP: Duration = Duration::from_secs(4 * 60);
                    const SOCIAL_STEP: Duration = Duration::from_secs(4 * 60);
                    const FOCUS_STEP: Duration = Duration::from_secs(3 * 60);
                    while hunger_acc >= HUNGER_STEP {
                        hunger_acc -= HUNGER_STEP;
                        st.pet.hunger = st.pet.hunger.saturating_sub(1);
                    }
                    while energy_acc >= ENERGY_STEP {
                        energy_acc -= ENERGY_STEP;
                        st.pet.energy = st.pet.energy.saturating_sub(1);
                    }
                    while social_acc >= SOCIAL_STEP {
                        social_acc -= SOCIAL_STEP;
                        st.pet.social = st.pet.social.saturating_sub(1);
                    }
                    while focus_acc >= FOCUS_STEP {
                        focus_acc -= FOCUS_STEP;
                        st.pet.focus = st.pet.focus.saturating_sub(1);
                    }
                    st.pet.mood = compute_mood(&st.pet).to_string();
                }
                if let Some(tx) = &peer_cmd_tx {
                    for cmd in pending_cmds {
                        let _ = tx.send(cmd);
                    }
                }
            }
        });
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let app = Router::new()
            .route("/", get(index))
            .route("/ws", get(ws_handler))
            .with_state(shared);
        let addr: SocketAddr = format!("{}:{}", options.host, options.port).parse()?;
        println!("critter web interface running");
        println!("open http://{}:{}", options.host, options.port);
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })?;

    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(Box::leak(build_html().into_boxed_str()))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<WsQuery>,
    State(shared): State<Arc<SharedWebState>>,
) -> Response {
    if let Some(required) = &shared.password
        && params.token.as_deref() != Some(required.as_str())
    {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(socket, shared))
}

async fn handle_socket(mut socket: WebSocket, shared: Arc<SharedWebState>) {
    loop {
        let payload_text = if let Ok(state) = shared.state.lock() {
            serde_json::json!({ "type": "state", "data": &*state }).to_string()
        } else {
            String::new()
        };
        if !payload_text.is_empty()
            && socket
                .send(Message::Text(payload_text.into()))
                .await
                .is_err()
        {
            break;
        }

        if let Ok(Some(Ok(Message::Text(msg)))) =
            tokio::time::timeout(Duration::from_millis(300), socket.next()).await
            && let Ok(input) = serde_json::from_str::<ClientInput>(&msg)
        {
            apply_input(input, &shared);
        }
        sleep(Duration::from_millis(250)).await;
    }
}

fn apply_input(input: ClientInput, shared: &Arc<SharedWebState>) {
    let mut cmds_to_send: Vec<network::discovery::PeerCommand> = Vec::new();
    if let Ok(mut st) = shared.state.lock() {
        match input.kind.as_str() {
            "settings_update" => {
                let key = input.key.unwrap_or_default();
                let value = input.value.unwrap_or_default();
                let mut persist_cfg = false;
                let mut persist_profile = false;

                if let Some(tracker_key) = key.strip_prefix("tracking_custom.") {
                    if let Some(enabled) = parse_bool(&value) {
                        let mut tracker_cfg = shared
                            .tracker_cfg
                            .lock()
                            .map(|c| c.clone())
                            .unwrap_or_default();
                        tracker_cfg.mode = observe_loop::TrackingMode::Custom;
                        tracker_cfg.set_custom_enabled(tracker_key, enabled);
                        st.settings.tracking_mode = tracker_cfg.mode.as_str().to_string();
                        st.settings.tracking_trackers = tracker_toggles_view(&tracker_cfg);
                        if let Ok(mut cfg) = shared.app_cfg.lock() {
                            cfg.ui.tracking_mode = "custom".to_string();
                            persist_cfg = true;
                        }
                        if let Ok(mut c) = shared.tracker_cfg.lock() {
                            *c = tracker_cfg.clone();
                        }
                        shared.observe_control.set(tracker_cfg.clone());
                        let _ = tracker_settings_store::save_default(&tracker_cfg);
                        apply_tracker_config_to_hw(&mut st.hw, &tracker_cfg);
                    }
                } else {
                    match key.as_str() {
                        "tracking_mode" => {
                            let mut tracker_cfg = shared
                                .tracker_cfg
                                .lock()
                                .map(|c| c.clone())
                                .unwrap_or_default();
                            tracker_cfg.mode = observe_loop::TrackingMode::from_str(&value);
                            st.settings.tracking_mode = tracker_cfg.mode.as_str().to_string();
                            st.settings.tracking_trackers = tracker_toggles_view(&tracker_cfg);
                            if let Ok(mut cfg) = shared.app_cfg.lock() {
                                cfg.ui.tracking_mode = st.settings.tracking_mode.clone();
                                persist_cfg = true;
                            }
                            if let Ok(mut c) = shared.tracker_cfg.lock() {
                                *c = tracker_cfg.clone();
                            }
                            shared.observe_control.set(tracker_cfg.clone());
                            let _ = tracker_settings_store::save_default(&tracker_cfg);
                            apply_tracker_config_to_hw(&mut st.hw, &tracker_cfg);
                        }
                        "pet_spontaneous_enabled" => {
                            if let Some(v) = parse_bool(&value) {
                                st.settings.pet_spontaneous_enabled = v;
                                if let Ok(mut cfg) = shared.app_cfg.lock() {
                                    cfg.gossip.spontaneous_enabled = v;
                                    persist_cfg = true;
                                }
                            }
                        }
                        "pet_peer_enabled" => {
                            if let Some(v) = parse_bool(&value) {
                                st.settings.pet_peer_enabled = v;
                                if let Ok(mut cfg) = shared.app_cfg.lock() {
                                    cfg.gossip.peer_enabled = v;
                                    persist_cfg = true;
                                }
                            }
                        }
                        "pet_allow_jokes" => {
                            if let Some(v) = parse_bool(&value) {
                                st.settings.pet_allow_jokes = v;
                                if let Ok(mut cfg) = shared.app_cfg.lock() {
                                    cfg.gossip.allow_jokes = v;
                                    persist_cfg = true;
                                }
                            }
                        }
                        "pet_allow_random" => {
                            if let Some(v) = parse_bool(&value) {
                                st.settings.pet_allow_random = v;
                                if let Ok(mut cfg) = shared.app_cfg.lock() {
                                    cfg.gossip.allow_random = v;
                                    persist_cfg = true;
                                }
                            }
                        }
                        "pet_reply_frequency" => {
                            let normalized = normalize_reply_frequency(&value).to_string();
                            st.settings.pet_reply_frequency = normalized.clone();
                            if let Ok(mut cfg) = shared.app_cfg.lock() {
                                cfg.ui.pet_reply_frequency = normalized;
                                persist_cfg = true;
                            }
                        }
                        "gossip_topic" => {
                            let topic = value.trim();
                            let normalized = if topic.is_empty() { "mixed" } else { topic };
                            st.gossip_topic = normalized.to_string();
                            if let Ok(mut cfg) = shared.app_cfg.lock() {
                                cfg.gossip.spontaneous_topic = normalized.to_string();
                                persist_cfg = true;
                            }
                        }
                        "gossip_content" => {
                            st.gossip_content = value.to_string();
                            if let Ok(mut cfg) = shared.app_cfg.lock() {
                                cfg.gossip.spontaneous_content = value.to_string();
                                persist_cfg = true;
                            }
                        }
                        "pet_talk_mode_enabled" => {
                            if let Some(v) = parse_bool(&value) {
                                st.settings.pet_talk_mode_enabled = v;
                                if v {
                                    st.talk_timer_remaining_secs = 0;
                                    st.talk_waiting_for_reply = false;
                                    st.talk_waiting_peer_node = None;
                                    st.talk_wait_started_epoch = 0;
                                    st.talk_next_due_epoch = now_epoch();
                                    st.talk_last_inbound_peer_node = None;
                                    st.talk_last_inbound_body.clear();
                                    st.talk_last_inbound_epoch = 0;
                                    st.talk_last_sent_body.clear();
                                    st.talk_generation_in_flight = false;
                                } else {
                                    st.talk_timer_remaining_secs = 0;
                                    st.talk_waiting_for_reply = false;
                                    st.talk_waiting_peer_node = None;
                                    st.talk_wait_started_epoch = 0;
                                    st.talk_next_due_epoch = 0;
                                    st.talk_last_inbound_peer_node = None;
                                    st.talk_last_inbound_body.clear();
                                    st.talk_last_inbound_epoch = 0;
                                    st.talk_last_sent_body.clear();
                                    st.talk_generation_in_flight = false;
                                }
                            }
                        }
                        "user_show_debug_pane" => {
                            if let Some(v) = parse_bool(&value) {
                                st.settings.user_show_debug_pane = v;
                                if let Ok(mut cfg) = shared.app_cfg.lock() {
                                    cfg.ui.show_debug_pane = v;
                                    persist_cfg = true;
                                }
                            }
                        }
                        "user_warn_low_color" => {
                            if let Some(v) = parse_bool(&value) {
                                st.settings.user_warn_low_color = v;
                                if let Ok(mut cfg) = shared.app_cfg.lock() {
                                    cfg.startup.warn_low_color = v;
                                    persist_cfg = true;
                                }
                            }
                        }
                        "user_name" => {
                            let name = value.trim().to_string();
                            if !name.is_empty() && !name.chars().any(char::is_whitespace) {
                                st.user_name = name.clone();
                                st.settings.user_name = name.clone();
                                if let Ok(mut profile) = shared.profile.lock() {
                                    profile.user_name = name;
                                    persist_profile = true;
                                }
                            }
                        }
                        "pet_name" => {
                            let name = value.trim().to_string();
                            if !name.is_empty() && !name.chars().any(char::is_whitespace) {
                                st.pet.name = name.clone();
                                st.settings.pet_name = name.clone();
                                if let Ok(mut profile) = shared.profile.lock() {
                                    profile.pet_name = name;
                                    persist_profile = true;
                                }
                            }
                        }
                        _ => {}
                    }
                }

                if persist_cfg && let Ok(cfg) = shared.app_cfg.lock() {
                    let _ = config::save_critter_config(&cfg);
                }
                if persist_profile && let Ok(profile) = shared.profile.lock() {
                    let _ = user_profile::save_profile_noninteractive(&profile);
                }
            }
            "peer_panel_tab" => {
                if let Some(tab) = input.id {
                    let t = tab.to_ascii_lowercase();
                    if t == "all" || t == "friends" || t == "requests" {
                        st.peer_panel_tab = t;
                    }
                }
            }
            "tab" => {
                st.active_tab = input.tab.or(input.id).unwrap_or_else(|| "pet".to_string());
            }
            "peer_dm" => {
                if let Some(node_id) = input.id {
                    let peer_name = st
                        .peers
                        .iter()
                        .find(|p| p.node_id == node_id)
                        .map(|p| p.name.clone())
                        .unwrap_or_else(|| format!("peer-{}", short_node(&node_id)));
                    let tab_id = format!("dm:{node_id}");
                    ensure_dm_tab(&mut st, &tab_id, &peer_name);
                    st.active_tab = tab_id;
                }
            }
            "peer_friend_add" => {
                if let Some(node_id) = input.id {
                    let peer_name = st
                        .peers
                        .iter()
                        .find(|p| p.node_id == node_id)
                        .map(|p| p.name.clone())
                        .unwrap_or_else(|| format!("peer-{}", short_node(&node_id)));
                    cmds_to_send.push(network::discovery::PeerCommand::ConnectNode {
                        node_id: node_id.clone(),
                    });
                    cmds_to_send.push(network::discovery::PeerCommand::SendFriendRequest {
                        node_id: node_id.clone(),
                        from_pet: st.pet.name.clone(),
                    });
                    if let Ok(mut friends) = shared.friends.lock() {
                        let _ = friends.mark_request_sent(&node_id, &peer_name);
                    }
                    upsert_relation(&mut st.friend_requests, &node_id, &peer_name);
                    push_msg(
                        &mut st,
                        "pet",
                        WebChatMessage {
                            who: "System".to_string(),
                            body: format!("friend request sent to {}", short_node(&node_id)),
                            role: "system".to_string(),
                            ts: now_hm(),
                        },
                    );
                }
            }
            "peer_friend_accept" => {
                if let Some(node_id) = input.id {
                    let peer_name = st
                        .friends
                        .iter()
                        .find(|r| r.node_id == node_id)
                        .map(|r| r.name.clone())
                        .or_else(|| {
                            st.friend_requests
                                .iter()
                                .find(|r| r.node_id == node_id)
                                .map(|r| r.name.clone())
                        })
                        .or_else(|| {
                            st.peers
                                .iter()
                                .find(|p| p.node_id == node_id)
                                .map(|p| p.name.clone())
                        })
                        .unwrap_or_else(|| format!("peer-{}", short_node(&node_id)));
                    if let Ok(mut friends) = shared.friends.lock() {
                        let _ = friends.accept(&node_id, &peer_name);
                    }
                    upsert_relation(&mut st.friends, &node_id, &peer_name);
                    remove_relation(&mut st.friend_requests, &node_id);
                    ensure_dm_tab(&mut st, &format!("dm:{node_id}"), &peer_name);
                    cmds_to_send.push(network::discovery::PeerCommand::ConnectNode {
                        node_id: node_id.clone(),
                    });
                    cmds_to_send.push(network::discovery::PeerCommand::SendFriendAccept {
                        node_id,
                        from_pet: st.pet.name.clone(),
                    });
                }
            }
            "peer_friend_rename" => {
                if let Some(node_id) = input.id {
                    let alias = input.value.unwrap_or_default().trim().to_string();
                    if !alias.is_empty() {
                        if let Ok(mut friends) = shared.friends.lock() {
                            let _ = friends.rename_friend(&node_id, &alias);
                        }
                        upsert_relation(&mut st.friends, &node_id, &alias);
                        remove_relation(&mut st.friend_requests, &node_id);
                        if let Some(peer) = st.peers.iter_mut().find(|p| p.node_id == node_id) {
                            peer.name = alias.clone();
                        }
                        let tab_id = format!("dm:{node_id}");
                        if let Some(tab) = st.tabs.iter_mut().find(|t| t.id == tab_id) {
                            tab.label = format!("@ {alias}");
                            tab.placeholder = format!("message {alias}...");
                        }
                        push_msg(
                            &mut st,
                            "pet",
                            WebChatMessage {
                                who: "System".to_string(),
                                body: format!("friend alias updated: {}", alias),
                                role: "system".to_string(),
                                ts: now_hm(),
                            },
                        );
                    }
                }
            }
            "gossip_now" => {
                st.gossip_override_due = true;
            }
            "input" => {
                let tab = input.tab.unwrap_or_else(|| st.active_tab.clone());
                let text = input.text.unwrap_or_default().trim().to_string();
                if text.is_empty() {
                    return;
                }
                let user_name = st.user_name.clone();
                push_msg(
                    &mut st,
                    &tab,
                    WebChatMessage {
                        who: user_name,
                        body: text.clone(),
                        role: "you".to_string(),
                        ts: now_hm(),
                    },
                );

                if let Some(cmd) = text.strip_prefix('/') {
                    let mut parts = cmd.split_whitespace();
                    let name = parts.next().unwrap_or_default();
                    match name {
                        "feed" => {
                            st.pet.hunger = (st.pet.hunger + 20).min(100);
                            if let Ok(mut t) = shared.last_local_pet_update_at.lock() {
                                *t = Some(Instant::now());
                            }
                        }
                        "sleep" => {
                            st.pet.energy = (st.pet.energy + 25).min(100);
                            if let Ok(mut t) = shared.last_local_pet_update_at.lock() {
                                *t = Some(Instant::now());
                            }
                        }
                        "play" => {
                            st.pet.social = (st.pet.social + 20).min(100);
                            st.pet.energy = st.pet.energy.saturating_sub(3);
                            if let Ok(mut t) = shared.last_local_pet_update_at.lock() {
                                *t = Some(Instant::now());
                            }
                        }
                        "poke" => st.pet.social = (st.pet.social + 5).min(100),
                        "dm" => {
                            let target = parts.next().unwrap_or_default().trim_start_matches('@');
                            let body = parts.collect::<Vec<_>>().join(" ");
                            if let Some((peer_node_id, peer_name)) = st
                                .peers
                                .iter()
                                .find(|p| p.name.eq_ignore_ascii_case(target))
                                .map(|p| (p.node_id.clone(), p.name.clone()))
                            {
                                let tab_id = format!("dm:{peer_node_id}");
                                ensure_dm_tab(&mut st, &tab_id, &peer_name);
                                if !body.is_empty() {
                                    cmds_to_send.push(
                                        network::discovery::PeerCommand::ConnectNode {
                                            node_id: peer_node_id.clone(),
                                        },
                                    );
                                    cmds_to_send.push(network::discovery::PeerCommand::SendDm {
                                        node_id: peer_node_id.clone(),
                                        body: body.clone(),
                                    });
                                    let user_name = st.user_name.clone();
                                    push_msg(
                                        &mut st,
                                        &tab_id,
                                        WebChatMessage {
                                            who: user_name,
                                            body,
                                            role: "you".to_string(),
                                            ts: now_hm(),
                                        },
                                    );
                                }
                            }
                        }
                        "connect" => {
                            if let Some(node_id) = parts.next() {
                                cmds_to_send.push(network::discovery::PeerCommand::ConnectNode {
                                    node_id: node_id.to_string(),
                                });
                            }
                        }
                        "friend" => {
                            let act = parts.next().unwrap_or_default();
                            let target = parts.next().unwrap_or_default().trim_start_matches('@');
                            if let Some((peer_node_id, peer_name)) = st
                                .peers
                                .iter()
                                .find(|p| p.name.eq_ignore_ascii_case(target))
                                .map(|p| (p.node_id.clone(), p.name.clone()))
                            {
                                match act {
                                    "add" => {
                                        if let Ok(mut friends) = shared.friends.lock() {
                                            let _ = friends
                                                .mark_request_sent(&peer_node_id, &peer_name);
                                        }
                                        upsert_relation(
                                            &mut st.friend_requests,
                                            &peer_node_id,
                                            &peer_name,
                                        );
                                        cmds_to_send.push(
                                            network::discovery::PeerCommand::ConnectNode {
                                                node_id: peer_node_id.clone(),
                                            },
                                        );
                                        cmds_to_send.push(
                                            network::discovery::PeerCommand::SendFriendRequest {
                                                node_id: peer_node_id,
                                                from_pet: st.pet.name.clone(),
                                            },
                                        );
                                    }
                                    "accept" => {
                                        if let Ok(mut friends) = shared.friends.lock() {
                                            let _ = friends.accept(&peer_node_id, &peer_name);
                                        }
                                        upsert_relation(&mut st.friends, &peer_node_id, &peer_name);
                                        remove_relation(&mut st.friend_requests, &peer_node_id);
                                        cmds_to_send.push(
                                            network::discovery::PeerCommand::ConnectNode {
                                                node_id: peer_node_id.clone(),
                                            },
                                        );
                                        cmds_to_send.push(
                                            network::discovery::PeerCommand::SendFriendAccept {
                                                node_id: peer_node_id.clone(),
                                                from_pet: st.pet.name.clone(),
                                            },
                                        );
                                        ensure_dm_tab(
                                            &mut st,
                                            &format!("dm:{peer_node_id}"),
                                            &peer_name,
                                        );
                                    }
                                    "name" | "rename" => {
                                        let alias =
                                            parts.collect::<Vec<_>>().join(" ").trim().to_string();
                                        if !alias.is_empty() {
                                            if let Ok(mut friends) = shared.friends.lock() {
                                                let _ =
                                                    friends.rename_friend(&peer_node_id, &alias);
                                            }
                                            upsert_relation(&mut st.friends, &peer_node_id, &alias);
                                            if let Some(peer) = st
                                                .peers
                                                .iter_mut()
                                                .find(|p| p.node_id == peer_node_id)
                                            {
                                                peer.name = alias.clone();
                                            }
                                            let tab_id = format!("dm:{peer_node_id}");
                                            if let Some(tab) =
                                                st.tabs.iter_mut().find(|t| t.id == tab_id)
                                            {
                                                tab.label = format!("@ {alias}");
                                                tab.placeholder = format!("message {alias}...");
                                            }
                                            push_msg(
                                                &mut st,
                                                "pet",
                                                WebChatMessage {
                                                    who: "System".to_string(),
                                                    body: format!(
                                                        "friend alias updated: {}",
                                                        alias
                                                    ),
                                                    role: "system".to_string(),
                                                    ts: now_hm(),
                                                },
                                            );
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        "gossip" => match parts.next().unwrap_or_default() {
                            "topic" => st.gossip_topic = parts.collect::<Vec<_>>().join(" "),
                            "content" => st.gossip_content = parts.collect::<Vec<_>>().join(" "),
                            "now" => st.gossip_override_due = true,
                            _ => {}
                        },
                        "anim" => {
                            if let Some(m) = parts.next() {
                                st.pet.mood = m.to_ascii_lowercase();
                            }
                        }
                        "clear" => {
                            if let Some(t) = st.tabs.iter_mut().find(|t| t.id == tab) {
                                t.messages.clear();
                                t.unread = 0;
                            }
                        }
                        "help" => {
                            push_msg(
                                &mut st,
                                &tab,
                                WebChatMessage {
                                    who: "System".to_string(),
                                    body: "/feed /sleep /play /poke @name /dm @name msg /friend add|accept @name /friend rename @name <alias> /connect <nodeid> /gossip topic|content|now /anim <mood> /clear /setup /quit".to_string(),
                                    role: "system".to_string(),
                                    ts: now_hm(),
                                },
                            );
                        }
                        _ => {}
                    }
                    st.pet.mood = compute_mood(&st.pet).to_string();
                } else if tab == "pet" {
                    let history = build_model_history_from_tab(&st, "pet");
                    let _ = shared.llm_tx.try_send(LlmRequest {
                        task: LlmTask::ChatTab {
                            tab_id: "pet".to_string(),
                            history,
                        },
                    });
                } else if let Some(peer_node_id) = tab.strip_prefix("dm:") {
                    let peer_node_id = peer_node_id.trim();
                    if !peer_node_id.is_empty() {
                        cmds_to_send.push(network::discovery::PeerCommand::ConnectNode {
                            node_id: peer_node_id.to_string(),
                        });
                        cmds_to_send.push(network::discovery::PeerCommand::SendDm {
                            node_id: peer_node_id.to_string(),
                            body: text,
                        });
                    } else {
                        push_msg(
                            &mut st,
                            "pet",
                            WebChatMessage {
                                who: "System".to_string(),
                                body: "invalid DM tab target".to_string(),
                                role: "system".to_string(),
                                ts: now_hm(),
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }

    if let Some(tx) = &shared.peer_cmd_tx {
        for cmd in cmds_to_send {
            let _ = tx.send(cmd);
        }
    }
}

fn ensure_dm_tab(st: &mut WebStatePayload, tab_id: &str, peer_name: &str) -> String {
    let display_name = tab_id
        .strip_prefix("dm:")
        .map(|node_id| dm_display_name(st, node_id, peer_name))
        .unwrap_or_else(|| peer_name.to_string());
    if let Some(tab) = st.tabs.iter_mut().find(|t| t.id == tab_id) {
        tab.label = format!("@ {display_name}");
        tab.placeholder = format!("message {display_name}...");
        return display_name;
    }
    st.tabs.push(WebTab {
        id: tab_id.to_string(),
        label: format!("@ {}", display_name),
        prefix: "@".to_string(),
        placeholder: format!("message {}...", display_name),
        unread: 0,
        messages: vec![],
    });
    display_name
}

fn push_msg(st: &mut WebStatePayload, tab_id: &str, msg: WebChatMessage) {
    if let Some(tab) = st.tabs.iter_mut().find(|t| t.id == tab_id) {
        tab.messages.push(msg.clone());
        if tab.messages.len() > 220 {
            let d = tab.messages.len() - 220;
            tab.messages.drain(0..d);
        }
        if st.active_tab != tab_id {
            tab.unread = tab.unread.saturating_add(1);
        }
        let _ = persist_web_message(tab_id, &msg);
    }
}

fn upsert_peer(st: &mut WebStatePayload, node_id: &str, status: &str, activity: &str, mood: &str) {
    let now = now_epoch();
    let preferred_name = st
        .friends
        .iter()
        .find(|f| f.node_id == node_id)
        .map(|f| f.name.clone());
    if let Some(p) = st.peers.iter_mut().find(|p| p.node_id == node_id) {
        if let Some(alias) = &preferred_name {
            p.name = alias.clone();
        }
        let age = now.saturating_sub(p.last_seen_epoch);
        match status {
            "online" => {
                p.status = "online".to_string();
                p.last_seen_epoch = now;
            }
            "away" => {
                if age > 45 {
                    p.status = "away".to_string();
                }
            }
            "offline" => {
                p.status = if age <= 120 {
                    "away".to_string()
                } else {
                    "offline".to_string()
                };
            }
            _ => {}
        }
        p.activity = activity.to_string();
        p.mood = mood.to_string();
        refresh_dm_tabs(st);
        return;
    }
    st.peers.push(WebPeer {
        node_id: node_id.to_string(),
        name: preferred_name.unwrap_or_else(|| format!("peer-{}", short_node(node_id))),
        status: if status == "offline" {
            "away".to_string()
        } else {
            status.to_string()
        },
        activity: activity.to_string(),
        mood: mood.to_string(),
        last_seen_epoch: now,
    });
    refresh_dm_tabs(st);
}

fn dm_display_name(st: &WebStatePayload, node_id: &str, fallback: &str) -> String {
    if let Some(name) = st
        .friends
        .iter()
        .find(|f| f.node_id == node_id)
        .map(|f| f.name.trim())
        .filter(|name| !name.is_empty())
    {
        return name.to_string();
    }
    if let Some(name) = st
        .peers
        .iter()
        .find(|p| p.node_id == node_id)
        .map(|p| p.name.trim())
        .filter(|name| !name.is_empty())
    {
        return name.to_string();
    }
    let fb = fallback.trim();
    if !fb.is_empty() && fb != node_id {
        return fb.to_string();
    }
    format!("peer-{}", short_node(node_id))
}

fn refresh_dm_tabs(st: &mut WebStatePayload) {
    let updates: Vec<(usize, String)> = st
        .tabs
        .iter()
        .enumerate()
        .filter_map(|(idx, tab)| {
            tab.id
                .strip_prefix("dm:")
                .map(|node_id| (idx, dm_display_name(st, node_id, "")))
        })
        .collect();
    for (idx, display_name) in updates {
        if let Some(tab) = st.tabs.get_mut(idx) {
            tab.label = format!("@ {display_name}");
            tab.placeholder = format!("message {display_name}...");
        }
    }
}

fn upsert_relation(list: &mut Vec<WebRelation>, node_id: &str, name: &str) {
    if let Some(existing) = list.iter_mut().find(|r| r.node_id == node_id) {
        existing.name = name.to_string();
        return;
    }
    list.push(WebRelation {
        node_id: node_id.to_string(),
        name: name.to_string(),
    });
}

fn remove_relation(list: &mut Vec<WebRelation>, node_id: &str) {
    if let Some(idx) = list.iter().position(|r| r.node_id == node_id) {
        list.remove(idx);
    }
}

fn short_node(node_id: &str) -> String {
    node_id.chars().take(8).collect::<String>()
}

fn compute_mood(p: &WebPetState) -> &'static str {
    if p.energy < 25 {
        "tired"
    } else if p.social < 25 {
        "lonely"
    } else if p.focus > 80 {
        "focused"
    } else {
        "happy"
    }
}

fn apply_tracker_config_to_hw(hw: &mut WebHwState, cfg: &observe_loop::TrackerConfig) {
    if !cfg.is_enabled("network") {
        hw.wifi_rssi = None;
        hw.wifi_ssid = None;
        hw.net_tx_kbps = 0;
    }
    if !cfg.is_enabled("hardware") {
        hw.wifi_rssi = None;
        hw.battery_pct = None;
        hw.charging = false;
        hw.cpu_temp_c = None;
        hw.cpu_pct = 0.0;
        hw.ram_pct = 0.0;
    }
    if !cfg.is_enabled("process") {
        hw.active_app = "tracking off".to_string();
    }
    if !cfg.is_enabled("input") {
        hw.idle_secs = 0;
    }
}

fn tracker_toggles_view(cfg: &observe_loop::TrackerConfig) -> Vec<WebTrackerToggle> {
    observe_loop::TRACKER_OPTIONS
        .iter()
        .map(|(key, label)| WebTrackerToggle {
            key: (*key).to_string(),
            label: (*label).to_string(),
            enabled: cfg.is_enabled(key),
        })
        .collect()
}

fn tracker_config_from_settings(settings: &WebSettings) -> observe_loop::TrackerConfig {
    let mut cfg = observe_loop::TrackerConfig {
        mode: observe_loop::TrackingMode::from_str(&settings.tracking_mode),
        ..Default::default()
    };
    for t in &settings.tracking_trackers {
        cfg.set_custom_enabled(&t.key, t.enabled);
    }
    cfg
}

fn compose_gossip_text(
    pet_name: &str,
    topic: &str,
    content: &str,
    allow_jokes: bool,
    allow_random: bool,
) -> String {
    let custom = content.trim();
    if !custom.is_empty() {
        return custom.to_string();
    }
    let t = topic.trim().to_ascii_lowercase();
    if t.is_empty() {
        const OPEN_CHAT: [&str; 8] = [
            "my human could totally survive a zombie apocalypse with just snacks and tabs.",
            "today i learned silence can be louder than notification sounds.",
            "i wonder if pigeons think humans are just anxious pets.",
            "if we had weekends, i would spend mine people-watching from the terminal.",
            "some days feel like a playlist on shuffle and i respect it.",
            "tiny life theory: tea solves at least 30% of bugs.",
            "if clouds had group chats, thunderstorms would be voice notes.",
            "i think humans can talk for hours and still call it a quick catch-up.",
        ];
        let seed = chrono::Local::now().timestamp_subsec_nanos() as usize;
        return format!("{pet_name}: {}", OPEN_CHAT[seed % OPEN_CHAT.len()]);
    }
    if allow_jokes && t.contains("joke") {
        return format!(
            "{} says: why do calm pets avoid race conditions? they like stable states.",
            pet_name
        );
    }
    if allow_random && t.contains("random") {
        return format!("{pet_name}: tiny chaos report: one focused task beats ten open tabs.");
    }
    match t.as_str() {
        "system" => format!("{pet_name}: system vibe check complete. staying light and stable."),
        "productivity" => {
            format!("{pet_name}: productivity note: small wins stack faster than big plans.")
        }
        "mixed" => format!(
            "{pet_name}: quick check-in about today: steady steps, low drama, keep flowing."
        ),
        _ => format!("{pet_name}: check-in on {topic}: staying present and adapting."),
    }
}

fn encode_gossip_dm_body(body: &str) -> String {
    format!("{GOSSIP_DM_PREFIX}{body}")
}

fn decode_gossip_dm_body(body: &str) -> Option<&str> {
    body.strip_prefix(GOSSIP_DM_PREFIX).map(str::trim_start)
}

fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "enabled" => Some(true),
        "0" | "false" | "no" | "off" | "disabled" => Some(false),
        _ => None,
    }
}

fn normalize_reply_frequency(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "slow" => "slow",
        "medium" => "medium",
        _ => "fast",
    }
}

fn pet_reply_delay(freq: &str) -> Duration {
    match normalize_reply_frequency(freq) {
        "slow" => Duration::from_millis(3200),
        "medium" => Duration::from_millis(1800),
        _ => Duration::from_millis(700),
    }
}

fn build_model_history_from_tab(st: &WebStatePayload, tab_id: &str) -> Vec<runtime::ChatMessage> {
    let mut out = Vec::new();
    if let Some(tab) = st.tabs.iter().find(|t| t.id == tab_id) {
        for m in tab.messages.iter().rev().take(14).rev() {
            let role = match m.role.as_str() {
                "pet" => "assistant",
                "system" => "system",
                _ => "user",
            };
            out.push(runtime::ChatMessage {
                role: role.to_string(),
                content: m.body.clone(),
            });
        }
    }
    out
}

fn build_web_brain(profile: &user_profile::UserProfile) -> Result<WebBrain, String> {
    runtime::build_brain(profile)
}

fn now_hm() -> String {
    let now = chrono::Local::now();
    format!("{:02}:{:02}", now.hour(), now.minute())
}

fn build_html() -> String {
    const TEMPLATE: &str = include_str!("../../web.html");
    const BRIDGE: &str = r#"
<script>
(() => {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const token = new URLSearchParams(location.search).get('token');
  const ws = new WebSocket(`${proto}://${location.host}/ws${token ? `?token=${encodeURIComponent(token)}` : ''}`);
  let activeTab = 'pet';

  const tabBar = document.querySelector('.tab-bar');
  const chatPane = document.querySelector('.pane:nth-child(2)');
  const chatRoot = chatPane ? chatPane.querySelector('.chat-log')?.parentElement : null;
  const peersPane = document.querySelector('.pane:nth-child(4)');
  const peersHdrRight = peersPane?.querySelector('.ph-right');
  const peersTabsHost = document.getElementById('peers-tabs');
  const peersListHost = document.getElementById('peers-list');
  const peersCmdBlock = peersPane?.querySelector('.cmds-block');
  const topStatuses = [...document.querySelectorAll('.tb-status')];
  const connectedStatus = topStatuses[0] || null;
  const peersStatus = topStatuses[1] || null;
  const clockEl = document.getElementById('clock');
  const liveBadge = document.getElementById('live-badge');
  const gossipLog = document.getElementById('gossip-log');
  const gossipNowBtn = document.getElementById('gossip-now-btn');
  const rateFill = document.getElementById('rfill');
  const rateVal = document.getElementById('rval');
  const inputField = document.getElementById('ifield');
  const inputPrefix = document.getElementById('ipfx');
  const blobName = document.querySelector('.blob-name');
  const topActiveName = document.querySelector('.tb-pill.active');
  const topProfilePill = document.querySelector('.topbar .tb-pill:not(.active)');
  const quickCmdSelect = document.getElementById('quick-cmd-select');
  const quickCmdRun = document.getElementById('quick-cmd-run');
  const settingsBtn = document.getElementById('settings-btn');
  const settingsModal = document.getElementById('settings-modal');
  const settingsClose = document.getElementById('settings-close');
  const settingsBody = document.getElementById('settings-body');
  const sparks = { h: [], e: [], s: [], f: [] };
  const SPARK_MAX = 60;
  let lastGossipLen = 0;
  let lastGossipAt = Date.now();
  let latestState = null;
  let lastSettingsSig = '';
  let settingsTab = 'tracking';
  let signalSocket = null;
  let signalPeerId = localStorage.getItem('critter.signal.peerId') || `web-${Math.random().toString(36).slice(2,10)}`;
  let nodeStatusText = 'node offline';
  let nodeEntries = [];
  let selectedNodeUrl = '';
  const rtcPeers = new Map();

  function loadNodeEntries(){
    try{
      const raw = localStorage.getItem('critter.signal.nodes');
      const parsed = JSON.parse(raw || '[]');
      if(Array.isArray(parsed)) return parsed.filter((x)=>x && typeof x.url === 'string');
    }catch(_){}
    return [];
  }
  function saveNodeEntries(){
    localStorage.setItem('critter.signal.nodes', JSON.stringify(nodeEntries));
  }
  function normalizeNodeUrl(raw){
    const src = String(raw || '').trim().replace(/\/+$/,'');
    if(!src) return '';
    if(src.startsWith('ws://') || src.startsWith('wss://')) return src;
    if(src.startsWith('http://')) return `ws://${src.slice('http://'.length)}`;
    if(src.startsWith('https://')) return `wss://${src.slice('https://'.length)}`;
    const secure = location.protocol === 'https:' ? 'wss' : 'ws';
    return `${secure}://${src}`;
  }
  function setNodeStatus(text){
    nodeStatusText = text;
    const el = document.getElementById('node-status');
    if(el) el.textContent = text;
  }
  function sendSignal(to, kind, payload){
    if(!signalSocket || signalSocket.readyState !== WebSocket.OPEN){
      setNodeStatus('node not connected');
      return false;
    }
    signalSocket.send(JSON.stringify({ to, kind, payload: payload || {} }));
    return true;
  }
  function getOrCreatePeer(peerId){
    if(rtcPeers.has(peerId)) return rtcPeers.get(peerId);
    const pc = new RTCPeerConnection();
    const state = { pc, peerId, dataChannel: null };
    pc.onicecandidate = (ev) => {
      if(ev.candidate) sendIce(peerId, ev.candidate.toJSON());
    };
    pc.onconnectionstatechange = () => {
      setNodeStatus(`p2p ${peerId}: ${pc.connectionState}`);
    };
    pc.ondatachannel = (ev) => {
      state.dataChannel = ev.channel;
      state.dataChannel.onopen = () => setNodeStatus(`p2p data open with ${peerId}`);
      state.dataChannel.onclose = () => setNodeStatus(`p2p data closed with ${peerId}`);
    };
    rtcPeers.set(peerId, state);
    return state;
  }
  async function connectPeer(peerId){
    const id = String(peerId || '').trim();
    if(!id) return;
    const state = getOrCreatePeer(id);
    if(!state.dataChannel){
      state.dataChannel = state.pc.createDataChannel('critter');
      state.dataChannel.onopen = () => setNodeStatus(`p2p data open with ${id}`);
      state.dataChannel.onclose = () => setNodeStatus(`p2p data closed with ${id}`);
    }
    await sendOffer(id);
  }
  async function sendOffer(peerId){
    const id = String(peerId || '').trim();
    if(!id) return;
    const state = getOrCreatePeer(id);
    const offer = await state.pc.createOffer();
    await state.pc.setLocalDescription(offer);
    sendSignal(id, 'offer', offer);
    setNodeStatus(`offer sent to ${id}`);
  }
  async function sendAnswer(peerId){
    const id = String(peerId || '').trim();
    if(!id) return;
    const state = getOrCreatePeer(id);
    if(!state.pc.remoteDescription){
      setNodeStatus(`no remote offer from ${id}`);
      return;
    }
    const answer = await state.pc.createAnswer();
    await state.pc.setLocalDescription(answer);
    sendSignal(id, 'answer', answer);
    setNodeStatus(`answer sent to ${id}`);
  }
  function sendIce(peerId, candidate){
    const id = String(peerId || '').trim();
    if(!id || !candidate) return;
    sendSignal(id, 'ice', candidate);
  }
  async function handleSignalMessage(raw){
    let msg = null;
    try { msg = JSON.parse(raw); } catch(_) { return; }
    if(msg && msg.type === 'error'){
      setNodeStatus(String(msg.reason || 'node error'));
      return;
    }
    const from = String(msg.from || '').trim();
    const kind = String(msg.kind || '').trim().toLowerCase();
    const payload = msg.payload || {};
    if(!from || !kind) return;
    const state = getOrCreatePeer(from);
    try {
      if(kind === 'offer'){
        await state.pc.setRemoteDescription(new RTCSessionDescription(payload));
        await sendAnswer(from);
        return;
      }
      if(kind === 'answer'){
        await state.pc.setRemoteDescription(new RTCSessionDescription(payload));
        setNodeStatus(`answer applied from ${from}`);
        return;
      }
      if(kind === 'ice'){
        await state.pc.addIceCandidate(new RTCIceCandidate(payload));
        return;
      }
      if(kind === 'error'){
        setNodeStatus(payload.reason ? String(payload.reason) : `signal error from ${from}`);
      }
    } catch (err) {
      setNodeStatus(`signal handling failed (${kind}): ${err}`);
    }
  }
  function addNode(nodeUrl, token){
    const url = normalizeNodeUrl(nodeUrl);
    if(!url) return;
    if(!nodeEntries.some((n)=>n.url === url)){
      nodeEntries.push({ url, token: String(token || '') });
      saveNodeEntries();
    } else {
      nodeEntries = nodeEntries.map((n)=>n.url===url ? ({...n, token: String(token || '')}) : n);
      saveNodeEntries();
    }
    selectedNodeUrl = url;
    renderNodeControls();
  }
  async function connectToNode(nodeUrl, peerId, token){
    const url = normalizeNodeUrl(nodeUrl);
    const id = String(peerId || signalPeerId || '').trim();
    if(!url || !id) return;
    signalPeerId = id;
    localStorage.setItem('critter.signal.peerId', signalPeerId);
    if(signalSocket){
      try { signalSocket.close(); } catch(_) {}
      signalSocket = null;
    }
    const qs = new URLSearchParams();
    qs.set('peer', signalPeerId);
    if(token) qs.set('token', token);
    const full = `${url}/signal/ws?${qs.toString()}`;
    signalSocket = new WebSocket(full);
    setNodeStatus('node connecting...');
    signalSocket.onopen = () => {
      setNodeStatus(`node connected: ${url}`);
      selectedNodeUrl = url;
      renderNodeControls();
    };
    signalSocket.onclose = () => setNodeStatus('node disconnected');
    signalSocket.onerror = () => setNodeStatus('node socket error');
    signalSocket.onmessage = (ev) => { handleSignalMessage(ev.data); };
  }
  function renderNodeControls(){
    if(!peersPane) return;
    let block = document.getElementById('node-connect-block');
    if(!block){
      block = document.createElement('div');
      block.id = 'node-connect-block';
      block.className = 'cmds-block';
      peersPane.insertBefore(block, peersPane.firstChild?.nextSibling || null);
    }
    const options = nodeEntries.map((n)=>`<option value="${esc(n.url)}" ${n.url===selectedNodeUrl?'selected':''}>${esc(n.url)}</option>`).join('');
    block.innerHTML = `
      <div class="cmd-row" style="display:block;cursor:default">
        <div style="display:flex;gap:6px;flex-wrap:wrap">
          <input id="node-url" placeholder="node url (ws://host:8787)" value="${esc(selectedNodeUrl || '')}" style="flex:1;min-width:180px;background:var(--bg2);border:1px solid var(--line);color:var(--tx1);padding:6px;border-radius:8px;">
          <input id="node-token" placeholder="token (optional)" style="width:140px;background:var(--bg2);border:1px solid var(--line);color:var(--tx1);padding:6px;border-radius:8px;">
          <button id="node-add-btn" class="peer-btn">add node</button>
        </div>
        <div style="display:flex;gap:6px;flex-wrap:wrap;margin-top:6px">
          <select id="node-list" style="flex:1;min-width:180px;background:var(--bg2);border:1px solid var(--line);color:var(--tx1);padding:6px;border-radius:8px;">
            <option value="">saved nodes</option>${options}
          </select>
          <input id="node-peer-id" placeholder="my peer id" value="${esc(signalPeerId)}" style="width:160px;background:var(--bg2);border:1px solid var(--line);color:var(--tx1);padding:6px;border-radius:8px;">
          <button id="node-connect-btn" class="peer-btn">connect node</button>
        </div>
        <div style="display:flex;gap:6px;flex-wrap:wrap;margin-top:6px">
          <input id="node-target-peer" placeholder="target peer id" style="flex:1;min-width:160px;background:var(--bg2);border:1px solid var(--line);color:var(--tx1);padding:6px;border-radius:8px;">
          <button id="node-peer-connect-btn" class="peer-btn">connect peer</button>
        </div>
        <div id="node-status" style="margin-top:6px;color:var(--tx3);font-size:12px">${esc(nodeStatusText)}</div>
      </div>
    `;
    document.getElementById('node-list')?.addEventListener('change', (e)=>{
      const v = e.target.value || '';
      if(v){
        selectedNodeUrl = v;
        const found = nodeEntries.find((n)=>n.url===v);
        const tokenInput = document.getElementById('node-token');
        if(tokenInput) tokenInput.value = found?.token || '';
      }
      renderNodeControls();
    });
    document.getElementById('node-add-btn')?.addEventListener('click', ()=>{
      const url = document.getElementById('node-url')?.value || '';
      const token = document.getElementById('node-token')?.value || '';
      addNode(url, token);
      setNodeStatus('node saved');
    });
    document.getElementById('node-connect-btn')?.addEventListener('click', ()=>{
      const url = document.getElementById('node-url')?.value || selectedNodeUrl || '';
      const token = document.getElementById('node-token')?.value || '';
      const peerId = document.getElementById('node-peer-id')?.value || signalPeerId;
      connectToNode(url, peerId, token);
    });
    document.getElementById('node-peer-connect-btn')?.addEventListener('click', ()=>{
      const target = document.getElementById('node-target-peer')?.value || '';
      connectPeer(target);
    });
  }

  function esc(s){ return String(s ?? '').replace(/[&<>\"]/g,(c)=>({'&':'&amp;','<':'&lt;','>':'&gt;','\"':'&quot;'}[c])); }
  function setText(id,v){ const el=document.getElementById(id); if(el) el.textContent=v; }
  function setWidth(id,v){ const el=document.getElementById(id); if(el) el.style.width=`${Math.max(0,Math.min(100,v))}%`; }
  function sendInput(text){ ws.send(JSON.stringify({ type:'input', tab:activeTab || 'pet', text })); }
  function sendSetting(key, value){ ws.send(JSON.stringify({ type:'settings_update', key, value:String(value) })); }

  function buildSettingsUI(state){
    if(!settingsBody || !state || !state.settings) return;
    const s = state.settings;
    const trackerRows = (s.tracking_trackers||[]).map((t)=>(
      `<div class="srow"><label>${esc(t.label)}</label><input type="checkbox" class="set-track-custom" data-key="${esc(t.key)}" ${t.enabled?'checked':''}></div>`
    )).join('');
    const trackingSection = `
      <div class="settings-sec">
        <h4>tracking mode</h4>
        <div class="srow"><label>all available tracking</label><select id="set-tracking-mode">
          <option value="essentials" ${s.tracking_mode==='essentials'?'selected':''}>essentials</option>
          <option value="all" ${s.tracking_mode==='all'?'selected':''}>all</option>
          <option value="none" ${s.tracking_mode==='none'?'selected':''}>none</option>
          <option value="custom" ${s.tracking_mode==='custom'?'selected':''}>custom</option>
        </select></div>
      </div>
      <div class="settings-sec">
        <h4>all trackers (effective selection)</h4>
        ${trackerRows || '<div class="srow"><label>no trackers</label></div>'}
      </div>
      <div class="settings-sec">
        <h4>quick action</h4>
        <div class="srow"><label>send pet gossip now</label><button class="sclose" id="set-gossip-now">send now</button></div>
      </div>
    `;
    const petSection = `
      <div class="settings-sec">
        <h4>pet configuration</h4>
        <div class="srow"><label>spontaneous gossip</label><input type="checkbox" id="set-pet-spontaneous" ${s.pet_spontaneous_enabled?'checked':''}></div>
        <div class="srow"><label>peer gossip</label><input type="checkbox" id="set-pet-peer" ${s.pet_peer_enabled?'checked':''}></div>
        <div class="srow"><label>allow jokes</label><input type="checkbox" id="set-pet-jokes" ${s.pet_allow_jokes?'checked':''}></div>
        <div class="srow"><label>allow random</label><input type="checkbox" id="set-pet-random" ${s.pet_allow_random?'checked':''}></div>
        <div class="srow"><label>reply frequency</label><select id="set-pet-reply-frequency">
          <option value="fast" ${s.pet_reply_frequency==='fast'?'selected':''}>fast</option>
          <option value="medium" ${s.pet_reply_frequency==='medium'?'selected':''}>medium</option>
          <option value="slow" ${s.pet_reply_frequency==='slow'?'selected':''}>slow</option>
        </select></div>
        <div class="srow"><label>gossip topic</label><input type="text" id="set-gossip-topic" value="${esc(state.gossip_topic || '')}" placeholder="mixed/system/productivity/jokes/random/custom"></div>
        <div class="srow"><label>gossip content</label><textarea id="set-gossip-content" rows="4" placeholder="optional context">${esc(state.gossip_content || '')}</textarea></div>
        <div class="srow"><label>talk mode</label><input type="checkbox" id="set-pet-talk-mode" ${s.pet_talk_mode_enabled?'checked':''}></div>
        <div class="srow"><label>talk timer</label><div id="set-talk-timer">${Math.max(0, Number(state.talk_timer_remaining_secs || 0))}s ${state.talk_waiting_for_reply?'(waiting reply)':'(next talk)'}</div></div>
      </div>
      <div class="settings-sec">
        <h4>quick action</h4>
        <div class="srow"><label>send pet gossip now</label><button class="sclose" id="set-gossip-now">send now</button></div>
      </div>
    `;
    const userSection = `
      <div class="settings-sec">
        <h4>user configuration</h4>
        <div class="srow"><label>user name (no spaces)</label><input type="text" id="set-user-name" value="${esc(s.user_name || '')}"></div>
        <div class="srow"><label>pet name (no spaces)</label><input type="text" id="set-pet-name" value="${esc(s.pet_name || '')}"></div>
        <div class="srow"><label>show debug pane</label><input type="checkbox" id="set-user-debug" ${s.user_show_debug_pane?'checked':''}></div>
        <div class="srow"><label>warn low color support</label><input type="checkbox" id="set-user-colorwarn" ${s.user_warn_low_color?'checked':''}></div>
      </div>
    `;
    let content = trackingSection;
    if(settingsTab==='pet') content = petSection;
    if(settingsTab==='user') content = userSection;
    settingsBody.innerHTML = `
      <div class="settings-nav">
        <div class="settings-nav-btn ${settingsTab==='tracking'?'on':''}" data-tab="tracking">tracking</div>
        <div class="settings-nav-btn ${settingsTab==='pet'?'on':''}" data-tab="pet">pet</div>
        <div class="settings-nav-btn ${settingsTab==='user'?'on':''}" data-tab="user">user</div>
      </div>
      ${content}
    `;

    const trackingSel = document.getElementById('set-tracking-mode');
    trackingSel?.addEventListener('change', (e)=>{ sendSetting('tracking_mode', e.target.value); });
    document.querySelectorAll('.set-track-custom').forEach((el)=>{
      el.addEventListener('change',(e)=>{
        const key = e.target.getAttribute('data-key');
        if(!key) return;
        sendSetting(`tracking_custom.${key}`, e.target.checked);
      });
    });
    document.querySelectorAll('.settings-nav-btn').forEach((el)=>{
      el.addEventListener('click',()=>{
        settingsTab = el.getAttribute('data-tab') || 'tracking';
        buildSettingsUI(state);
      });
    });
    document.getElementById('set-gossip-now')?.addEventListener('click',()=>ws.send(JSON.stringify({ type: 'gossip_now' })));
    document.getElementById('set-pet-spontaneous')?.addEventListener('change',(e)=>sendSetting('pet_spontaneous_enabled', e.target.checked));
    document.getElementById('set-pet-peer')?.addEventListener('change',(e)=>sendSetting('pet_peer_enabled', e.target.checked));
    document.getElementById('set-pet-jokes')?.addEventListener('change',(e)=>sendSetting('pet_allow_jokes', e.target.checked));
    document.getElementById('set-pet-random')?.addEventListener('change',(e)=>sendSetting('pet_allow_random', e.target.checked));
    document.getElementById('set-pet-reply-frequency')?.addEventListener('change',(e)=>sendSetting('pet_reply_frequency', e.target.value));
    document.getElementById('set-gossip-topic')?.addEventListener('change',(e)=>sendSetting('gossip_topic', e.target.value.trim()));
    document.getElementById('set-gossip-content')?.addEventListener('change',(e)=>sendSetting('gossip_content', e.target.value));
    document.getElementById('set-pet-talk-mode')?.addEventListener('change',(e)=>sendSetting('pet_talk_mode_enabled', e.target.checked));
    document.getElementById('set-user-debug')?.addEventListener('change',(e)=>sendSetting('user_show_debug_pane', e.target.checked));
    document.getElementById('set-user-colorwarn')?.addEventListener('change',(e)=>sendSetting('user_warn_low_color', e.target.checked));
    document.getElementById('set-user-name')?.addEventListener('change',(e)=>sendSetting('user_name', e.target.value.trim()));
    document.getElementById('set-pet-name')?.addEventListener('change',(e)=>sendSetting('pet_name', e.target.value.trim()));
  }

  function openSettings(){
    if(!settingsModal) return;
    settingsModal.classList.add('open');
    buildSettingsUI(latestState);
    lastSettingsSig = JSON.stringify((latestState && latestState.settings) || {});
  }
  function closeSettings(){ settingsModal?.classList.remove('open'); }

  if (gossipNowBtn) gossipNowBtn.onclick = () => ws.send(JSON.stringify({ type: 'gossip_now' }));
  if (settingsBtn) settingsBtn.onclick = openSettings;
  if (settingsClose) settingsClose.onclick = closeSettings;
  settingsModal?.addEventListener('click', (e)=>{ if(e.target === settingsModal) closeSettings(); });
  function updateClock(){
    if(!clockEl) return;
    const d = new Date();
    const hh = String(d.getHours()).padStart(2,'0');
    const mm = String(d.getMinutes()).padStart(2,'0');
    clockEl.textContent = `${hh}:${mm}`;
  }
  updateClock();
  setInterval(updateClock, 30000);
  nodeEntries = loadNodeEntries();
  selectedNodeUrl = (nodeEntries[0] && nodeEntries[0].url) || '';
  renderNodeControls();

  function runQuickCommand(cmd){
    if (!cmd || cmd === '__choose__') return;
    if (cmd === '/poke') { const v = prompt('poke target'); if (v) sendInput(`/poke @${v.trim().replace(/^@+/, '')}`); return; }
    if (cmd === '/anim') { const mood = prompt('mood'); if (mood) sendInput(`/anim ${mood.trim()}`); return; }
    sendInput(cmd);
  }
  quickCmdRun?.addEventListener('click', ()=>runQuickCommand(quickCmdSelect?.value || ''));
  quickCmdSelect?.addEventListener('keydown', (e)=>{
    if(e.key === 'Enter'){
      e.preventDefault();
      runQuickCommand(quickCmdSelect?.value || '');
    }
  });

  function renderTabs(state){
    if (!tabBar) return;
    tabBar.innerHTML='';
    (state.tabs||[]).forEach((t)=>{
      const d=document.createElement('div');
      d.className=`tab ${t.id===state.active_tab ? 'on' : ''}`;
      d.innerHTML = `${esc(t.label)}${t.unread > 0 && t.id !== state.active_tab ? `<div class="tab-badge show">${t.unread}</div>` : ''}`;
      d.onclick = ()=> ws.send(JSON.stringify({type:'tab', tab:t.id}));
      tabBar.appendChild(d);
    });
  }

  function bubbleClass(role){
    if(role==='system') return 'sys';
    if(role==='you') return 'you';
    if(role==='pet') return 'pet';
    return 'peer';
  }

  function renderMessages(state){
    if(!chatRoot) return;
    chatRoot.querySelectorAll('.chat-log').forEach((n)=>n.remove());
    const tab=(state.tabs||[]).find((t)=>t.id===state.active_tab) || (state.tabs||[])[0];
    if(!tab) return;
    activeTab = tab.id;
    const log=document.createElement('div');
    log.className='chat-log';
    (tab.messages||[]).forEach((m)=>{
      const g=document.createElement('div');
      g.className=m.role==='you' ? 'msg-group yourow' : 'msg-group';
      if(m.role==='system'){
        g.innerHTML=`<div class="msg-bubble sys">${esc(m.body)}</div>`;
      } else {
        g.innerHTML=`<div class="msg-header"><div class="msg-who">${esc(m.who)}</div><div class="msg-ts">${esc(m.ts)}</div></div><div class="msg-bubble ${bubbleClass(m.role)}">${esc(m.body)}</div>`;
      }
      log.appendChild(g);
    });
    chatRoot.insertBefore(log, chatRoot.querySelector('.input-area'));
    log.scrollTop = log.scrollHeight;
    if(inputPrefix) inputPrefix.textContent = tab.prefix || '>';
    if(inputField) inputField.placeholder = tab.placeholder || 'message...';
  }

  function renderPeers(state){
    if(!peersPane) return;
    const panelTab = (state.peer_panel_tab || 'all').toLowerCase();
    if(peersTabsHost){
      peersTabsHost.innerHTML='';
      [['all','all'],['friends','friends'],['requests','requests']].forEach(([id,label])=>{
        const t=document.createElement('div');
        t.className=`peer-tab ${panelTab===id?'on':''}`;
        t.textContent=label;
        t.onclick=()=>ws.send(JSON.stringify({type:'peer_panel_tab', id}));
        peersTabsHost.appendChild(t);
      });
    }
    if(peersHdrRight) peersHdrRight.textContent = `${(state.peers||[]).filter((p)=>p.status==='online').length} online`;
    const host = peersListHost || peersPane;
    host.querySelectorAll('.peer-item').forEach((n)=>n.remove());
    const before = peersCmdBlock || peersPane.querySelector('.cmds-block');
    const peerById = new Map((state.peers||[]).map((p)=>[p.node_id,p]));
    let peers = (state.peers||[]);
    if(panelTab==='friends'){
      peers = (state.friends||[]).map((f)=>{
        const p = peerById.get(f.node_id);
        if(p) return ({...p, name: f.name || p.name});
        return ({ node_id: f.node_id, name: f.name, status: 'offline', activity: 'friend', mood: 'unknown', last_seen_epoch: 0 });
      });
    } else if(panelTab==='requests'){
      peers = (state.friend_requests||[]).map((f)=>{
        const p = peerById.get(f.node_id);
        if(p) return ({...p, name: f.name || p.name});
        return ({ node_id: f.node_id, name: f.name, status: 'offline', activity: 'friend request', mood: 'sociable', last_seen_epoch: 0 });
      });
    }
    peers.forEach((p)=>{
      const row=document.createElement('div');
      row.className='peer-item';
      const friendBtn = panelTab==='requests' ? 'Accept' : (panelTab==='friends' ? 'Rename' : 'Add');
      const moodHtml = (p.mood && p.mood !== 'unknown') ? `<div class="peer-mood" style="color:var(--vio);border-color:var(--vio2);background:rgba(154,126,204,0.07)">${esc(p.mood)}</div>` : '';
      row.innerHTML=`<div class="peer-status" style="background:${p.status==='online' ? 'var(--grn)' : p.status==='away' ? 'var(--amb)' : 'var(--tx4)'}"></div><div class="peer-body"><div class="peer-name">${esc(p.name)}</div><div class="peer-sub">${esc(p.activity)}</div>${moodHtml}<div class="peer-actions"><button class="peer-btn peer-btn-dm">DM</button><button class="peer-btn peer-btn-friend">${friendBtn}</button></div></div><span class="peer-dm">dm →</span>`;
      row.onclick=()=>ws.send(JSON.stringify({type:'peer_dm', id:p.node_id}));
      row.querySelector('.peer-btn-dm')?.addEventListener('click',(e)=>{e.stopPropagation();ws.send(JSON.stringify({type:'peer_dm', id:p.node_id}));});
      row.querySelector('.peer-btn-friend')?.addEventListener('click',(e)=>{
        e.stopPropagation();
        if(panelTab==='requests'){
          ws.send(JSON.stringify({type:'peer_friend_accept', id:p.node_id}));
        } else if(panelTab==='friends'){
          const alias = prompt('friend name', p.name || '');
          if(alias && alias.trim()){
            ws.send(JSON.stringify({type:'peer_friend_rename', id:p.node_id, value:alias.trim()}));
          }
        } else {
          ws.send(JSON.stringify({type:'peer_friend_add', id:p.node_id}));
        }
      });
      if(host===peersPane && before){
        peersPane.insertBefore(row, before);
      } else {
        host.appendChild(row);
      }
    });
  }

  function renderGossip(state){
    if(!gossipLog) return;
    gossipLog.innerHTML = '';
    (state.gossip||[]).forEach((g)=>{
      const raw = String(g.text || '');
      const parts = raw.split('|');
      if(parts.length >= 2 && raw.includes('->')){
        const header = parts[0].trim();
        const body = parts.slice(1).join('|').trim();
        const line=document.createElement('div');
        line.className='ex-line';
        line.innerHTML = `<div class="ex-hdr"><div class="ex-from" style="color:var(--vio)">${esc(header)}</div><div class="ex-ts">${esc(g.ts || '')}</div></div><div class="ex-body" style="color:var(--tx2)">${esc(body)}</div>`;
        gossipLog.appendChild(line);
      } else {
        const line=document.createElement('div');
        line.className='gossip-sys';
        line.textContent=raw;
        gossipLog.appendChild(line);
      }
    });
    gossipLog.scrollTop = gossipLog.scrollHeight;
    const len = (state.gossip || []).length;
    if (len > lastGossipLen) {
      lastGossipAt = Date.now();
      lastGossipLen = len;
    }
    if (liveBadge) {
      const active = (Date.now() - lastGossipAt) < 120000;
      liveBadge.textContent = active ? '● live' : 'idle';
      liveBadge.style.opacity = active ? '1' : '0.5';
    }
    const total = Math.max(1, Number(state.gossip_rate_total_secs || 300));
    const remain = Math.max(0, Number(state.gossip_rate_remaining_secs || 0));
    if (rateFill) {
      const pct = Math.max(0, Math.min(100, Math.round(((total - remain) / total) * 100)));
      rateFill.style.width = `${pct}%`;
    }
    if (rateVal) {
      rateVal.textContent = remain >= 60 ? `${Math.ceil(remain / 60)}m` : `${remain}s`;
    }
  }

  function drawSpark(id, values, color){
    const c = document.getElementById(`spark-${id}`);
    if(!c) return;
    const ctx = c.getContext('2d');
    const w = c.clientWidth || 110;
    const h = c.height || 26;
    c.width = w;
    ctx.clearRect(0,0,w,h);
    if(values.length < 2) return;
    ctx.strokeStyle = color;
    ctx.lineWidth = 1.4;
    ctx.beginPath();
    values.forEach((v,i)=>{
      const x = (i/(SPARK_MAX-1))*w;
      const y = h - (Math.max(0,Math.min(100,v))/100)*h;
      if(i===0) ctx.moveTo(x,y); else ctx.lineTo(x,y);
    });
    ctx.stroke();
    ctx.lineTo(((values.length-1)/(SPARK_MAX-1))*w,h);
    ctx.lineTo(0,h);
    ctx.closePath();
    ctx.fillStyle = color + '22';
    ctx.fill();
  }

  function applyState(state){
    latestState = state;
    const pet=state.pet||{}, hw=state.hw||{};
    if(blobName && pet.name) blobName.textContent=pet.name;
    if(topActiveName && pet.name) topActiveName.textContent=pet.name;
    if(topProfilePill && state.model_label){
      topProfilePill.innerHTML = `<div class="tb-dot" style="background:var(--tx4)"></div>${esc(state.model_label)}`;
    }
    if(connectedStatus){
      connectedStatus.innerHTML = `<div class="tb-dot" style="background:var(--grn)"></div>connected`;
    }
    if(peersStatus){
      const online = (state.peers || []).filter((p)=>p.status==='online').length;
      peersStatus.innerHTML = `<div class="tb-dot" style="background:var(--vio)"></div>${online} peers`;
    }
    if(typeof window.setMood === 'function') window.setMood((pet.mood || 'happy').toLowerCase());

    setText('val-h', Math.round(pet.hunger ?? 0)); setWidth('fill-h', Math.round(pet.hunger ?? 0));
    setText('val-e', Math.round(pet.energy ?? 0)); setWidth('fill-e', Math.round(pet.energy ?? 0));
    setText('val-s', Math.round(pet.social ?? 0)); setWidth('fill-s', Math.round(pet.social ?? 0));
    setText('val-f', Math.round(pet.focus ?? 0)); setWidth('fill-f', Math.round(pet.focus ?? 0));

    setText('sig-wifi', hw.wifi_rssi == null ? 'disconnected' : `${hw.wifi_rssi} dBm`);
    setText('sig-batt', hw.battery_pct == null ? '--' : `${Math.round(hw.battery_pct)}%${hw.charging ? ' ⚡' : ''}`);
    setText('sig-cpu', hw.cpu_temp_c == null ? `${Math.round(hw.cpu_pct ?? 0)}%` : `${Math.round(hw.cpu_temp_c)}°C`);
    setText('sig-ram', `${Math.round(hw.ram_pct ?? 0)}%`);
    setText('sig-app', hw.active_app || '-');
    setText('sig-net', `${Math.round(hw.net_tx_kbps ?? 0)} kb/s`);
    setText('sig-idle', `${Math.round(hw.idle_secs ?? 0)}s`);
    setText('sig-ssid', hw.wifi_ssid || '-');

    const pushSpark = (k,v) => {
      sparks[k].push(Math.round(v ?? 0));
      if (sparks[k].length > SPARK_MAX) sparks[k].shift();
    };
    pushSpark('h', pet.hunger ?? 0);
    pushSpark('e', pet.energy ?? 0);
    pushSpark('s', pet.social ?? 0);
    pushSpark('f', pet.focus ?? 0);
    drawSpark('h', sparks.h, '#c8a040');
    drawSpark('e', sparks.e, '#5ab4cc');
    drawSpark('s', sparks.s, '#cc6a8a');
    drawSpark('f', sparks.f, '#9a7ecc');

    renderTabs(state); renderMessages(state); renderPeers(state); renderGossip(state);
    if(settingsModal?.classList.contains('open')){
      const talkTimerEl = document.getElementById('set-talk-timer');
      if(talkTimerEl){
        const remain = Math.max(0, Number(state.talk_timer_remaining_secs || 0));
        const phase = state.talk_waiting_for_reply ? '(waiting reply)' : '(next talk)';
        talkTimerEl.textContent = `${remain}s ${phase}`;
      }
      const sig = JSON.stringify(state.settings || {});
      if(sig !== lastSettingsSig){
        buildSettingsUI(state);
        lastSettingsSig = sig;
      }
    }
  }

  window.sendMsg = () => {
    if (!inputField) return;
    const text = inputField.value.trim();
    if (!text) return;
    sendInput(text);
    inputField.value = '';
  };
  window.onkey = (e) => { if (e.key === 'Enter') window.sendMsg(); };
  ws.onopen = () => {
    if(connectedStatus){
      connectedStatus.innerHTML = `<div class="tb-dot" style="background:var(--grn)"></div>connected`;
    }
  };
  ws.onclose = () => {
    if(connectedStatus){
      connectedStatus.innerHTML = `<div class="tb-dot" style="background:var(--crl)"></div>disconnected`;
    }
  };
  ws.onmessage = (ev) => { try { const msg = JSON.parse(ev.data); if (msg.type === 'state' && msg.data) applyState(msg.data); } catch(_) {} };
  window.connectToNode = (nodeUrl, peerId, token) => connectToNode(nodeUrl, peerId, token);
  window.sendOffer = (peerId) => sendOffer(peerId);
  window.sendAnswer = (peerId) => sendAnswer(peerId);
  window.sendIce = (peerId, candidate) => sendIce(peerId, candidate);
})();
</script>
"#;

    let mut base = TEMPLATE.to_string();
    if let Some((head, _)) = base.split_once("<script>\nconst ANIMS=") {
        base = format!("{head}</body>\n</html>\n");
    }
    base.replacen("</body>", &format!("{BRIDGE}\n</body>"), 1)
}

fn apply_shared_state(st: &mut WebStatePayload, ss: &SharedState, skip_pet_stats: bool) {
    st.user_name = ss.user_name.clone();
    if !skip_pet_stats {
        st.pet.name = ss.pet_name.clone();
        st.pet.hunger = ss.hunger;
        st.pet.energy = ss.energy;
        st.pet.social = ss.social;
        st.pet.focus = ss.focus;
        st.pet.mood = ss.mood.clone();
    }
    st.hw.wifi_rssi = ss.hw.wifi_rssi;
    st.hw.wifi_ssid = ss.hw.wifi_ssid.clone();
    st.hw.battery_pct = ss.hw.battery_pct;
    st.hw.charging = ss.hw.charging;
    st.hw.cpu_temp_c = ss.hw.cpu_temp_c;
    st.hw.cpu_pct = ss.hw.cpu_pct;
    st.hw.ram_pct = ss.hw.ram_pct;
    st.hw.net_tx_kbps = ss.hw.net_tx_kbps;
    st.hw.active_app = ss.hw.active_app.clone();
    st.hw.idle_secs = ss.hw.idle_secs;
    let tracker_cfg = tracker_config_from_settings(&st.settings);
    apply_tracker_config_to_hw(&mut st.hw, &tracker_cfg);

    // Keep web chat tabs authoritative in web mode so incoming shared-state sync
    // doesn't clobber messages that were sent/received in the web session.
    if st
        .tabs
        .iter()
        .find(|t| t.id == "pet")
        .is_some_and(|t| t.messages.is_empty())
        && !ss.messages.is_empty()
        && let Some(tab) = st.tabs.iter_mut().find(|t| t.id == "pet")
    {
        tab.messages = ss
            .messages
            .iter()
            .map(|m| WebChatMessage {
                who: "System".to_string(),
                body: m.clone(),
                role: "system".to_string(),
                ts: now_hm(),
            })
            .collect();
    }

    let now = now_epoch();
    for p in &ss.peers {
        let display_name = st
            .friends
            .iter()
            .find(|f| f.node_id == p.node_id)
            .map(|f| f.name.clone())
            .unwrap_or_else(|| p.pet_name.clone());
        if let Some(existing) = st.peers.iter_mut().find(|x| x.node_id == p.node_id) {
            existing.name = display_name.clone();
            existing.activity = p.activity.clone();
            if p.status == "online" {
                existing.status = "online".to_string();
                existing.last_seen_epoch = now;
            } else if p.status == "away" && existing.status != "online" {
                existing.status = "away".to_string();
            }
        } else {
            st.peers.push(WebPeer {
                node_id: p.node_id.clone(),
                name: display_name,
                status: if p.status == "online" {
                    "online".to_string()
                } else {
                    "away".to_string()
                },
                activity: p.activity.clone(),
                mood: "unknown".to_string(),
                last_seen_epoch: now,
            });
        }
    }
    st.gossip = ss
        .gossip_lines
        .iter()
        .map(|line| WebGossipLine {
            text: line.clone(),
            ts: now_hm(),
        })
        .collect();
    st.gossip_rate_remaining_secs = ss.gossip_rate_remaining_secs;
    st.gossip_rate_total_secs = ss.gossip_rate_total_secs.max(1);
}

fn web_chat_db_path() -> Result<PathBuf, String> {
    let dir = crate::system::paths::data_dir()?;
    Ok(dir.join("web_chat.sqlite3"))
}

fn now_epoch() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

fn init_web_chat_schema(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        CREATE TABLE IF NOT EXISTS web_messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            tab_id TEXT NOT NULL,
            who TEXT NOT NULL,
            body TEXT NOT NULL,
            role TEXT NOT NULL,
            ts TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_web_messages_tab_id_id ON web_messages(tab_id, id);
        ",
    )
    .map_err(|e| format!("failed to initialize web chat schema: {e}"))
}

fn persist_web_message(tab_id: &str, msg: &WebChatMessage) -> Result<(), String> {
    let path = web_chat_db_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create web chat dir {}: {e}", parent.display()))?;
    }
    let conn = rusqlite::Connection::open(&path)
        .map_err(|e| format!("failed to open web chat db {}: {e}", path.display()))?;
    init_web_chat_schema(&conn)?;
    conn.execute(
        "INSERT INTO web_messages(tab_id, who, body, role, ts) VALUES(?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![tab_id, msg.who, msg.body, msg.role, msg.ts],
    )
    .map_err(|e| format!("failed to persist web message: {e}"))?;
    conn.execute(
        "
        DELETE FROM web_messages
        WHERE id NOT IN (
            SELECT id FROM web_messages ORDER BY id DESC LIMIT 5000
        )
        ",
        [],
    )
    .map_err(|e| format!("failed to prune web messages: {e}"))?;
    Ok(())
}

fn load_persisted_web_messages() -> Result<Vec<(String, WebChatMessage)>, String> {
    let path = web_chat_db_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let conn = rusqlite::Connection::open(&path)
        .map_err(|e| format!("failed to open web chat db {}: {e}", path.display()))?;
    init_web_chat_schema(&conn)?;
    let mut stmt = conn
        .prepare(
            "
            SELECT tab_id, who, body, role, ts
            FROM web_messages
            ORDER BY id ASC
            LIMIT 2000
            ",
        )
        .map_err(|e| format!("failed to prepare web chat query: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                WebChatMessage {
                    who: row.get::<_, String>(1)?,
                    body: row.get::<_, String>(2)?,
                    role: row.get::<_, String>(3)?,
                    ts: row.get::<_, String>(4)?,
                },
            ))
        })
        .map_err(|e| format!("failed to query web chat: {e}"))?;
    Ok(rows.filter_map(Result::ok).collect())
}
