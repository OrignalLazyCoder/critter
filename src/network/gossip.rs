use crate::network::codec::MoodPacket;

pub(crate) const GOSSIP_ALPN: &[u8] = b"critter/mood/1";

pub(crate) fn encode_packet(packet: &MoodPacket) -> Result<Vec<u8>, String> {
    bincode::serialize(packet).map_err(|e| format!("serialize mood packet failed: {e}"))
}

pub(crate) fn decode_packet(bytes: &[u8]) -> Result<MoodPacket, String> {
    bincode::deserialize(bytes).map_err(|e| format!("deserialize mood packet failed: {e}"))
}

pub(crate) fn packet_signature(packet: &MoodPacket) -> (u8, u8, u8, u8, bool, i8) {
    (
        packet.hunger_bucket,
        packet.energy_bucket,
        packet.social_bucket,
        packet.focus_bucket,
        packet.charging,
        packet.wifi_bucket,
    )
}

pub(crate) fn should_broadcast(
    prev: Option<&MoodPacket>,
    next: &MoodPacket,
    periodic_elapsed: bool,
) -> bool {
    if periodic_elapsed {
        return true;
    }
    match prev {
        None => true,
        Some(prev) => packet_signature(prev) != packet_signature(next),
    }
}
