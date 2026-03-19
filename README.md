# Critter

Critter is a local-first Rust app that combines a virtual pet, live system observation, LLM chat, and peer social features.

It now runs in three modes from a single binary:
- `web` mode (default): browser UI + local pet runtime
- `tui` mode: terminal UI (ratatui)
- `node` mode: public signaling relay for internet peer coordination

## What Critter Does

- Real-time pet simulation with mood + four stats:
  - Hunger (`H`)
  - Energy (`E`)
  - Social (`S`)
  - Focus (`F`)
- Live OS signal ingestion (battery, CPU, RAM, app focus, Wi-Fi, network tx, idle, etc.)
- Local or OpenAI-backed pet chat
- Peer-to-peer social interactions:
  - discovery + connect
  - friend requests/accept
  - DM between friends
  - peer gossip lines
- Persistent profile state per peer instance

## Runtime Modes

### 1) Web Mode (default)

Command:

```bash
cargo run
# or
cargo run -- web
```

Defaults:
- host: `127.0.0.1`
- port: `7777 + profile_index` (example: `peer-0 => 7777`, `peer-1 => 7778`)

Flags:

```bash
cargo run -- web --host 127.0.0.1 --port 7777
cargo run -- web --host 0.0.0.0 --port 7777 --password <token>
```

Security rule:
- Non-localhost host binding requires `--password`.

### 2) TUI Mode

Command:

```bash
cargo run -- tui
```

This starts the ratatui terminal interface.

### 3) Signaling Node Mode

Command:

```bash
cargo run -- node
```

Defaults:
- host: `127.0.0.1`
- port: `8787`

Public bind example:

```bash
cargo run -- node --host 0.0.0.0 --port 8787 --password <token>
```

Security rule:
- Non-localhost host binding requires `--password`.

## Profile Isolation

Every profile has isolated config/data/identity.

Use:

```bash
cargo run -- peer 0
cargo run -- peer 1
cargo run -- node peer 0
```

Per-profile artifacts include:
- config TOML
- SQLite stores
- iroh secret key (`iroh_secret.key`)

## Web UI Features

- Full dashboard with pet state, system signals, peers, gossip, and tabs
- Chat tabs:
  - `pet`
  - DM tabs (`dm:<node_id>`) for peer chats
- Settings modal (tracking, pet behavior, user settings)
- Command shortcuts via UI buttons
- Gossip feed with cooldown indicator
- Peer panel with tabs:
  - all
  - friends
  - requests

### Web Node Controls (built-in)

The web client includes signaling/WebRTC helpers and UI controls:
- add node
- connect node
- connect peer

Built-in browser helpers (exposed on `window`):
- `connectToNode(nodeUrl, peerId, token)`
- `sendOffer(peerId)`
- `sendAnswer(peerId)`
- `sendIce(peerId, candidate)`

## Signaling Node API

Base server endpoints:
- `GET /health` -> `ok`
- `GET /signal/peers` -> list of online peer IDs
- `GET /signal/ws?peer=<id>[&token=<password>]` -> WebSocket endpoint

### Relay Message Format

Client -> node:

```json
{
  "to": "peer_b",
  "kind": "offer",
  "payload": {"sdp": "..."}
}
```

Node -> target peer:

```json
{
  "from": "peer_a",
  "kind": "offer",
  "payload": {"sdp": "..."}
}
```

Validation rules:
- `peer` IDs must be `[A-Za-z0-9_-]+`
- cannot signal self
- offline/invalid targets return an error message

## P2P and Networking Model

Current app networking combines:
- Local peer transport/events via `iroh`
- mDNS discovery on LAN (when enabled)
- direct NodeId connect (`/connect <nodeid>`)
- optional internet signaling via `node` mode server + browser helpers

Notes:
- signaling node currently relays signaling payloads only
- direct media/data relay (TURN-like forwarding) is not implemented in this server yet

## Chat and Social Behavior

