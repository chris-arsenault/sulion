//! WireGuard endpoint for the development-node channel.
//!
//! Runs beside the control process in its network namespace, so `wg0` is an
//! interface the control process binds directly. That is the whole reason this
//! is a separate process: it needs `NET_ADMIN` to create the interface, and the
//! control plane should not.
//!
//! It owns control's tunnel key and reconciles the approved-peer set. Peers are
//! whatever an operator has approved, so approving a node in the UI is also
//! what admits it to the tunnel.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::process::Stdio;
use std::time::Duration;

use sulion::node_protocol::tunnel::{TunnelKeypair, DEFAULT_TUNNEL_PORT, DEFAULT_TUNNEL_SUBNET};
use sulion::node_protocol::{ApprovedPeer, NodeControl};

/// How often the approved-peer set is reconciled. Approval is a human action,
/// so this only has to feel immediate rather than be instant.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(5);
const INTERFACE: &str = "wg0";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sulion=debug".into()),
        )
        .init();

    let config = sulion::config::Config::from_env()?;
    let pool = sulion::db::connect_and_wait_for_migrations(&config.db_url, "node-tunnel").await?;
    let keypair = TunnelKeypair::load_or_create_control(&pool).await?;
    tracing::info!(public_key = %keypair.public_key(), "control tunnel identity ready");

    let subnet =
        std::env::var("SULION_TUNNEL_SUBNET").unwrap_or_else(|_| DEFAULT_TUNNEL_SUBNET.to_string());
    let port: u16 = std::env::var("SULION_TUNNEL_PORT")
        .ok()
        .and_then(|port| port.parse().ok())
        .unwrap_or(DEFAULT_TUNNEL_PORT);
    let address = control_address(&subnet)?;

    bring_up(&keypair, address, prefix_of(&subnet)?, port).await?;

    // NodeControl is used only for its approved-peer view here; this process
    // never serves node connections itself.
    let control = NodeControl::new(pool);
    let mut applied: BTreeMap<String, Ipv4Addr> = BTreeMap::new();
    let mut interval = tokio::time::interval(RECONCILE_INTERVAL);
    loop {
        interval.tick().await;
        let peers = match control.approved_peers().await {
            Ok(peers) => peers,
            Err(error) => {
                tracing::warn!(%error, "failed to read approved tunnel peers");
                continue;
            }
        };
        if let Err(error) = reconcile(&mut applied, &peers).await {
            tracing::warn!(%error, "failed to reconcile tunnel peers");
        }
    }
}

/// Creates and configures `wg0` in this network namespace.
///
/// Idempotent: a restarted sidecar re-adopts an interface the previous one
/// created rather than tearing down a working tunnel.
async fn bring_up(
    keypair: &TunnelKeypair,
    address: Ipv4Addr,
    prefix: u8,
    port: u16,
) -> anyhow::Result<()> {
    if run("ip", &["link", "show", INTERFACE]).await.is_err() {
        run("ip", &["link", "add", INTERFACE, "type", "wireguard"]).await?;
        tracing::info!(interface = INTERFACE, "created tunnel interface");
    }

    // The private key goes in on stdin so it never appears in a process list.
    let mut child = tokio::process::Command::new("wg")
        .args(["set", INTERFACE, "listen-port", &port.to_string()])
        .args(["private-key", "/dev/stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()?;
    {
        use tokio::io::AsyncWriteExt;
        let mut stdin = child.stdin.take().expect("piped stdin");
        stdin
            .write_all(format!("{}\n", keypair.private_key()).as_bytes())
            .await?;
        stdin.shutdown().await?;
    }
    let status = child.wait().await?;
    if !status.success() {
        anyhow::bail!("wg set failed with status {status}");
    }

    let cidr = format!("{address}/{prefix}");
    // Re-adding an existing address is an error, not a no-op, so tolerate it.
    let _ = run("ip", &["addr", "add", &cidr, "dev", INTERFACE]).await;
    run("ip", &["link", "set", INTERFACE, "up"]).await?;
    tracing::info!(interface = INTERFACE, address = %cidr, port, "tunnel interface up");
    Ok(())
}

/// Brings the live peer set in line with what has been approved.
///
/// Removing a peer matters as much as adding one: a node whose approval is
/// revoked, or whose key is replaced, must lose the tunnel rather than keep a
/// working interface until the next restart.
async fn reconcile(
    applied: &mut BTreeMap<String, Ipv4Addr>,
    approved: &[ApprovedPeer],
) -> anyhow::Result<()> {
    let desired: BTreeMap<String, Ipv4Addr> = approved
        .iter()
        .map(|peer| (peer.public_key.clone(), peer.address))
        .collect();

    for (public_key, _) in applied
        .iter()
        .filter(|(key, _)| !desired.contains_key(*key))
    {
        run("wg", &["set", INTERFACE, "peer", public_key, "remove"]).await?;
        tracing::info!(peer = %public_key, "removed a tunnel peer that is no longer approved");
    }

    for (public_key, address) in &desired {
        if applied.get(public_key) == Some(address) {
            continue;
        }
        let allowed = format!("{address}/32");
        run(
            "wg",
            &[
                "set",
                INTERFACE,
                "peer",
                public_key,
                "allowed-ips",
                &allowed,
            ],
        )
        .await?;
        tracing::info!(peer = %public_key, %address, "configured an approved tunnel peer");
    }

    *applied = desired;
    Ok(())
}

async fn run(program: &str, args: &[&str]) -> anyhow::Result<()> {
    let output = tokio::process::Command::new(program)
        .args(args)
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn control_address(subnet: &str) -> anyhow::Result<Ipv4Addr> {
    let network: Ipv4Addr = subnet
        .split('/')
        .next()
        .unwrap_or_default()
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid tunnel subnet {subnet}"))?;
    Ok(Ipv4Addr::from(u32::from(network) + 1))
}

fn prefix_of(subnet: &str) -> anyhow::Result<u8> {
    subnet
        .split_once('/')
        .and_then(|(_, prefix)| prefix.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("invalid tunnel subnet {subnet}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_takes_the_first_host_address_of_the_subnet() {
        assert_eq!(
            control_address("10.88.0.0/24").unwrap(),
            "10.88.0.1".parse::<Ipv4Addr>().unwrap()
        );
        assert_eq!(prefix_of("10.88.0.0/24").unwrap(), 24);
    }

    #[tokio::test]
    async fn reconciling_an_unchanged_set_issues_no_commands() {
        // The reconcile loop runs every few seconds forever, so an unchanged
        // set must not shell out; `run` would fail here if it did.
        let mut applied = BTreeMap::new();
        applied.insert("peer-key".to_string(), "10.88.0.2".parse().unwrap());
        let approved = vec![ApprovedPeer {
            node_id: uuid::Uuid::nil(),
            public_key: "peer-key".into(),
            address: "10.88.0.2".parse().unwrap(),
        }];
        reconcile(&mut applied, &approved)
            .await
            .expect("unchanged reconcile must not invoke wg");
    }
}
