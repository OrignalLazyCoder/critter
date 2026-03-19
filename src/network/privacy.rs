use crate::network::codec::{MoodLevel, MoodPacket};

pub(crate) fn validate_mood_packet(packet: &MoodPacket) -> Result<(), String> {
    if packet.version != 1 {
        return Err(format!(
            "unsupported mood packet version: {}",
            packet.version
        ));
    }
    for (name, value) in [
        ("hunger_bucket", packet.hunger_bucket),
        ("energy_bucket", packet.energy_bucket),
        ("social_bucket", packet.social_bucket),
        ("focus_bucket", packet.focus_bucket),
    ] {
        if !(1..=5).contains(&value) {
            return Err(format!("{name} out of range: {value}"));
        }
    }
    if !(0..=5).contains(&packet.wifi_bucket) {
        return Err(format!("wifi_bucket out of range: {}", packet.wifi_bucket));
    }
    match packet.mood_level {
        MoodLevel::Low | MoodLevel::Medium | MoodLevel::High => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use toml::Value;

    use crate::network::codec::{MoodLevel, MoodPacket};

    use super::validate_mood_packet;

    #[test]
    fn packet_schema_is_bucketed_only() {
        let packet = MoodPacket {
            version: 1,
            ts_epoch: 0,
            mood_level: MoodLevel::Medium,
            hunger_bucket: 3,
            energy_bucket: 2,
            social_bucket: 4,
            focus_bucket: 5,
            charging: false,
            wifi_bucket: 4,
        };
        let value = Value::try_from(&packet).expect("serialize packet to toml value");
        let table = value.as_table().expect("packet as toml table");
        let keys: BTreeSet<&str> = table.keys().map(|k| k.as_str()).collect();
        let expected: BTreeSet<&str> = [
            "version",
            "ts_epoch",
            "mood_level",
            "hunger_bucket",
            "energy_bucket",
            "social_bucket",
            "focus_bucket",
            "charging",
            "wifi_bucket",
        ]
        .into_iter()
        .collect();

        assert_eq!(keys, expected);
        for forbidden in [
            "cpu", "temp", "app", "title", "ssid", "process", "idle", "location", "calendar",
            "memory", "storage", "window",
        ] {
            assert!(!keys.contains(forbidden));
        }
    }

    #[test]
    fn packet_validation_rejects_out_of_range_values() {
        let invalid = MoodPacket {
            version: 1,
            ts_epoch: 0,
            mood_level: MoodLevel::Low,
            hunger_bucket: 0,
            energy_bucket: 2,
            social_bucket: 3,
            focus_bucket: 4,
            charging: true,
            wifi_bucket: 6,
        };
        assert!(validate_mood_packet(&invalid).is_err());
    }
}
