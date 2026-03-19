use serde::{Deserialize, Serialize};

pub(crate) const CHAT_ALPN: &[u8] = b"critter/chat/1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DmPacket {
    pub version: u8,
    pub from: String,
    pub body: String,
    pub ts_epoch: i64,
}

pub(crate) fn encode_dm(packet: &DmPacket) -> Result<Vec<u8>, String> {
    bincode::serialize(packet).map_err(|e| format!("serialize dm packet failed: {e}"))
}

pub(crate) fn decode_dm(bytes: &[u8]) -> Result<DmPacket, String> {
    let p: DmPacket =
        bincode::deserialize(bytes).map_err(|e| format!("deserialize dm packet failed: {e}"))?;
    if p.version != 1 {
        return Err(format!("unsupported dm packet version: {}", p.version));
    }
    if p.body.trim().is_empty() {
        return Err("empty dm body".to_string());
    }
    Ok(p)
}
