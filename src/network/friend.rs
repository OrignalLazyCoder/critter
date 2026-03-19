use serde::{Deserialize, Serialize};

pub(crate) const FRIEND_ALPN: &[u8] = b"critter/friend/1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FriendPacket {
    pub version: u8,
    pub from: String,
    pub from_pet: String,
    pub kind: FriendPacketKind,
    pub ts_epoch: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum FriendPacketKind {
    Request,
    Accept,
}

pub(crate) fn encode_friend(packet: &FriendPacket) -> Result<Vec<u8>, String> {
    bincode::serialize(packet).map_err(|e| format!("serialize friend packet failed: {e}"))
}

pub(crate) fn decode_friend(bytes: &[u8]) -> Result<FriendPacket, String> {
    let p: FriendPacket = bincode::deserialize(bytes)
        .map_err(|e| format!("deserialize friend packet failed: {e}"))?;
    if p.version != 1 {
        return Err(format!("unsupported friend packet version: {}", p.version));
    }
    if p.from.trim().is_empty() {
        return Err("friend packet missing sender id".to_string());
    }
    Ok(p)
}
