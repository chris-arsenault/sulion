#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sulion=debug".into()),
        )
        .init();

    let client_config = sulion::node_protocol::client::NodeClientConfig::from_env()?;
    if !client_config.private_key_path.try_exists()? {
        sulion::node_protocol::client::generate_private_key(&client_config.private_key_path)?;
        tracing::info!(
            path = %client_config.private_key_path.display(),
            "generated development node identity key"
        );
    }
    let private_key = std::sync::Arc::new(sulion::node_protocol::client::load_private_key(
        &client_config.private_key_path,
    )?);
    let boot_id = uuid::Uuid::new_v4();

    // The identity key plus an operator's approval is the entire bootstrap.
    // Everything the node needs to reach a running state arrives over that
    // authenticated channel, so no secret is ever placed on this machine by
    // hand. Runs before the database connection because the credentials for it
    // are among the things being delivered, and while still root because the
    // delivered file is root-owned host state.
    // Nothing below is allowed to end the process. A node may be started before
    // its control plane exists, before the control plane has migrated, or while
    // the control plane is mid-deploy; in every one of those cases it must sit
    // and retry rather than exit and leave the enclave dark. Deploy order is
    // therefore not something anyone has to think about.
    let mut client_config = client_config;
    client_config.control_url = retry_forever("node bootstrap", || {
        bootstrap_runtime_config(&client_config, &private_key, boot_id)
    })
    .await;
    tracing::info!(control_url = %client_config.control_url, "node control channel selected");

    let config = sulion::config::Config::from_env()?;
    let pool = retry_forever("database connection", || {
        sulion::db::connect_and_wait_for_migrations(&config.db_url, "node")
    })
    .await;
    drop_node_privileges()?;
    let runtime = sulion::node_runtime::NodeRuntime::new(
        client_config.node_id,
        boot_id,
        pool.clone(),
        config.repos_root,
        config.workspaces_root,
    );
    runtime.clone().run_background_managers().await;

    let correlate_socket = config.correlate_sock_path;
    tokio::spawn(async move {
        if let Err(error) = sulion::correlate::run(pool, correlate_socket).await {
            tracing::error!(%error, "node correlation socket exited");
        }
    });
    sulion::node_protocol::client::run_with_key(client_config, runtime, private_key).await
}

/// Backoff between startup attempts. Long enough not to spin against a control
/// plane that is still deploying, short enough that a node is up promptly once
/// the other end appears.
const STARTUP_RETRY: std::time::Duration = std::time::Duration::from_secs(5);

/// Runs a fallible startup step until it succeeds.
///
/// Startup steps depend on things outside this machine — the control plane
/// answering, having been approved, having migrated the database — and none of
/// those failing is a reason to exit. Exiting would hand the problem to Docker's
/// restart policy, losing the backoff and the explanation with it.
async fn retry_forever<T, E, F, Fut>(what: &str, mut attempt: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let mut announced = false;
    loop {
        match attempt().await {
            Ok(value) => {
                if announced {
                    tracing::info!(step = what, "startup step succeeded after retrying");
                }
                return value;
            }
            Err(error) => {
                if !announced {
                    tracing::warn!(step = what, %error, "startup step failed; retrying until it succeeds");
                    announced = true;
                }
                tokio::time::sleep(STARTUP_RETRY).await;
            }
        }
    }
}

/// Writes the granted peering so the host can bring the tunnel up.
///
/// The node cannot configure its own interface — it does not own the host
/// network namespace — so it renders the configuration into root-owned state
/// and a host path unit applies it, the same shape as delivered credentials.
fn apply_tunnel_peering(peering: &sulion::node_protocol::TunnelPeering) -> anyhow::Result<bool> {
    use sulion::node_protocol::tunnel;

    let keypair = tunnel::TunnelKeypair::load_or_create(&tunnel::node_key_path())?;
    let path = tunnel::node_config_path();
    let rendered = tunnel::render_node_config(&keypair, peering);
    tunnel::write_node_config(&path, &rendered)
}

/// How long to wait between re-enrollments while the host has not yet activated
/// delivered configuration. The host rebuilds this container when it does, so
/// this loop exists only so a failed activation retries instead of wedging.
const ACTIVATION_POLL: std::time::Duration = std::time::Duration::from_secs(30);
/// Shorter, because bringing an interface up is fast and the node is otherwise
/// idle waiting for credentials it can only receive over that interface.
const TUNNEL_ACTIVATION_POLL: std::time::Duration = std::time::Duration::from_secs(3);

