//! WireGuard peering the control plane grants an approved node.
//!
//! The tunnel is what makes the node channel confidential, and it also turns
//! the source check into a cryptographic one: a packet arriving from a tunnel
//! address provably came from the holder of that peer's private key, rather
//! than merely from something plugged into the same LAN.
//!
//! Peering cannot be delivered before the tunnel exists, so it is exchanged on
//! the one cleartext connection a node makes before it is approved. That hop
//! carries public keys only; credentials wait for the tunnel.

use std::net::Ipv4Addr;

use base64::Engine;
use uuid::Uuid;

use super::model::TunnelPeering;
use super::NodeProtocolError;
use crate::db::Pool;

/// Default tunnel subnet. Control takes the first host address and nodes are
/// assigned from the second upward.
pub const DEFAULT_TUNNEL_SUBNET: &str = "10.88.0.0/24";
pub const DEFAULT_TUNNEL_PORT: u16 = 51820;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelPolicy {
    network: Ipv4Addr,
    prefix: u8,
    endpoint: String,
    control_public_key: String,
    control_port: u16,
}

impl TunnelPolicy {
    /// Reads the deployment's tunnel settings and the sidecar's published
    /// public key. Returns `None` when either is absent, which is how a
    /// deployment without a tunnel keeps working.
    pub async fn load(pool: &Pool) -> Result<Option<Self>, NodeProtocolError> {
        let Ok(endpoint) = std::env::var("SULION_TUNNEL_ENDPOINT") else {
            return Ok(None);
        };
        if endpoint.is_empty() {
            return Ok(None);
        }
        let subnet = std::env::var("SULION_TUNNEL_SUBNET")
            .unwrap_or_else(|_| DEFAULT_TUNNEL_SUBNET.to_string());
        let (network, prefix) = parse_subnet(&subnet)?;

        let Some((public_key,)) =
            sqlx::query_as::<_, (Vec<u8>,)>("SELECT public_key FROM control_tunnel WHERE id = 1")
                .fetch_optional(pool)
                .await?
        else {
            // The sidecar has not started yet. Nodes keep enrolling; they are
            // granted peering once it publishes its key.
            return Ok(None);
        };

        // The port the backend already listens on: wg0 lives in the backend's
        // own network namespace, so the tunnel reaches it directly.
        let control_port = std::env::var("SULION_LISTEN")
            .ok()
            .and_then(|listen| listen.rsplit(':').next().and_then(|port| port.parse().ok()))
            .unwrap_or(8080);

        Ok(Some(Self {
            network,
            prefix,
            endpoint,
            control_public_key: base64::engine::general_purpose::STANDARD.encode(public_key),
            control_port,
        }))
    }

    pub fn control_address(&self) -> Ipv4Addr {
        host_address(self.network, 1)
    }

    /// Renders the peering for a node holding `node_address`.
    pub fn peering_for(&self, node_address: Ipv4Addr) -> TunnelPeering {
        TunnelPeering {
            control_public_key: self.control_public_key.clone(),
            node_address: format!("{node_address}/{}", self.prefix),
            control_address: self.control_address().to_string(),
            endpoint: self.endpoint.clone(),
            control_url: format!(
                "ws://{}:{}/ws/nodes",
                self.control_address(),
                self.control_port
            ),
        }
    }

    /// Picks the lowest unused host address above control's own.
    ///
    /// Address assignment is control's job because only control can see every
    /// node; a node choosing its own could collide with another's.
    pub fn allocate(&self, taken: &[Ipv4Addr]) -> Result<Ipv4Addr, NodeProtocolError> {
        let host_count = 1_u32 << (32 - self.prefix);
        for offset in 2..host_count.saturating_sub(1) {
            let candidate = host_address(self.network, offset);
            if !taken.contains(&candidate) {
                return Ok(candidate);
            }
        }
        Err(NodeProtocolError::InvalidRequest(
            "tunnel subnet has no free addresses".into(),
        ))
    }

    pub fn contains(&self, address: Ipv4Addr) -> bool {
        let mask = if self.prefix == 0 {
            0
        } else {
            u32::MAX << (32 - self.prefix)
        };
        u32::from(address) & mask == u32::from(self.network) & mask
    }
}

fn host_address(network: Ipv4Addr, offset: u32) -> Ipv4Addr {
    Ipv4Addr::from(u32::from(network).saturating_add(offset))
}

fn parse_subnet(value: &str) -> Result<(Ipv4Addr, u8), NodeProtocolError> {
    let invalid = || NodeProtocolError::InvalidRequest(format!("invalid tunnel subnet {value}"));
    let (address, prefix) = value.split_once('/').ok_or_else(invalid)?;
    let address: Ipv4Addr = address.parse().map_err(|_| invalid())?;
    let prefix: u8 = prefix.parse().map_err(|_| invalid())?;
    if !(8..=30).contains(&prefix) {
        return Err(invalid());
    }
    Ok((address, prefix))
}

