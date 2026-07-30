//! Ensures a devenv server is running and dialing this node.
//!
//! Two launch modes, chosen by `SULION_DOCKER_MODE`:
//!
//! - **container** (`direct`): a label-owned container on the host Docker
//!   daemon, host-network, sharing the development home and the run volume.
//!   Adopted if already running — never recreated — which is what lets it
//!   outlive node releases. Not a compose service on purpose: the node
//!   deploy's `--remove-orphans` must not know it exists.
//! - **child** (anything else): the `sulion-devenv` binary as a child
//!   process, exiting on disconnect and respawned by this supervisor. No
//!   survival — the fallback for processes with no Docker daemon (e2e,
//!   loopback standalone, tests).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::link::DevenvLink;

pub const CONTAINER_NAME: &str = "sulion-devenv";
pub const OWNER_LABEL: &str = "sulion.devenv";

#[derive(Debug, Clone)]
pub enum LaunchMode {
    Container {
        image: String,
        home_host_path: String,
        run_volume: String,
    },
    Child,
}

#[derive(Debug, Clone)]
pub struct LauncherConfig {
    pub socket_path: PathBuf,
    pub mode: LaunchMode,
}

impl LauncherConfig {
    pub fn from_env() -> Self {
        let socket_path = PathBuf::from(
            std::env::var("SULION_DEVENV_SOCK")
                .unwrap_or_else(|_| "/run/sulion/devenv.sock".to_string()),
        );
        let direct = std::env::var("SULION_DOCKER_MODE").is_ok_and(|mode| mode == "direct");
        let mode = if direct {
            match std::env::var("SULION_DEVENV_IMAGE") {
                Ok(image) if !image.trim().is_empty() => LaunchMode::Container {
                    image,
                    home_host_path: std::env::var("SULION_DEVENV_HOME_HOST_PATH")
                        .unwrap_or_else(|_| "/home/sulion".to_string()),
                    run_volume: std::env::var("SULION_DEVENV_RUN_VOLUME")
                        .unwrap_or_else(|_| "sulion_sulion_run".to_string()),
                },
                _ => {
                    // Shells still work as a child process, but they will die
                    // with this node — say so loudly rather than silently
                    // downgrading the survival promise.
                    tracing::error!(
                        "SULION_DOCKER_MODE=direct but SULION_DEVENV_IMAGE is unset; \
                         falling back to a child devenv — PTYs will NOT survive node restarts"
                    );
                    LaunchMode::Child
                }
            }
        } else {
            LaunchMode::Child
        };
        Self { socket_path, mode }
    }
}

/// Starts the link's listener and whatever keeps a devenv dialing it.
pub fn start(link: Arc<DevenvLink>, config: LauncherConfig) {
    let socket_path = config.socket_path.clone();
    tokio::spawn(async move {
        if let Err(error) = link.run_listener(socket_path).await {
            tracing::error!(%error, "devenv listener exited");
        }
    });
    match config.mode {
        LaunchMode::Child => {
            tokio::spawn(supervise_child(config.socket_path));
        }
        LaunchMode::Container {
            image,
            home_host_path,
            run_volume,
        } => {
            tokio::spawn(ensure_container_loop(
                config.socket_path,
                image,
                home_host_path,
                run_volume,
            ));
        }
    }
}

