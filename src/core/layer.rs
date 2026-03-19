use std::{
    sync::mpsc::{Receiver, Sender},
    time::Duration,
};

use crate::{
    config, network, observe,
    system::{observe_loop, runtime_state_store::RuntimeStateStore},
};

pub(crate) struct RuntimePipes {
    pub(crate) observe_rx: Receiver<observe::snapshot::OsSnapshot>,
    pub(crate) observe_control: crate::system::observe_loop::ObserveControl,
    pub(crate) peer_events_rx: Receiver<network::discovery::PeerEvent>,
    pub(crate) peer_cmd_tx: Sender<network::discovery::PeerCommand>,
    pub(crate) runtime_state_store: Option<RuntimeStateStore>,
}

pub(crate) fn bootstrap_runtime_pipes(
    observe_tick: Duration,
    network_cfg: &config::NetworkConfig,
) -> RuntimePipes {
    let observe_handle =
        observe_loop::start_observe_thread_controlled(observe_tick, Default::default());
    let peer_network =
        network::discovery::start_peer_network_thread(network::discovery::PeerNetworkOptions {
            enable_mdns: network_cfg.enable_mdns,
            enable_direct_nodeid_connect: network_cfg.enable_direct_nodeid_connect,
        });
    RuntimePipes {
        observe_rx: observe_handle.rx,
        observe_control: observe_handle.control,
        peer_events_rx: peer_network.events_rx,
        peer_cmd_tx: peer_network.cmd_tx,
        runtime_state_store: RuntimeStateStore::open_default().ok(),
    }
}