/// Fetches runtime configuration over the node channel and writes it to
/// root-owned host state.
///
/// Returns once this container's own environment already reflects the delivered
/// configuration. Until then it keeps the delivered file current and waits for
/// the host activation step to rebuild the stack around it.
async fn bootstrap_runtime_config(
    client_config: &sulion::node_protocol::client::NodeClientConfig,
    private_key: &ring::signature::Ed25519KeyPair,
    boot_id: uuid::Uuid,
) -> anyhow::Result<String> {
    use sulion::node_protocol::NodeRuntimeConfig;

    let path = NodeRuntimeConfig::delivered_path();
    // Enrollment starts on the cleartext bootstrap address because the tunnel
    // cannot exist yet, and moves to the tunnel the moment one is granted.
    // Only public keys ever cross the cleartext hop.
    let mut url = client_config.control_url.clone();
    loop {
        let enrollment = sulion::node_protocol::client::await_runtime_config(
            client_config,
            private_key,
            boot_id,
            &url,
        )
        .await;

        if let Some(peering) = enrollment.peering.as_ref() {
            if apply_tunnel_peering(peering)? {
                tracing::info!(
                    node_address = %peering.node_address,
                    "tunnel peering written; retrying enrollment over the tunnel",
                );
            }
            url = peering.control_url.clone();
        }

        let Some(delivered) = enrollment.config else {
            if enrollment.peering.is_some() {
                // Credentials are withheld until the connection arrives over
                // the tunnel. Give the host activation a moment, then retry
                // against the tunnel address.
                tokio::time::sleep(TUNNEL_ACTIVATION_POLL).await;
                continue;
            }
            tracing::info!(
                "control plane delivers no node configuration; using the local environment"
            );
            return Ok(url);
        };

        if delivered.write_delivered(&path)? {
            tracing::info!(
                path = %path.display(),
                digest = %delivered.digest(),
                keys = ?delivered.key_names(),
                "wrote delivered node runtime configuration",
            );
        }
        if delivered.matches_current_env() {
            return Ok(url);
        }
        tracing::info!(
            digest = %delivered.digest(),
            "delivered configuration is newer than this container's environment; \
             waiting for the host to activate it",
        );
        tokio::time::sleep(ACTIVATION_POLL).await;
    }
}

fn drop_node_privileges() -> anyhow::Result<()> {
    use std::os::unix::fs::MetadataExt;

    if unsafe { libc::geteuid() } != 0 {
        return Ok(());
    }
    let user = std::env::var("SULION_NODE_RUN_USER").unwrap_or_else(|_| "dev".into());
    let uid: libc::uid_t = std::env::var("SULION_NODE_RUN_UID")
        .map_err(|_| anyhow::anyhow!("root node requires SULION_NODE_RUN_UID"))?
        .parse()
        .map_err(|_| anyhow::anyhow!("SULION_NODE_RUN_UID must be an integer"))?;
    let gid: libc::gid_t = std::env::var("SULION_NODE_RUN_GID")
        .map_err(|_| anyhow::anyhow!("root node requires SULION_NODE_RUN_GID"))?
        .parse()
        .map_err(|_| anyhow::anyhow!("SULION_NODE_RUN_GID must be an integer"))?;
    let user = std::ffi::CString::new(user)
        .map_err(|_| anyhow::anyhow!("SULION_NODE_RUN_USER contains a NUL byte"))?;
    if unsafe { libc::initgroups(user.as_ptr(), gid) } != 0 {
        return Err(anyhow::Error::from(std::io::Error::last_os_error()));
    }
    if std::env::var("SULION_DOCKER_MODE").is_ok_and(|value| value == "direct") {
        let docker_host =
            std::env::var("DOCKER_HOST").unwrap_or_else(|_| "unix:///var/run/docker.sock".into());
        let socket_path = docker_host
            .strip_prefix("unix://")
            .ok_or_else(|| anyhow::anyhow!("direct Docker requires a unix:// DOCKER_HOST"))?;
        let docker_gid = std::fs::metadata(socket_path)
            .map_err(|error| anyhow::anyhow!("read Docker socket {socket_path}: {error}"))?
            .gid() as libc::gid_t;
        add_supplementary_group(docker_gid)?;
    }
    if unsafe { libc::setgid(gid) } != 0 {
        return Err(anyhow::Error::from(std::io::Error::last_os_error()));
    }
    if unsafe { libc::setuid(uid) } != 0 {
        return Err(anyhow::Error::from(std::io::Error::last_os_error()));
    }
    if unsafe { libc::geteuid() } != uid || unsafe { libc::getegid() } != gid {
        anyhow::bail!("node privilege drop did not reach the configured identity");
    }
    Ok(())
}

fn add_supplementary_group(gid: libc::gid_t) -> anyhow::Result<()> {
    let count = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
    if count < 0 {
        return Err(anyhow::Error::from(std::io::Error::last_os_error()));
    }
    let mut groups = vec![0 as libc::gid_t; count as usize];
    if count > 0 && unsafe { libc::getgroups(count, groups.as_mut_ptr()) } < 0 {
        return Err(anyhow::Error::from(std::io::Error::last_os_error()));
    }
    if !groups.contains(&gid) {
        groups.push(gid);
        if unsafe { libc::setgroups(groups.len(), groups.as_ptr()) } != 0 {
            return Err(anyhow::Error::from(std::io::Error::last_os_error()));
        }
    }
    Ok(())
}