/// Decodes a WireGuard public key offered by a node.
pub fn decode_tunnel_public_key(value: &str) -> Result<Vec<u8>, NodeProtocolError> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| NodeProtocolError::AuthenticationFailed)?;
    if decoded.len() != 32 {
        return Err(NodeProtocolError::AuthenticationFailed);
    }
    Ok(decoded)
}

/// Encodes a WireGuard key in the base64 form the `wg` tools expect.
pub fn encode_tunnel_key(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A persisted WireGuard keypair.
///
/// WireGuard keys are raw X25519, and both ends need theirs to survive a
/// restart: a node's so its approved peering stays valid, control's so every
/// paired node's pinned peer key keeps matching.
#[derive(Clone)]
pub struct TunnelKeypair {
    private_key: [u8; 32],
    public_key: [u8; 32],
}

impl TunnelKeypair {
    pub fn generate() -> Self {
        let secret = x25519_dalek::StaticSecret::random_from_rng(rand_core::OsRng);
        Self::from_private(secret.to_bytes())
    }

    pub fn from_private(private_key: [u8; 32]) -> Self {
        let secret = x25519_dalek::StaticSecret::from(private_key);
        let public_key = x25519_dalek::PublicKey::from(&secret).to_bytes();
        Self {
            private_key: secret.to_bytes(),
            public_key,
        }
    }

    pub fn private_bytes(&self) -> &[u8; 32] {
        &self.private_key
    }

    pub fn public_bytes(&self) -> &[u8; 32] {
        &self.public_key
    }

    pub fn private_key(&self) -> String {
        encode_tunnel_key(&self.private_key)
    }

    pub fn public_key(&self) -> String {
        encode_tunnel_key(&self.public_key)
    }

    /// Loads the node's tunnel key from root-owned host state, generating one
    /// on first boot. Mirrors how the node's identity key is handled, so a
    /// reinstall produces a new key that needs a fresh approval.
    pub fn load_or_create(path: &std::path::Path) -> anyhow::Result<Self> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        if let Ok(stored) = std::fs::read(path) {
            let bytes: [u8; 32] = stored
                .as_slice()
                .try_into()
                .map_err(|_| anyhow::anyhow!("tunnel key at {} is not 32 bytes", path.display()))?;
            return Ok(Self::from_private(bytes));
        }
        let generated = Self::generate();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("tunnel.tmp");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(generated.private_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        Ok(generated)
    }

    /// Loads control's tunnel key from the database, generating one on first
    /// start. The public half is what nodes are handed as their peer.
    pub async fn load_or_create_control(pool: &Pool) -> Result<Self, NodeProtocolError> {
        if let Some((private_key,)) =
            sqlx::query_as::<_, (Vec<u8>,)>("SELECT private_key FROM control_tunnel WHERE id = 1")
                .fetch_optional(pool)
                .await?
        {
            let bytes: [u8; 32] = private_key.as_slice().try_into().map_err(|_| {
                NodeProtocolError::Cryptography("stored control tunnel key is invalid".into())
            })?;
            return Ok(Self::from_private(bytes));
        }
        let generated = Self::generate();
        sqlx::query(
            "INSERT INTO control_tunnel (id, private_key, public_key) \
             VALUES (1, $1, $2) ON CONFLICT (id) DO NOTHING",
        )
        .bind(generated.private_bytes().as_slice())
        .bind(generated.public_bytes().as_slice())
        .execute(pool)
        .await?;
        // Re-read so a racing sidecar and this one converge on one key.
        Box::pin(Self::load_or_create_control(pool)).await
    }
}

impl std::fmt::Debug for TunnelKeypair {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TunnelKeypair")
            .field("public_key", &self.public_key())
            .finish_non_exhaustive()
    }
}

pub const NODE_TUNNEL_KEY_PATH_ENV: &str = "SULION_NODE_TUNNEL_KEY_PATH";
pub const DEFAULT_NODE_TUNNEL_KEY_PATH: &str = "/var/lib/sulion-node/tunnel-private.key";
pub const NODE_TUNNEL_CONF_PATH_ENV: &str = "SULION_NODE_TUNNEL_CONF_PATH";
pub const DEFAULT_NODE_TUNNEL_CONF_PATH: &str = "/var/lib/sulion-node/wg0.conf";

/// Renders the node's `wg0` configuration.
///
/// `AllowedIPs` is control's single address rather than the whole subnet: the
/// tunnel exists to reach the control plane, and a node has no business
/// routing to another node through it.
pub fn render_node_config(keypair: &TunnelKeypair, peering: &TunnelPeering) -> String {
    format!(
        "# Generated by sulion-node from the peering the control plane granted.\n\
         # Do not edit; regenerated whenever the peering changes.\n\
         [Interface]\n\
         PrivateKey = {private_key}\n\
         Address = {node_address}\n\
         \n\
         [Peer]\n\
         PublicKey = {control_public_key}\n\
         AllowedIPs = {control_address}/32\n\
         Endpoint = {endpoint}\n\
         PersistentKeepalive = 25\n",
        private_key = keypair.private_key(),
        node_address = peering.node_address,
        control_public_key = peering.control_public_key,
        control_address = peering.control_address,
        endpoint = peering.endpoint,
    )
}

