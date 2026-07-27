#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    if let Some(command) = args.get(1).map(String::as_str) {
        match command {
            "keygen" => return keygen(&args[2..]),
            "enroll" => return enroll(&args[2..]).await,
            _ => {}
        }
    }

    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sulion=debug".into()),
        )
        .init();

    let client_config = sulion::node_protocol::client::NodeClientConfig::from_env()?;
    let config = sulion::config::Config::from_env()?;
    let pool = sulion::db::connect_and_wait_for_migrations(&config.db_url, "node").await?;
    let private_key = std::sync::Arc::new(sulion::node_protocol::client::load_private_key(
        &client_config.private_key_path,
    )?);
    drop_node_privileges()?;
    let runtime = sulion::node_runtime::NodeRuntime::new(
        client_config.node_id,
        uuid::Uuid::new_v4(),
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

fn keygen(args: &[String]) -> anyhow::Result<()> {
    let output = option(args, "--output")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("usage: sulion-node keygen --output <private-key-path>"))?;
    let public_key = sulion::node_protocol::client::generate_private_key(&output)?;
    println!("{public_key}");
    Ok(())
}

async fn enroll(args: &[String]) -> anyhow::Result<()> {
    let control_url = option(args, "--control-url").ok_or_else(|| {
        anyhow::anyhow!(
            "usage: sulion-node enroll --control-url <https-url> --token <token> --key <path>"
        )
    })?;
    let token = option(args, "--token")
        .ok_or_else(|| anyhow::anyhow!("sulion-node enroll requires --token"))?;
    let key = option(args, "--key")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("sulion-node enroll requires --key"))?;
    let enrolled = sulion::node_protocol::client::enroll(&control_url, &token, &key).await?;
    println!("{}", serde_json::to_string_pretty(&enrolled)?);
    Ok(())
}

fn option(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}
