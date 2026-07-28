//! Environment a PTY shell inherits.
//!
//! Split from the manager because what a shell can see is a policy question of
//! its own: it decides which credentials, roots, and helper paths reach an
//! agent's session, and it is read far more often than the spawn machinery
//! around it.

use std::path::{Path, PathBuf};

use portable_pty::CommandBuilder;
use uuid::Uuid;

use super::PtyWorkspaceMetadata;

pub(super) fn configure_pty_environment(
    cmd: &mut CommandBuilder,
    id: Uuid,
    shell: &Path,
    secret_broker_key_path: Option<&PathBuf>,
    workspace: Option<&PtyWorkspaceMetadata>,
) {
    cmd.env_clear();

    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/dev".to_string());
    let user = std::env::var("USER").unwrap_or_else(|_| "dev".to_string());
    cmd.env("HOME", &home);
    cmd.env("USER", &user);
    cmd.env("LOGNAME", &user);
    cmd.env("SHELL", shell.as_os_str());
    cmd.env("PATH", default_pty_path(&home));

    for key in [
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "TZ",
        "COLORTERM",
        "RUSTUP_HOME",
        "CARGO_HOME",
        "RUSTUP_TOOLCHAIN",
        "DOTNET_ROOT",
        "DOTNET_CLI_TELEMETRY_OPTOUT",
        "DOTNET_NOLOGO",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ] {
        forward_env(cmd, key);
    }

    cmd.env("SULION_PTY_ID", id.to_string());
    if let Some(workspace) = workspace {
        cmd.env("SULION_REPO_NAME", &workspace.repo_name);
        cmd.env("SULION_WORKSPACE_ID", workspace.id.to_string());
        cmd.env("SULION_WORKSPACE_KIND", &workspace.kind);
        cmd.env(
            "SULION_WORKSPACE_PATH",
            workspace.path.to_string_lossy().as_ref(),
        );
        cmd.env(
            "SULION_CANONICAL_REPO",
            std::env::var("SULION_REPOS_ROOT")
                .map(|root| PathBuf::from(root).join(&workspace.repo_name))
                .unwrap_or_else(|_| PathBuf::from("/home/dev/repos").join(&workspace.repo_name))
                .to_string_lossy()
                .as_ref(),
        );
        if let Some(branch) = &workspace.branch_name {
            cmd.env("SULION_BRANCH", branch);
        }
        if let Some(base_ref) = &workspace.base_ref {
            cmd.env("SULION_BASE_REF", base_ref);
        }
        if let Some(base_sha) = &workspace.base_sha {
            cmd.env("SULION_BASE_SHA", base_sha);
        }
        if let Some(merge_target) = &workspace.merge_target {
            cmd.env("SULION_MERGE_TARGET", merge_target);
        }
    }
    cmd.env(
        "SULION_CORRELATE_SOCK",
        std::env::var("SULION_CORRELATE_SOCK")
            .unwrap_or_else(|_| "/run/sulion/correlate.sock".to_string()),
    );
    for key in [
        "SULION_CLAUDE_PROJECTS",
        "SULION_CODEX_SESSIONS",
        "SULION_SECRET_BROKER_URL",
        "SULION_RETRIEVAL_URL",
        "SULION_RETRIEVAL_TOKEN",
        "SULION_CODE_INTEL_URL",
        "SULION_CODE_INTEL_TOKEN",
        "SULION_RUNNER_URL",
        "SULION_DOCKER_MODE",
        "DOCKER_HOST",
        "XDG_RUNTIME_DIR",
    ] {
        forward_env(cmd, key);
    }
    if let Some(path) = secret_broker_key_path {
        cmd.env(
            "SULION_SECRET_BROKER_KEY_PATH",
            path.to_string_lossy().as_ref(),
        );
    }
}

fn default_pty_path(home: &str) -> String {
    let base = std::env::var("PATH")
        .unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin:/usr/local/sbin:/usr/sbin:/sbin".into());
    format!("{home}/.local/bin:/opt/sulion/bin:{base}")
}

fn forward_env(cmd: &mut CommandBuilder, key: &str) {
    if let Ok(value) = std::env::var(key) {
        cmd.env(key, value);
    }
}