/// Path the node writes its rendered tunnel configuration to.
pub fn node_config_path() -> std::path::PathBuf {
    std::path::PathBuf::from(
        std::env::var(NODE_TUNNEL_CONF_PATH_ENV)
            .unwrap_or_else(|_| DEFAULT_NODE_TUNNEL_CONF_PATH.to_string()),
    )
}

/// Path the node's tunnel private key is persisted at.
pub fn node_key_path() -> std::path::PathBuf {
    std::path::PathBuf::from(
        std::env::var(NODE_TUNNEL_KEY_PATH_ENV)
            .unwrap_or_else(|_| DEFAULT_NODE_TUNNEL_KEY_PATH.to_string()),
    )
}

/// Writes the rendered configuration when it differs from what is on disk.
///
/// Returns whether it changed, so a reconnect that re-delivers the same peering
/// does not churn the interface. The write is atomic because a host path unit
/// watches this file.
pub fn write_node_config(path: &std::path::Path, rendered: &str) -> anyhow::Result<bool> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    if std::fs::read_to_string(path).is_ok_and(|current| current == rendered) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("wg.tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(rendered.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path)?;
    Ok(true)
}

/// A node's approved peering, as the sidecar needs it to configure `wg`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovedPeer {
    pub node_id: Uuid,
    pub public_key: String,
    pub address: Ipv4Addr,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> TunnelPolicy {
        TunnelPolicy {
            network: "10.88.0.0".parse().unwrap(),
            prefix: 24,
            endpoint: "192.168.66.3:51820".into(),
            control_public_key: "Y29udHJvbC1rZXktMzItYnl0ZXMtcGxhY2Vob2xkZXI=".into(),
            control_port: 8080,
        }
    }

    #[test]
    fn control_takes_the_first_host_address() {
        assert_eq!(
            policy().control_address(),
            "10.88.0.1".parse::<Ipv4Addr>().unwrap()
        );
    }

    #[test]
    fn allocation_skips_addresses_already_in_use() {
        let policy = policy();
        assert_eq!(
            policy.allocate(&[]).unwrap(),
            "10.88.0.2".parse::<Ipv4Addr>().unwrap()
        );
        assert_eq!(
            policy
                .allocate(&["10.88.0.2".parse().unwrap(), "10.88.0.3".parse().unwrap()])
                .unwrap(),
            "10.88.0.4".parse::<Ipv4Addr>().unwrap()
        );
    }

    #[test]
    fn peering_points_the_node_at_control_only() {
        let peering = policy().peering_for("10.88.0.2".parse().unwrap());
        assert_eq!(peering.node_address, "10.88.0.2/24");
        assert_eq!(peering.control_address, "10.88.0.1");
        assert_eq!(peering.endpoint, "192.168.66.3:51820");
        assert_eq!(peering.control_url, "ws://10.88.0.1:8080/ws/nodes");
    }

    #[test]
    fn membership_follows_the_configured_prefix() {
        let policy = policy();
        assert!(policy.contains("10.88.0.9".parse().unwrap()));
        assert!(!policy.contains("10.88.1.9".parse().unwrap()));
        assert!(!policy.contains("192.168.66.4".parse().unwrap()));
    }

    #[test]
    fn a_keypair_persists_and_reloads_to_the_same_public_key() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tunnel-private.key");
        let first = TunnelKeypair::load_or_create(&path).unwrap();
        let second = TunnelKeypair::load_or_create(&path).unwrap();
        assert_eq!(first.public_key(), second.public_key());
        // Approval binds to the public key, so a silently rotated key would
        // lock the node out until it was approved again.
        assert_ne!(first.public_key(), TunnelKeypair::generate().public_key());
    }

    #[test]
    fn a_tunnel_key_is_not_world_readable() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tunnel-private.key");
        TunnelKeypair::load_or_create(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0);
    }

    #[test]
    fn the_rendered_node_config_routes_only_to_control() {
        let keypair = TunnelKeypair::generate();
        let peering = policy().peering_for("10.88.0.2".parse().unwrap());
        let rendered = render_node_config(&keypair, &peering);
        assert!(rendered.contains("Address = 10.88.0.2/24"));
        // Control's single address, not the subnet: nodes do not route to
        // each other through this tunnel.
        assert!(rendered.contains("AllowedIPs = 10.88.0.1/32"));
        assert!(rendered.contains("Endpoint = 192.168.66.3:51820"));
        assert!(rendered.contains(&format!("PrivateKey = {}", keypair.private_key())));
    }

    #[test]
    fn writing_the_node_config_reports_only_real_changes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("wg0.conf");
        assert!(write_node_config(&path, "first").unwrap());
        assert!(!write_node_config(&path, "first").unwrap());
        assert!(write_node_config(&path, "second").unwrap());
    }

    #[test]
    fn a_malformed_subnet_is_an_error_rather_than_a_default() {
        assert!(parse_subnet("10.88.0.0").is_err());
        assert!(parse_subnet("10.88.0.0/31").is_err());
        assert!(parse_subnet("nonsense/24").is_err());
    }
}
