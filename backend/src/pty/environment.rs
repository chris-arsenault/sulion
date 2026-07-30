//! Environment a PTY shell inherits.
//!
//! Split from the manager because what a shell can see is a policy question of
//! its own: it decides which credentials, roots, and helper paths reach an
//! agent's session, and it is read far more often than the spawn machinery
//! around it.
//!
//! Returns plain pairs rather than mutating a `CommandBuilder`: the policy is
//! evaluated against the node's environment, but the spawn may happen in a
//! different process, so the result has to cross a wire.

use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::PtyWorkspaceMetadata;

/// The complete environment for a PTY shell. The caller starts from an empty
/// environment (`env_clear`) and applies exactly these pairs.
pub(crate) fn pty_environment(
    id: Uuid,
    shell: &Path,
    secret_broker_key_path: Option<&PathBuf>,
    workspace: Option<&PtyWorkspaceMetadata>,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();
    let mut set = |key: &str, value: String| env.push((key.to_string(), value));

    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sulion".to_string());
    let user = std::env::var("USER").unwrap_or_else(|_| "sulion".to_string());
    set("HOME", home.clone());
    set("USER", user.clone());
    set("LOGNAME", user);
    set("SHELL", shell.to_string_lossy().into_owned());
    set("PATH", default_pty_path(&home));

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
        forward_env(&mut env, key);
    }

    env.push(("SULION_PTY_ID".to_string(), id.to_string()));
    if let Some(workspace) = workspace {
        env.push(("SULION_REPO_NAME".to_string(), workspace.repo_name.clone()));
        env.push(("SULION_WORKSPACE_ID".to_string(), workspace.id.to_string()));
        env.push(("SULION_WORKSPACE_KIND".to_string(), workspace.kind.clone()));
        env.push((
            "SULION_WORKSPACE_PATH".to_string(),
            workspace.path.to_string_lossy().into_owned(),
        ));
        env.push((
            "SULION_CANONICAL_REPO".to_string(),
            std::env::var("SULION_REPOS_ROOT")
                .map(|root| PathBuf::from(root).join(&workspace.repo_name))
                .unwrap_or_else(|_| PathBuf::from("/home/sulion/repos").join(&workspace.repo_name))
                .to_string_lossy()
                .into_owned(),
        ));
        if let Some(branch) = &workspace.branch_name {
            env.push(("SULION_BRANCH".to_string(), branch.clone()));
        }
        if let Some(base_ref) = &workspace.base_ref {
            env.push(("SULION_BASE_REF".to_string(), base_ref.clone()));
        }
        if let Some(base_sha) = &workspace.base_sha {
            env.push(("SULION_BASE_SHA".to_string(), base_sha.clone()));
        }
        if let Some(merge_target) = &workspace.merge_target {
            env.push(("SULION_MERGE_TARGET".to_string(), merge_target.clone()));
        }
    }
    env.push((
        "SULION_CORRELATE_SOCK".to_string(),
        std::env::var("SULION_CORRELATE_SOCK")
            .unwrap_or_else(|_| "/run/sulion/correlate.sock".to_string()),
    ));
    for key in [
        "SULION_CLAUDE_PROJECTS",
        "SULION_CODEX_SESSIONS",
        "SULION_SECRET_BROKER_URL",
        "SULION_RETRIEVAL_URL",
        // Pinned control TLS certificate, so PTY tools reaching the broker and
        // retrieval verify the control plane's self-signed endpoint. Added as
        // an extra root; public TLS is unaffected.
        "SULION_CONTROL_TLS_CA",
        "SULION_RETRIEVAL_TOKEN",
        "SULION_CODE_INTEL_URL",
        "SULION_CODE_INTEL_TOKEN",
        "SULION_RUNNER_URL",
        "SULION_DOCKER_MODE",
        "DOCKER_HOST",
        "XDG_RUNTIME_DIR",
    ] {
        forward_env(&mut env, key);
    }
    if let Some(path) = secret_broker_key_path {
        env.push((
            "SULION_SECRET_BROKER_KEY_PATH".to_string(),
            path.to_string_lossy().into_owned(),
        ));
    }
    env.push((
        "TERM".to_string(),
        std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".to_string()),
    ));
    env
}

fn default_pty_path(home: &str) -> String {
    let base = std::env::var("PATH")
        .unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin:/usr/local/sbin:/usr/sbin:/sbin".into());
    format!("{home}/.local/bin:/opt/sulion/bin:{base}")
}

fn forward_env(env: &mut Vec<(String, String)>, key: &str) {
    if let Ok(value) = std::env::var(key) {
        env.push((key.to_string(), value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_pairs_carry_the_session_identity_and_broker_wiring() {
        let id = Uuid::new_v4();
        let key_path = PathBuf::from("/run/sulion/pty-keys/test.pk8");
        let env = pty_environment(id, Path::new("/bin/bash"), Some(&key_path), None);
        let get = |key: &str| {
            env.iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| panic!("missing env pair {key}"))
        };
        assert_eq!(get("SULION_PTY_ID"), id.to_string());
        assert_eq!(get("SHELL"), "/bin/bash");
        assert_eq!(
            get("SULION_SECRET_BROKER_KEY_PATH"),
            key_path.to_string_lossy()
        );
        assert!(get("PATH").contains("/opt/sulion/bin"));
        assert!(!get("TERM").is_empty());
        assert!(env.iter().any(|(k, _)| k == "SULION_CORRELATE_SOCK"));
    }
}
