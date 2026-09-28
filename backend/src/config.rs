use std::net::SocketAddr;
use std::path::PathBuf;

/// An environment variable's value, trimmed, treating unset and blank alike.
///
/// Every service had its own copy of this, byte for byte. The `env_required`
/// wrappers around it stay per-service: they differ in error type and in
/// whether they distinguish "unset" from "empty", which is a real difference
/// rather than duplication.
pub fn env_optional(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub db_url: String,
    pub repos_root: PathBuf,
    pub workspaces_root: PathBuf,
    pub library_root: PathBuf,
    pub claude_projects_dir: PathBuf,
    pub codex_sessions_dir: PathBuf,
    pub correlate_sock_path: PathBuf,
    pub standalone_node: Option<StandaloneNodeConfig>,
    pub auth: Option<AuthConfig>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Self::read_env(AuthConfig::from_env()?)
    }

    /// Node and ingester binaries do not serve browser requests.
    pub fn worker_from_env() -> anyhow::Result<Self> {
        Self::read_env(None)
    }

    fn read_env(auth: Option<AuthConfig>) -> anyhow::Result<Self> {
        let listen: SocketAddr = std::env::var("SULION_LISTEN")
            .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
            .parse()?;
        let db_url = std::env::var("SULION_DB_URL")
            .map_err(|_| anyhow::anyhow!("SULION_DB_URL must be set"))?;
        let repos_root = PathBuf::from(
            std::env::var("SULION_REPOS_ROOT")
                .unwrap_or_else(|_| dirs_home().join("repos").to_string_lossy().into_owned()),
        );
        let workspaces_root =
            PathBuf::from(std::env::var("SULION_WORKSPACES_ROOT").unwrap_or_else(|_| {
                dirs_home()
                    .join(".sulion/workspaces")
                    .to_string_lossy()
                    .into_owned()
            }));
        let library_root =
            PathBuf::from(std::env::var("SULION_LIBRARY_ROOT").unwrap_or_else(|_| {
                dirs_home()
                    .join(".sulion/library")
                    .to_string_lossy()
                    .into_owned()
            }));
        let claude_projects_dir =
            PathBuf::from(std::env::var("SULION_CLAUDE_PROJECTS").unwrap_or_else(|_| {
                dirs_home()
                    .join(".claude/projects")
                    .to_string_lossy()
                    .into_owned()
            }));
        let codex_sessions_dir =
            PathBuf::from(std::env::var("SULION_CODEX_SESSIONS").unwrap_or_else(|_| {
                dirs_home()
                    .join(".codex/sessions")
                    .to_string_lossy()
                    .into_owned()
            }));
        // Persist resolved paths back to the process env so pty.rs
        // forwards them into spawned shells even when the operator
        // didn't set them explicitly.
        std::env::set_var("SULION_REPOS_ROOT", &repos_root);
        std::env::set_var("SULION_WORKSPACES_ROOT", &workspaces_root);
        std::env::set_var("SULION_CLAUDE_PROJECTS", &claude_projects_dir);
        std::env::set_var("SULION_CODEX_SESSIONS", &codex_sessions_dir);
        let correlate_sock_path = PathBuf::from(
            std::env::var("SULION_CORRELATE_SOCK")
                .unwrap_or_else(|_| "/run/sulion/correlate.sock".to_string()),
        );
        let standalone_node = StandaloneNodeConfig::from_env()?;
        Ok(Self {
            listen,
            db_url,
            repos_root,
            workspaces_root,
            library_root,
            claude_projects_dir,
            codex_sessions_dir,
            correlate_sock_path,
            standalone_node,
            auth,
        })
    }
}

#[derive(Debug, Clone)]
pub struct StandaloneNodeConfig {
    pub node_id: uuid::Uuid,
    pub display_name: String,
}

impl StandaloneNodeConfig {
    fn from_env() -> anyhow::Result<Option<Self>> {
        let default_transport = match std::env::var("SULION_DEPLOYMENT_ROLE").as_deref() {
            Ok("control-plane") => "remote",
            _ => "loopback",
        };
        let transport = std::env::var("SULION_NODE_TRANSPORT")
            .unwrap_or_else(|_| default_transport.to_string());
        match transport.as_str() {
            "remote" => Ok(None),
            "loopback" => {
                let node_id = std::env::var("SULION_STANDALONE_NODE_ID")
                    .unwrap_or_else(|_| "00000000-0000-0000-0000-000000000001".into())
                    .parse()
                    .map_err(|_| anyhow::anyhow!("SULION_STANDALONE_NODE_ID must be a UUID"))?;
                let display_name = std::env::var("SULION_STANDALONE_NODE_NAME")
                    .unwrap_or_else(|_| "standalone".into());
                if display_name.trim().is_empty() {
                    anyhow::bail!("SULION_STANDALONE_NODE_NAME must not be empty");
                }
                Ok(Some(Self {
                    node_id,
                    display_name,
                }))
            }
            _ => anyhow::bail!("SULION_NODE_TRANSPORT must be loopback or remote"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub issuer_url: String,
    pub client_id: String,
}

impl AuthConfig {
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        Self::parse(
            env_optional("SULION_DEPLOYMENT_ROLE").as_deref(),
            env_optional("SULION_AUTH_MODE").as_deref(),
            env_optional("SULION_AUTH_ISSUER_URL").as_deref(),
            env_optional("SULION_AUTH_CLIENT_ID").as_deref(),
        )
    }

    fn parse(
        role: Option<&str>,
        mode: Option<&str>,
        issuer: Option<&str>,
        client: Option<&str>,
    ) -> anyhow::Result<Option<Self>> {
        let development = matches!(role, Some("development" | "test"));
        if mode == Some("disabled") && development {
            return Ok(None);
        }
        anyhow::ensure!(
            mode.is_none() || mode == Some("required"),
            "authentication bypass requires an explicit development or test role"
        );
        let issuer = issuer
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("SULION_AUTH_ISSUER_URL must be set"))?;
        let client = client
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("SULION_AUTH_CLIENT_ID must be set"))?;
        let url = url::Url::parse(issuer)?;
        anyhow::ensure!(
            url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "invalid auth issuer URL"
        );
        anyhow::ensure!(
            url.scheme() == "https" || (development && url.scheme() == "http"),
            "auth issuer must use HTTPS in production"
        );
        Ok(Some(Self {
            issuer_url: issuer.trim_end_matches('/').into(),
            client_id: client.trim().into(),
        }))
    }
}

#[cfg(test)]
mod auth_tests {
    use super::AuthConfig;
    #[test]
    fn production_cannot_accidentally_disable_authentication() {
        for role in [
            None,
            Some("control-plane"),
            Some("standalone"),
            Some("broker"),
            Some("node"),
            Some("ingester"),
        ] {
            assert!(AuthConfig::parse(role, None, None, None).is_err());
            assert!(AuthConfig::parse(role, Some("disabled"), None, None).is_err());
            assert!(AuthConfig::parse(role, None, Some(" "), Some("client")).is_err());
            assert!(AuthConfig::parse(role, None, Some("http://issuer"), Some("client")).is_err());
            assert!(
                AuthConfig::parse(role, None, Some("https://issuer"), Some("client"))
                    .unwrap()
                    .is_some()
            );
        }
        assert!(
            AuthConfig::parse(Some("development"), Some("disabled"), None, None)
                .unwrap()
                .is_none()
        );
        assert!(AuthConfig::parse(
            Some("test"),
            None,
            Some("http://localhost:1234"),
            Some("test")
        )
        .unwrap()
        .is_some());
    }
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/home/sulion"))
}
