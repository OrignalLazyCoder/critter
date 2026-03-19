use std::fs;

use iroh::{Endpoint, SecretKey, address_lookup::MdnsAddressLookup, endpoint::presets};

pub(crate) struct PeerNode {
    pub endpoint: Endpoint,
    pub mdns: MdnsAddressLookup,
}

pub(crate) async fn start_node(alpns: Vec<Vec<u8>>) -> Result<PeerNode, String> {
    let secret_key = load_or_create_secret_key()?;
    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret_key)
        .alpns(alpns)
        .bind()
        .await
        .map_err(|e| format!("failed to bind iroh endpoint: {e}"))?;

    let mdns = MdnsAddressLookup::builder()
        .build(endpoint.id())
        .map_err(|e| format!("failed to init mdns lookup: {e}"))?;

    let address_lookup = endpoint
        .address_lookup()
        .map_err(|e| format!("iroh address lookup unavailable: {e}"))?;
    address_lookup.add(mdns.clone());

    Ok(PeerNode { endpoint, mdns })
}

fn load_or_create_secret_key() -> Result<SecretKey, String> {
    let data_dir = crate::system::paths::data_dir()?;
    let path = data_dir.join("iroh_secret.key");
    if path.exists() {
        let raw = fs::read(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        if raw.len() == 32 {
            let mut bytes = [0_u8; 32];
            bytes.copy_from_slice(&raw);
            return Ok(SecretKey::from_bytes(&bytes));
        }
    }

    let mut rng = rand::rng();
    let secret = SecretKey::generate(&mut rng);
    fs::write(&path, secret.to_bytes())
        .map_err(|e| format!("failed to write {}: {e}", path.display()))?;
    Ok(secret)
}