- Pet tab routes messages to pet brain
- DM tabs are friend-scoped
- Friend operations:
  - `/friend add @name`
  - `/friend accept @name`
  - `/friend list`
- Direct connect:
  - `/connect <nodeid>`

## Gossip System

- Separate from main chat
- Supports spontaneous and peer-driven lines
- Configurable topic/content and pacing
- Cooldowns + per-peer turn spacing + max turns

Main commands:
- `/gossip show`
- `/gossip topic <none|system|productivity|jokes|random|mixed|custom>`
- `/gossip content <text>`
- `/gossip interval <min_secs> <max_secs>`
- `/gossip spontaneous <on|off>`
- `/gossip peer <on|off>`

## Command Reference

Core:
- `/feed`
- `/sleep`
- `/play`
- `/anim <emotion|auto>`
- `/clear`
- `/setup`
- `/help`
- `/q` or `/quit`

Networking and social:
- `/connect <nodeid>`
- `/dm @name [message]`
- `/friend add @name`
- `/friend accept @name`
- `/friend list`

Groups:
- `/group create #name`
- `/invite @name`
- `/join <code>`
- `/leave`

Generic runtime config:
- `/config show`
- `/config get <key>`
- `/config set <key> <value>`

## Configuration

`critter.toml` sections:
- `[startup]`
  - `warn_low_color`
- `[chat_persistence]`
  - `enabled`
  - `path`
  - `max_messages`
  - `load_recent_count`
- `[network]`
  - `enable_mdns`
  - `enable_direct_nodeid_connect`
- `[gossip]`
  - `spontaneous_enabled`
  - `spontaneous_min_interval_secs`
  - `spontaneous_max_interval_secs`
  - `spontaneous_topic`
  - `spontaneous_content`
  - `allow_jokes`
  - `allow_random`
  - `peer_enabled`
  - `peer_cooldown_secs`
  - `peer_turn_spacing_secs`
  - `peer_max_turns`
- `[ui]`
  - `show_debug_pane`
  - `tracking_mode`

## Persistence Layout

Per profile, Critter uses:

Config dir:
- `~/.config/critter/profiles/<profile>/critter.toml`
- `~/.config/critter/profiles/<profile>/activity.toml`

Data dir:
- `~/.local/share/critter/profiles/<profile>/config.sqlite3`
- `~/.local/share/critter/profiles/<profile>/events.sqlite3`
- `~/.local/share/critter/profiles/<profile>/state.sqlite3`
- `~/.local/share/critter/profiles/<profile>/social.sqlite3`
- `~/.local/share/critter/profiles/<profile>/chat.sqlite3` (optional)
- `~/.local/share/critter/profiles/<profile>/web_chat.sqlite3` (web mode)
- `~/.local/share/critter/profiles/<profile>/iroh_secret.key`

## Project Structure

- `src/main.rs` - binary entry
- `src/system/entrypoint.rs` - mode/profile/port/host/password routing
- `src/core/` - runtime state machine + command execution
- `src/ui/` - terminal rendering
- `src/web/` - web server + websocket UI bridge
- `src/network/` - iroh peer networking + signaling node
- `src/social/` - friends, groups, dialogue/talk policies
- `src/observe/` - OS telemetry collectors
- `src/system/` - boot, setup, stores, profile paths
- `src/pet/` - emotions + animation data

## Requirements

- Rust stable toolchain
- `cmake` (for `llama-cpp-sys-2`)
- C/C++ toolchain (Xcode CLT on macOS, or GCC/Clang on Linux)
- `curl` (for model bootstrap path)

## Build and Verify

```bash
cargo fmt
cargo check
```

## Public Node Deployment Notes

For first deployment, a small VM is enough for signaling-only:
- e2-small or e2-medium
- static public IP
- firewall open for `tcp:8787`
- run with password when bound to `0.0.0.0`

If your app is served over HTTPS, use `wss://` for browser signaling connections.

## Current Limits

- Signaling node is not a TURN/media relay.
- Group workflows exist but are still evolving.
- Some modules are under active iteration.