/// Child mode: spawn `sulion-devenv`, respawn if it dies. It exits when the
/// connection closes, so this supervisor is also what ends it cleanly when
/// the node process goes away.
async fn supervise_child(socket_path: PathBuf) {
    let binary = child_binary_path();
    loop {
        let spawned = tokio::process::Command::new(&binary)
            .env("SULION_DEVENV_SOCK", &socket_path)
            .env("SULION_DEVENV_EXIT_ON_DISCONNECT", "1")
            .spawn();
        match spawned {
            Ok(mut child) => {
                tracing::info!(binary = %binary.display(), "devenv child started");
                match child.wait().await {
                    Ok(status) => {
                        tracing::warn!(%status, "devenv child exited; respawning")
                    }
                    Err(error) => tracing::error!(%error, "devenv child wait failed"),
                }
            }
            Err(error) => {
                tracing::error!(%error, binary = %binary.display(), "devenv child spawn failed");
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// The devenv binary ships next to the node binary in the same image; a PATH
/// lookup is the fallback for test and development layouts.
fn child_binary_path() -> PathBuf {
    if let Ok(path) = std::env::var("SULION_DEVENV_BIN") {
        return PathBuf::from(path);
    }
    if let Ok(current) = std::env::current_exe() {
        if let Some(dir) = current.parent() {
            let sibling = dir.join("sulion-devenv");
            if sibling.is_file() {
                return sibling;
            }
        }
    }
    PathBuf::from("sulion-devenv")
}

/// Container mode: make sure the label-owned container exists and runs, then
/// keep checking cheaply. Docker's restart policy handles crashes; this loop
/// handles the container being absent or stopped entirely.
async fn ensure_container_loop(
    socket_path: PathBuf,
    image: String,
    home_host_path: String,
    run_volume: String,
) {
    loop {
        if let Err(error) =
            ensure_container(&socket_path, &image, &home_host_path, &run_volume).await
        {
            tracing::error!(%error, "devenv container ensure failed; retrying");
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

async fn ensure_container(
    socket_path: &std::path::Path,
    image: &str,
    home_host_path: &str,
    run_volume: &str,
) -> anyhow::Result<()> {
    if let Some(state) = inspect_container().await? {
        if !state.owned {
            anyhow::bail!(
                "container {CONTAINER_NAME} exists without the {OWNER_LABEL} label; refusing to adopt it"
            );
        }
        if state.running {
            return Ok(());
        }
        tracing::warn!("devenv container exists but is not running; replacing it");
        run_docker(&["rm", "-f", CONTAINER_NAME]).await?;
    }
    let args = container_run_args(socket_path, image, home_host_path, run_volume);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run_docker(&args).await?;
    tracing::info!(%image, "devenv container started");
    Ok(())
}

struct ContainerState {
    running: bool,
    owned: bool,
}

async fn inspect_container() -> anyhow::Result<Option<ContainerState>> {
    let output = tokio::process::Command::new("docker")
        .args(["inspect", CONTAINER_NAME])
        .output()
        .await?;
    if !output.status.success() {
        return Ok(None);
    }
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let entry = parsed
        .get(0)
        .ok_or_else(|| anyhow::anyhow!("docker inspect returned no entries"))?;
    let running = entry
        .pointer("/State/Running")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let owned = container_labels_owned(entry);
    Ok(Some(ContainerState { running, owned }))
}

/// Label validation before adoption, same posture as the dev-postgres
/// runner: a name collision must never be treated as ours.
fn container_labels_owned(inspect_entry: &serde_json::Value) -> bool {
    inspect_entry
        .pointer("/Config/Labels")
        .and_then(|labels| labels.get(OWNER_LABEL))
        .and_then(|value| value.as_str())
        == Some("1")
}

fn container_run_args(
    socket_path: &std::path::Path,
    image: &str,
    home_host_path: &str,
    run_volume: &str,
) -> Vec<String> {
    vec![
        "run".into(),
        "-d".into(),
        "--name".into(),
        CONTAINER_NAME.into(),
        "--label".into(),
        format!("{OWNER_LABEL}=1"),
        // Restart policy covers daemon restarts and crashes; the node's
        // ensure loop covers absence. Neither ever recreates a healthy
        // running container.
        "--restart".into(),
        "unless-stopped".into(),
        // Host network so shells can bind the LAN dev ports (26000-26010)
        // exactly as they did inside the node container.
        "--network".into(),
        "host".into(),
        "-e".into(),
        format!("SULION_DEVENV_SOCK={}", socket_path.display()),
        "-v".into(),
        format!("{home_host_path}:/home/sulion"),
        "-v".into(),
        format!("{run_volume}:/run/sulion"),
        "--entrypoint".into(),
        "/usr/bin/dumb-init".into(),
        image.into(),
        "--".into(),
        // Through the image entrypoint: it seeds the home-directory config
        // PTY sessions rely on, then execs its argument.
        "/opt/sulion/entrypoint.sh".into(),
        "/usr/local/bin/sulion-devenv".into(),
    ]
}

async fn run_docker(args: &[&str]) -> anyhow::Result<()> {
    let output = tokio::process::Command::new("docker")
        .args(args)
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "docker {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_args_carry_label_mounts_and_host_network() {
        let args = container_run_args(
            std::path::Path::new("/run/sulion/devenv.sock"),
            "ghcr.io/example/backend:abc",
            "/home/sulion",
            "sulion_sulion_run",
        );
        let joined = args.join(" ");
        assert!(joined.contains("--label sulion.devenv=1"));
        assert!(joined.contains("--network host"));
        assert!(joined.contains("-v /home/sulion:/home/sulion"));
        assert!(joined.contains("-v sulion_sulion_run:/run/sulion"));
        assert!(joined.contains("--restart unless-stopped"));
        assert!(joined.contains("-e SULION_DEVENV_SOCK=/run/sulion/devenv.sock"));
        assert!(joined.ends_with(
            "ghcr.io/example/backend:abc -- /opt/sulion/entrypoint.sh /usr/local/bin/sulion-devenv"
        ));
    }

    #[test]
    fn label_validation_refuses_foreign_containers() {
        let ours: serde_json::Value = serde_json::json!({
            "Config": {"Labels": {"sulion.devenv": "1"}}
        });
        let foreign: serde_json::Value = serde_json::json!({
            "Config": {"Labels": {"com.example.other": "1"}}
        });
        let unlabeled: serde_json::Value = serde_json::json!({"Config": {}});
        assert!(container_labels_owned(&ours));
        assert!(!container_labels_owned(&foreign));
        assert!(!container_labels_owned(&unlabeled));
    }
}
