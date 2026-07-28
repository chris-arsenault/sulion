//! Source-address admission for the development-node control socket.
//!
//! A node connection carries terminal authority and receives forwarded database
//! credentials, so it is accepted only from the dedicated LAN. This is enforced
//! here in addition to the reverse proxy exposing `/ws/nodes` on a LAN-only
//! listener, so a proxy misconfiguration alone cannot admit a remote node.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use super::model::DEFAULT_NODE_LAN_CIDR;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Network {
    address: IpAddr,
    prefix: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeLanGuard {
    networks: Vec<Network>,
}

/// Admits every source. The boundary a deployment without a node LAN uses, and
/// the state the control plane holds until its policy is applied.
pub(crate) static UNRESTRICTED_POLICY: NodeSourcePolicy = NodeSourcePolicy {
    lan: NodeLanGuard::unrestricted(),
    trusted_proxies: NodeLanGuard::unrestricted(),
};

/// Why a node socket was refused. Carried so the log says which boundary
/// rejected the connection rather than a single opaque message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRefusal {
    /// No peer address was available, so no boundary could be applied.
    PeerUnknown,
    /// The request came through a trusted proxy that did not forward a usable
    /// client address. Refused rather than falling back to the proxy's own
    /// address, which would admit anything the proxy accepted.
    ForwardedAddressMissing,
    /// The resolved client address is outside the node LAN.
    OutsideLan,
}

impl std::fmt::Display for SourceRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self {
            Self::PeerUnknown => "peer address unavailable",
            Self::ForwardedAddressMissing => "trusted proxy forwarded no client address",
            Self::OutsideLan => "source outside the node LAN",
        };
        formatter.write_str(reason)
    }
}

/// Node socket admission across a possibly-proxied hop.
///
/// The control plane sits behind its own reverse proxy, so the TCP peer of a
/// node socket is usually nginx rather than the node. The forwarded client
/// address is honoured only when the peer is itself a configured proxy, which
/// keeps a forged header from an arbitrary client worthless.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeSourcePolicy {
    lan: NodeLanGuard,
    trusted_proxies: NodeLanGuard,
}

impl NodeSourcePolicy {
    pub fn new(lan: NodeLanGuard, trusted_proxies: NodeLanGuard) -> Self {
        Self {
            lan,
            trusted_proxies,
        }
    }

    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self::new(
            NodeLanGuard::from_env()?,
            NodeLanGuard::parse(
                &std::env::var("SULION_NODE_TRUSTED_PROXY_CIDR").unwrap_or_default(),
            )?,
        ))
    }

    pub fn is_unrestricted(&self) -> bool {
        self.lan.is_unrestricted()
    }

    /// Whether this peer may speak for a different client address.
    ///
    /// An empty proxy list trusts nobody. This deliberately does not reuse
    /// [`NodeLanGuard::admits`], whose empty case means "admit everything" —
    /// that reading would let any caller forge its own source address.
    fn trusts_proxy(&self, peer: IpAddr) -> bool {
        !self.trusted_proxies.is_unrestricted() && self.trusted_proxies.admits(peer)
    }

    /// Resolves the effective client address and applies the LAN boundary.
    pub fn admit(
        &self,
        peer: Option<IpAddr>,
        forwarded: Option<&str>,
    ) -> Result<IpAddr, SourceRefusal> {
        if self.lan.is_unrestricted() {
            return Ok(peer.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)));
        }
        let peer = peer.ok_or(SourceRefusal::PeerUnknown)?;
        let client = if self.trusts_proxy(peer) {
            forwarded
                .and_then(|value| value.split(',').next())
                .map(str::trim)
                .and_then(|value| value.parse::<IpAddr>().ok())
                .ok_or(SourceRefusal::ForwardedAddressMissing)?
        } else {
            peer
        };
        if self.lan.admits(client) {
            Ok(client)
        } else {
            Err(SourceRefusal::OutsideLan)
        }
    }
}

impl NodeLanGuard {
    pub const fn unrestricted() -> Self {
        Self {
            networks: Vec::new(),
        }
    }

