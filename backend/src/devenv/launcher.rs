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
    {
        let link = link.clone();
        tokio::spawn(async move {
            if let Err(error) = link.run_listener(socket_path).await {
                tracing::error!(%error, "devenv listener exited");
            }
        });
    }
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
                link,
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
    sibling_binary_path("sulion-devenv")
}

/// A binary shipped next to this process in the same image, with a PATH
/// lookup as the fallback for test and development layouts.
fn sibling_binary_path(name: &str) -> PathBuf {
    if let Ok(current) = std::env::current_exe() {
        if let Some(dir) = current.parent() {
            let sibling = dir.join(name);
            if sibling.is_file() {
                return sibling;
            }
        }
    }
    PathBuf::from(name)
}

/// Container mode: resolve the current toolset's image identity, publish it
/// as the target for new sessions, and make sure exactly that container
/// exists and runs — leaving every other devenv container untouched, because
/// they are still serving the sessions that started on them. Docker's
/// restart policy handles crashes; this loop handles absence.
async fn ensure_container_loop(
    link: Arc<DevenvLink>,
    socket_path: PathBuf,
    image: String,
    home_host_path: String,
    run_volume: String,
) {
    let mut published_ident: Option<String> = None;
    loop {
        match ensure_current_container(&socket_path, &image, &home_host_path, &run_volume).await {
            Ok(image_id) => {
                if published_ident.as_deref() != Some(image_id.as_str()) {
                    // Publish only once the devenv has dialed in under this
                    // identity: adoption and `docker run` both report success
                    // before the server inside has connected, and routing new
                    // sessions at a devenv that never shows up strands every
                    // spawn while the previous target still works.
                    if wait_for_connection(&link, &image_id, CONNECT_PUBLISH_TIMEOUT).await {
                        link.set_current_ident(Some(image_id.clone())).await;
                        published_ident = Some(image_id.clone());
                    } else {
                        tracing::error!(
                            ident = %image_id,
                            "devenv container is running but has not connected; \
                             new sessions stay on the previous devenv"
                        );
                    }
                }
                // Reap against the ensured identity, not the published one: a
                // current container whose hello is still pending must never
                // read as reapable.
                if let Err(error) = reap_emptied_containers(&link, &image_id).await {
                    tracing::warn!(%error, "devenv reap pass failed");
                }
            }
            Err(error) => {
                // Without knowing the current identity there is no safe reap
                // decision either.
                tracing::error!(%error, "devenv container ensure failed; retrying");
            }
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

const CONNECT_PUBLISH_TIMEOUT: Duration = Duration::from_secs(30);

/// True once the link holds a connection announced under `ident`. The devenv
/// dials on its own schedule (reconnect backoff caps at 5s), so publication
/// waits for the hello rather than assuming container-running means routable.
async fn wait_for_connection(link: &Arc<DevenvLink>, ident: &str, deadline: Duration) -> bool {
    let end = tokio::time::Instant::now() + deadline;
    loop {
        if link.is_connected(ident).await {
            return true;
        }
        if tokio::time::Instant::now() >= end {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Removes label-owned devenv containers that are not the current toolset
/// and host nothing. New sessions only land on the current devenv and
/// upgrades only move sessions to it, so an emptied non-current container
/// can never become non-empty again — there is nothing a retention window
/// would preserve. A stopped non-current container's shells are already
/// gone and goes regardless.
async fn reap_emptied_containers(
    link: &Arc<DevenvLink>,
    current_image_id: &str,
) -> anyhow::Result<()> {
    let output = tokio::process::Command::new("docker")
        .args([
            "ps",
            "-a",
            "--filter",
            &format!("label={OWNER_LABEL}"),
            "--format",
            "{{.Names}}",
        ])
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "docker ps failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let names: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(String::from)
        .collect();
    for name in names {
        let Some(state) = inspect_container(&name).await? else {
            continue;
        };
        if !state.owned {
            continue;
        }
        // Phase-1 containers carry no image-id label and host the
        // default-ident sessions.
        let ident = state
            .image_id
            .as_deref()
            .unwrap_or(super::link::DEFAULT_IDENT);
        let is_current = state.image_id.as_deref() == Some(current_image_id);
        let connected = link.is_connected(ident).await;
        let hosted = link.hosted_sessions(ident).await;
        if should_reap(state.running, is_current, connected, hosted) {
            tracing::info!(%name, hosted, running = state.running, "reaping emptied devenv container");
            run_docker(&["rm", "-f", &name]).await?;
        }
    }
    Ok(())
}

/// The reap decision, kept pure for testing: never the current container,
/// never one still hosting sessions; stopped non-current containers always.
/// A running container that has not connected has an unknown inventory — a
/// fresh node boot sees hosted == 0 for every devenv until the hellos arrive,
/// and reaping on that emptiness would kill shells mid-reconnect.
fn should_reap(running: bool, is_current: bool, connected: bool, hosted: usize) -> bool {
    if is_current {
        return false;
    }
    if !running {
        return true;
    }
    connected && hosted == 0
}

/// Ensures the container for the current image identity and returns that
/// identity. Per decision 5 the identity is the image ID: the `:sha` tag
/// changes every release, while the ID changes exactly when toolset content
/// does — so a backend-only release resolves to the already-running
/// container and rolls nothing.
async fn ensure_current_container(
    socket_path: &std::path::Path,
    image: &str,
    home_host_path: &str,
    run_volume: &str,
) -> anyhow::Result<String> {
    let image_id = resolve_image_id(image).await?;
    let name = container_name_for(&image_id);
    if let Some(state) = inspect_container(&name).await? {
        if !state.owned {
            anyhow::bail!(
                "container {name} exists without the {OWNER_LABEL} label; refusing to adopt it"
            );
        }
        if state.running {
            return Ok(image_id);
        }
        // A stopped devenv's shells are already gone; replacing it loses
        // nothing.
        tracing::warn!(%name, "devenv container exists but is not running; replacing it");
        run_docker(&["rm", "-f", &name]).await?;
    }
    // Deliver this node's per-commit binaries through the shared run
    // volume; the image itself carries only the toolset. The server binary
    // is exec'd at its versioned path; the CLI additionally gets a stable
    // symlink because the image's /usr/local/bin/sulion points at it.
    let exec_path = match deliver_versioned_binary(socket_path, "sulion-devenv").await {
        Ok(path) => path,
        Err(error) => {
            tracing::warn!(%error, "devenv binary delivery failed; using the image's own binary");
            PathBuf::from("/usr/local/bin/sulion-devenv")
        }
    };
    if let Err(error) = deliver_cli_binary(socket_path).await {
        tracing::warn!(%error, "sulion CLI delivery failed; PTY wrappers may not resolve");
    }
    let args = container_run_args(
        socket_path,
        image,
        home_host_path,
        run_volume,
        &image_id,
        &exec_path,
    );
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run_docker(&args).await?;
    tracing::info!(%image, %image_id, "devenv container started");
    Ok(image_id)
}

/// The image ID of a reference, pulling it if the daemon does not have it.
async fn resolve_image_id(image: &str) -> anyhow::Result<String> {
    if let Some(id) = inspect_image_id(image).await? {
        return Ok(id);
    }
    run_docker(&["pull", image]).await?;
    inspect_image_id(image)
        .await?
        .ok_or_else(|| anyhow::anyhow!("image {image} has no ID after pull"))
}

async fn inspect_image_id(image: &str) -> anyhow::Result<Option<String>> {
    let output = tokio::process::Command::new("docker")
        .args(["image", "inspect", "--format", "{{.Id}}", image])
        .output()
        .await?;
    if !output.status.success() {
        return Ok(None);
    }
    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!id.is_empty()).then_some(id))
}

/// Copies one of this node's binaries onto the run volume at a
/// content-hashed, write-once path and returns it — the same path inside the
/// container, which mounts the same volume at the same place.
async fn deliver_versioned_binary(
    socket_path: &std::path::Path,
    name: &str,
) -> anyhow::Result<PathBuf> {
    let source = if name == "sulion-devenv" {
        child_binary_path()
    } else {
        sibling_binary_path(name)
    };
    let bytes = tokio::fs::read(&source)
        .await
        .map_err(|err| anyhow::anyhow!("read {}: {err}", source.display()))?;
    let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
    let hash: String = digest.as_ref()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let run_root = socket_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("devenv socket path has no parent"))?;
    let target = delivered_binary_path(run_root, &hash, name);
    if !tokio::fs::try_exists(&target).await.unwrap_or(false) {
        let dir = target
            .parent()
            .ok_or_else(|| anyhow::anyhow!("delivered path has no parent"))?;
        tokio::fs::create_dir_all(dir).await?;
        // Staging + rename so a concurrent launch never sees a half-written
        // binary at the versioned path, which is never overwritten in place.
        let staging = dir.join(format!(".staging-{name}"));
        tokio::fs::write(&staging, &bytes).await?;
        let mut perms = tokio::fs::metadata(&staging).await?.permissions();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o755);
        tokio::fs::set_permissions(&staging, perms).await?;
        tokio::fs::rename(&staging, &target).await?;
    }
    Ok(target)
}

/// Delivers the `sulion` CLI and points the stable `<run>/bin/sulion`
/// symlink at it — the toolset image's /usr/local/bin/sulion resolves
/// through that link. Swapped atomically; the CLI's node-facing protocols
/// are additive, so running shells tolerate the version moving forward.
async fn deliver_cli_binary(socket_path: &std::path::Path) -> anyhow::Result<()> {
    let target = deliver_versioned_binary(socket_path, "sulion").await?;
    let run_root = socket_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("devenv socket path has no parent"))?;
    let bin_dir = run_root.join("bin");
    tokio::fs::create_dir_all(&bin_dir).await?;
    let staging = bin_dir.join(".sulion.next");
    let _ = tokio::fs::remove_file(&staging).await;
    tokio::fs::symlink(&target, &staging).await?;
    tokio::fs::rename(&staging, bin_dir.join("sulion")).await?;
    Ok(())
}

fn delivered_binary_path(run_root: &std::path::Path, hash: &str, name: &str) -> PathBuf {
    run_root.join("devenv-bin").join(hash).join(name)
}

/// One container per toolset image identity.
fn container_name_for(image_id: &str) -> String {
    let short: String = image_id
        .trim_start_matches("sha256:")
        .chars()
        .take(12)
        .collect();
    format!("{CONTAINER_NAME}-{short}")
}

struct ContainerState {
    running: bool,
    owned: bool,
    /// The `sulion.devenv.image-id` label; absent on phase-1 containers.
    image_id: Option<String>,
}

async fn inspect_container(name: &str) -> anyhow::Result<Option<ContainerState>> {
    let output = tokio::process::Command::new("docker")
        .args(["inspect", name])
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
    let image_id = entry
        .pointer("/Config/Labels")
        .and_then(|labels| labels.get(format!("{OWNER_LABEL}.image-id")))
        .and_then(|value| value.as_str())
        .map(String::from);
    Ok(Some(ContainerState {
        running,
        owned,
        image_id,
    }))
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
    image_id: &str,
    exec_path: &std::path::Path,
) -> Vec<String> {
    vec![
        "run".into(),
        "-d".into(),
        "--name".into(),
        container_name_for(image_id),
        "--label".into(),
        format!("{OWNER_LABEL}=1"),
        "--label".into(),
        format!("{OWNER_LABEL}.image-id={image_id}"),
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
        "-e".into(),
        format!("SULION_DEVENV_IDENT={image_id}"),
        "-v".into(),
        format!("{home_host_path}:/home/sulion"),
        "-v".into(),
        format!("{run_volume}:/run/sulion"),
        // The dedicated node's whole reason to exist is giving shells direct
        // Docker; the PTY environment advertises it (SULION_DOCKER_MODE=
        // direct + DOCKER_HOST), so the socket must be where it points.
        // This launcher only runs in direct mode — child-mode devenvs never
        // see a daemon.
        "-v".into(),
        "/var/run/docker.sock:/var/run/docker.sock".into(),
        "--entrypoint".into(),
        "/usr/bin/dumb-init".into(),
        image.into(),
        "--".into(),
        // Through the image entrypoint: it seeds the home-directory config
        // PTY sessions rely on, then execs its argument — the server binary
        // this node delivered onto the run volume.
        "/opt/sulion/entrypoint.sh".into(),
        exec_path.display().to_string(),
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
    fn container_args_carry_identity_label_mounts_and_host_network() {
        let args = container_run_args(
            std::path::Path::new("/run/sulion/devenv.sock"),
            "ghcr.io/example/devenv:abc",
            "/home/sulion",
            "sulion_sulion_run",
            "sha256:0123456789abcdef",
            std::path::Path::new("/run/sulion/devenv-bin/cafe/sulion-devenv"),
        );
        let joined = args.join(" ");
        assert!(joined.contains("--name sulion-devenv-0123456789ab"));
        assert!(joined.contains("--label sulion.devenv=1"));
        assert!(joined.contains("--label sulion.devenv.image-id=sha256:0123456789abcdef"));
        assert!(joined.contains("--network host"));
        assert!(joined.contains("-v /home/sulion:/home/sulion"));
        assert!(joined.contains("-v sulion_sulion_run:/run/sulion"));
        assert!(joined.contains("-v /var/run/docker.sock:/var/run/docker.sock"));
        assert!(joined.contains("--restart unless-stopped"));
        assert!(joined.contains("-e SULION_DEVENV_SOCK=/run/sulion/devenv.sock"));
        assert!(joined.contains("-e SULION_DEVENV_IDENT=sha256:0123456789abcdef"));
        assert!(joined.ends_with(
            "ghcr.io/example/devenv:abc -- /opt/sulion/entrypoint.sh /run/sulion/devenv-bin/cafe/sulion-devenv"
        ));
    }

    #[test]
    fn container_names_and_delivery_paths_are_versioned() {
        assert_eq!(
            container_name_for("sha256:deadbeefcafe0123"),
            "sulion-devenv-deadbeefcafe"
        );
        assert_eq!(
            delivered_binary_path(
                std::path::Path::new("/run/sulion"),
                "0011aabb",
                "sulion-devenv"
            ),
            std::path::PathBuf::from("/run/sulion/devenv-bin/0011aabb/sulion-devenv")
        );
        assert_eq!(
            delivered_binary_path(std::path::Path::new("/run/sulion"), "0011aabb", "sulion"),
            std::path::PathBuf::from("/run/sulion/devenv-bin/0011aabb/sulion")
        );
    }

    #[test]
    fn reaps_only_emptied_non_current_containers() {
        // The current container is never reaped, whatever its state.
        assert!(!should_reap(true, true, true, 0));
        assert!(!should_reap(false, true, false, 0));
        // A running non-current container survives while it hosts sessions.
        assert!(!should_reap(true, false, true, 3));
        assert!(should_reap(true, false, true, 0));
        // Running but not connected: inventory unknown (a fresh node boot
        // sees zero hosted for everything until the hellos land) — keep it.
        assert!(!should_reap(true, false, false, 0));
        // A stopped non-current container's shells are already gone.
        assert!(should_reap(false, false, false, 5));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn publication_waits_for_the_devenv_hello() {
        let (link, _events) = DevenvLink::new();
        assert!(
            !wait_for_connection(&link, "sha256:abc", Duration::from_millis(300)).await,
            "no connection must mean no publication"
        );
        let server = Arc::new(crate::devenv::server::DevenvServer::with_ident(Some(
            "sha256:abc".into(),
        )));
        let (node_side, devenv_side) = tokio::io::duplex(64 * 1024);
        tokio::spawn(server.serve(devenv_side));
        tokio::spawn(link.clone().handle_connection(node_side));
        assert!(wait_for_connection(&link, "sha256:abc", Duration::from_secs(10)).await);
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
