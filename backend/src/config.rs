use std::net::SocketAddr;
use std::path::PathBuf;

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
    pub auth: Option<AuthConfig>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let listen: SocketAddr = std::env::var("SULION_LISTEN")
            .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
            .parse()?;
        let db_url = std::env::var("SULION_DB_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .map_err(|_| anyhow::anyhow!("SULION_DB_URL or DATABASE_URL must be set"))?;
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
        let auth = AuthConfig::from_env()?;
        Ok(Self {
            listen,
            db_url,
            repos_root,
            workspaces_root,
            library_root,
            claude_projects_dir,
            codex_sessions_dir,
            correlate_sock_path,
            auth,
        })
    }
}

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub issuer_url: String,
    pub client_id: String,
}

impl AuthConfig {
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let issuer_url = match std::env::var("SULION_AUTH_ISSUER_URL") {
            Ok(value) => value.trim().trim_end_matches('/').to_string(),
            Err(_) => return Ok(None),
        };
        let client_id = std::env::var("SULION_AUTH_CLIENT_ID").map_err(|_| {
            anyhow::anyhow!("SULION_AUTH_CLIENT_ID must be set when auth is enabled")
        })?;
        Ok(Some(Self {
            issuer_url,
            client_id,
        }))
    }
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/home/dev"))
}