    /// Reads `SULION_NODE_LAN_CIDR`, a comma-separated CIDR list.
    ///
    /// An explicitly empty value disables the check, which the portable
    /// standalone and integration deployments use because they have no LAN
    /// boundary to enforce.
    pub fn from_env() -> anyhow::Result<Self> {
        let configured = std::env::var("SULION_NODE_LAN_CIDR")
            .unwrap_or_else(|_| DEFAULT_NODE_LAN_CIDR.to_string());
        Self::parse(&configured)
    }

    pub fn parse(value: &str) -> anyhow::Result<Self> {
        let networks = value
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(parse_network)
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Self { networks })
    }

    /// True when the guard admits every source, i.e. no CIDR was configured.
    pub fn is_unrestricted(&self) -> bool {
        self.networks.is_empty()
    }

    pub fn admits(&self, address: IpAddr) -> bool {
        if self.is_unrestricted() {
            return true;
        }
        let address = unmap(address);
        self.networks
            .iter()
            .any(|network| contains(network, address))
    }
}

fn parse_network(entry: &str) -> anyhow::Result<Network> {
    let (address, prefix) = entry
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("node LAN entry {entry} must be CIDR notation"))?;
    let address = unmap(
        address
            .parse::<IpAddr>()
            .map_err(|_| anyhow::anyhow!("node LAN entry {entry} has an invalid address"))?,
    );
    let prefix: u8 = prefix
        .parse()
        .map_err(|_| anyhow::anyhow!("node LAN entry {entry} has an invalid prefix"))?;
    let width = match address {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    };
    if prefix > width {
        anyhow::bail!("node LAN entry {entry} prefix exceeds the address width");
    }
    Ok(Network { address, prefix })
}

fn contains(network: &Network, address: IpAddr) -> bool {
    match (network.address, address) {
        (IpAddr::V4(network_address), IpAddr::V4(address)) => {
            masked_v4(network_address, network.prefix) == masked_v4(address, network.prefix)
        }
        (IpAddr::V6(network_address), IpAddr::V6(address)) => {
            masked_v6(network_address, network.prefix) == masked_v6(address, network.prefix)
        }
        _ => false,
    }
}

fn masked_v4(address: Ipv4Addr, prefix: u8) -> u32 {
    let bits = u32::from(address);
    if prefix == 0 {
        0
    } else {
        bits & (u32::MAX << (32 - prefix))
    }
}

fn masked_v6(address: Ipv6Addr, prefix: u8) -> u128 {
    let bits = u128::from(address);
    if prefix == 0 {
        0
    } else {
        bits & (u128::MAX << (128 - prefix))
    }
}

