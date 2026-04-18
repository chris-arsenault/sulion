use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub db_url: String,
    pub repos_root: PathBuf,
    pub claude_projects_dir: PathBuf,
    pub correlate_sock_path: PathBuf,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let listen: SocketAddr = std::env::var("SHUTTLECRAFT_LISTEN")
            .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
            .parse()?;
        let db_url = std::env::var("SHUTTLECRAFT_DB_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .map_err(|_| anyhow::anyhow!("SHUTTLECRAFT_DB_URL or DATABASE_URL must be set"))?;
        let repos_root = PathBuf::from(
            std::env::var("SHUTTLECRAFT_REPOS_ROOT")
                .unwrap_or_else(|_| dirs_home().join("repos").to_string_lossy().into_owned()),
        );
        let claude_projects_dir = PathBuf::from(
            std::env::var("SHUTTLECRAFT_CLAUDE_PROJECTS").unwrap_or_else(|_| {
                dirs_home()
                    .join(".claude/projects")
                    .to_string_lossy()
                    .into_owned()
            }),
        );
        let correlate_sock_path = PathBuf::from(
            std::env::var("SHUTTLECRAFT_CORRELATE_SOCK")
                .unwrap_or_else(|_| "/run/shuttlecraft/correlate.sock".to_string()),
        );
        Ok(Self {
            listen,
            db_url,
            repos_root,
            claude_projects_dir,
            correlate_sock_path,
        })
    }
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/home/dev"))
}
