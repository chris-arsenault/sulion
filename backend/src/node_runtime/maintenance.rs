//! One storage-maintenance owner per direct-Docker development node.

use std::path::PathBuf;
use std::time::Duration;

pub(super) fn start(repos_root: PathBuf) {
    if !std::env::var("SULION_DOCKER_MODE").is_ok_and(|mode| mode == "direct") {
        return;
    }
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let mut command = tokio::process::Command::new("python3");
            command
                .arg("/opt/sulion/storage-maintenance.py")
                .arg("--apply")
                .arg("--repos-root")
                .arg(&repos_root)
                .kill_on_drop(true);
            if let Some(root) = std::env::var_os("SULION_WORKSPACES_ROOT") {
                command.arg("--workspaces-root").arg(root);
            }
            match command.status().await {
                Ok(status) if status.success() => {
                    tracing::info!("node storage maintenance completed");
                }
                Ok(status) => {
                    tracing::warn!(%status, "node storage maintenance failed; see maintenance logs");
                }
                Err(error) => {
                    tracing::warn!(%error, "could not start node storage maintenance");
                }
            }
        }
    });
}