/// Collapses IPv4-mapped IPv6 sources so a dual-stack listener compares against
/// the same IPv4 CIDR an operator wrote.
fn unmap(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(value) => match value.to_ipv4_mapped() {
            Some(value) => IpAddr::V4(value),
            None => IpAddr::V6(value),
        },
        value => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard(value: &str) -> NodeLanGuard {
        NodeLanGuard::parse(value).expect("parse node LAN")
    }

    fn ip(value: &str) -> IpAddr {
        value.parse().expect("parse address")
    }

    #[test]
    fn dedicated_lan_admits_only_its_own_range() {
        let guard = guard(DEFAULT_NODE_LAN_CIDR);
        assert!(guard.admits(ip("192.168.66.4")));
        assert!(guard.admits(ip("192.168.66.255")));
        assert!(!guard.admits(ip("192.168.67.4")));
        assert!(!guard.admits(ip("10.0.0.4")));
        assert!(!guard.admits(ip("127.0.0.1")));
    }

    #[test]
    fn a_proxied_public_source_is_refused() {
        // The shape that matters: traffic arriving from the shared ALB rather
        // than the dedicated LAN must never reach the challenge.
        let guard = guard(DEFAULT_NODE_LAN_CIDR);
        assert!(!guard.admits(ip("203.0.113.7")));
    }

    #[test]
    fn ipv4_mapped_sources_match_the_ipv4_range() {
        let guard = guard(DEFAULT_NODE_LAN_CIDR);
        assert!(guard.admits(ip("::ffff:192.168.66.9")));
        assert!(!guard.admits(ip("::ffff:203.0.113.9")));
    }

    #[test]
    fn multiple_ranges_are_accepted() {
        let guard = guard("192.168.66.0/24, 127.0.0.0/8");
        assert!(guard.admits(ip("192.168.66.9")));
        assert!(guard.admits(ip("127.0.0.1")));
        assert!(!guard.admits(ip("10.1.2.3")));
    }

    #[test]
    fn an_empty_configuration_disables_the_boundary() {
        let guard = guard("");
        assert!(guard.is_unrestricted());
        assert!(guard.admits(ip("203.0.113.7")));
    }

    #[test]
    fn malformed_entries_are_rejected_rather_than_ignored() {
        assert!(NodeLanGuard::parse("192.168.66.0").is_err());
        assert!(NodeLanGuard::parse("192.168.66.0/33").is_err());
        assert!(NodeLanGuard::parse("not-an-address/24").is_err());
    }

    fn policy() -> NodeSourcePolicy {
        NodeSourcePolicy::new(guard(DEFAULT_NODE_LAN_CIDR), guard("172.28.0.0/16"))
    }

    #[test]
    fn a_direct_lan_node_is_admitted_on_its_own_address() {
        assert_eq!(
            policy().admit(Some(ip("192.168.66.4")), None),
            Ok(ip("192.168.66.4"))
        );
    }

    #[test]
    fn a_forwarded_address_is_honoured_only_behind_a_trusted_proxy() {
        // Through the control plane's own nginx, the node's real address is the
        // forwarded one.
        assert_eq!(
            policy().admit(Some(ip("172.28.0.2")), Some("192.168.66.4")),
            Ok(ip("192.168.66.4"))
        );
        // The same header from an untrusted peer is ignored, so forging it
        // gains nothing.
        assert_eq!(
            policy().admit(Some(ip("203.0.113.7")), Some("192.168.66.4")),
            Err(SourceRefusal::OutsideLan)
        );
    }

    #[test]
    fn configuring_no_proxies_trusts_no_forwarded_address() {
        // An empty CIDR list means "admit everything" for the LAN boundary but
        // must mean "trust nobody" here; conflating the two let any caller
        // forge its own source address.
        let policy = NodeSourcePolicy::new(guard(DEFAULT_NODE_LAN_CIDR), guard(""));
        assert_eq!(
            policy.admit(Some(ip("203.0.113.7")), Some("192.168.66.4")),
            Err(SourceRefusal::OutsideLan)
        );
        assert_eq!(
            policy.admit(Some(ip("192.168.66.4")), None),
            Ok(ip("192.168.66.4"))
        );
    }

    #[test]
    fn a_proxy_that_forwards_nothing_is_refused_rather_than_trusted() {
        assert_eq!(
            policy().admit(Some(ip("172.28.0.2")), None),
            Err(SourceRefusal::ForwardedAddressMissing)
        );
    }

    #[test]
    fn a_proxied_remote_node_is_refused() {
        assert_eq!(
            policy().admit(Some(ip("172.28.0.2")), Some("203.0.113.7")),
            Err(SourceRefusal::OutsideLan)
        );
    }

    #[test]
    fn only_the_first_forwarded_hop_is_read() {
        assert_eq!(
            policy().admit(Some(ip("172.28.0.2")), Some("192.168.66.4, 10.9.9.9")),
            Ok(ip("192.168.66.4"))
        );
    }

    #[test]
    fn a_missing_peer_is_refused_when_a_boundary_is_configured() {
        assert_eq!(policy().admit(None, None), Err(SourceRefusal::PeerUnknown));
    }

    #[test]
    fn an_unrestricted_policy_admits_a_missing_peer() {
        let policy = NodeSourcePolicy::new(guard(""), guard(""));
        assert!(policy.is_unrestricted());
        assert!(policy.admit(None, None).is_ok());
    }
}
