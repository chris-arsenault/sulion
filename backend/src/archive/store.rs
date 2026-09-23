//! Where archive objects go.
//!
//! Two backends behind one enum: S3 through the `aws` CLI the image already
//! carries (the same mechanism the trust appliance uses for its secret-store
//! backup, authenticated by the profile the Roles Anywhere bootstrap writes),
//! and a plain directory for integration tests. An enum rather than a trait
//! object because the call sites are three async methods and nothing else
//! will ever implement them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use tokio::process::Command;

/// The AWS CLI to run. `/opt/sulion/bin/aws` sits first on the image's
/// PATH and is the PTY wrapper that routes through the secret broker with a
/// PTY grant; the control process has no PTY and authenticates with its own
/// machine identity, so it must call the real CLI directly.
fn aws_cli() -> String {
    crate::config::env_optional("SULION_AWS_CLI").unwrap_or_else(|| "/usr/bin/aws".to_string())
}

#[derive(Debug, Clone)]
pub enum ObjectStore {
    S3 { bucket: String },
    Dir { root: PathBuf },
}

/// What a `HEAD` reports: enough to verify an upload landed intact.
#[derive(Debug, Clone, Default)]
pub struct ObjectHead {
    pub content_length: i64,
    pub metadata: BTreeMap<String, String>,
}

impl ObjectStore {
    /// `SULION_ARCHIVE_BUCKET` selects S3; `SULION_ARCHIVE_DIR` selects a
    /// directory (tests, or a local stand-in); neither disables archiving.
    pub fn from_env() -> Option<Self> {
        if let Some(dir) = crate::config::env_optional("SULION_ARCHIVE_DIR") {
            return Some(Self::Dir {
                root: PathBuf::from(dir),
            });
        }
        crate::config::env_optional("SULION_ARCHIVE_BUCKET").map(|bucket| Self::S3 { bucket })
    }

    pub fn describe(&self) -> String {
        match self {
            Self::S3 { bucket } => format!("s3://{bucket}"),
            Self::Dir { root } => root.display().to_string(),
        }
    }

    /// Uploads a local file under `key` with string metadata. Metadata values
    /// must be plain tokens (hashes, counts, ids): the CLI form is
    /// comma-separated and unquoted.
    pub async fn put_file(
        &self,
        key: &str,
        path: &Path,
        metadata: &BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        for (name, value) in metadata {
            if value.contains(',') || value.contains('=') || name.contains(',') {
                return Err(anyhow!(
                    "archive metadata {name} must not contain ',' or '='"
                ));
            }
        }
        match self {
            Self::S3 { bucket } => {
                let mut cmd = Command::new(aws_cli());
                cmd.arg("s3")
                    .arg("cp")
                    .arg(path)
                    .arg(format!("s3://{bucket}/{key}"))
                    .arg("--only-show-errors");
                if !metadata.is_empty() {
                    let joined = metadata
                        .iter()
                        .map(|(name, value)| format!("{name}={value}"))
                        .collect::<Vec<_>>()
                        .join(",");
                    cmd.arg("--metadata").arg(joined);
                }
                run_cli(cmd, "aws s3 cp (upload)").await?;
                Ok(())
            }
            Self::Dir { root } => {
                let target = root.join(key);
                if let Some(parent) = target.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::copy(path, &target)
                    .await
                    .with_context(|| format!("copy {} into archive dir", path.display()))?;
                let meta = serde_json::to_vec(metadata)?;
                tokio::fs::write(meta_path(&target), meta).await?;
                Ok(())
            }
        }
    }

    /// `HEAD` on the object; `None` when it does not exist.
    pub async fn head(&self, key: &str) -> anyhow::Result<Option<ObjectHead>> {
        match self {
            Self::S3 { bucket } => {
                let mut cmd = Command::new(aws_cli());
                cmd.args(["s3api", "head-object", "--bucket"])
                    .arg(bucket)
                    .arg("--key")
                    .arg(key)
                    .args(["--output", "json"]);
                let output = cmd.output().await.context("spawn aws s3api head-object")?;
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if stderr.contains("404") || stderr.contains("Not Found") {
                        return Ok(None);
                    }
                    return Err(anyhow!(
                        "aws s3api head-object failed ({}): {}",
                        output.status,
                        stderr.trim()
                    ));
                }
                let parsed: serde_json::Value =
                    serde_json::from_slice(&output.stdout).context("parse head-object output")?;
                let content_length = parsed
                    .get("ContentLength")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or_default();
                let metadata = parsed
                    .get("Metadata")
                    .and_then(serde_json::Value::as_object)
                    .map(|object| {
                        object
                            .iter()
                            .filter_map(|(name, value)| {
                                value
                                    .as_str()
                                    .map(|value| (name.clone(), value.to_string()))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(Some(ObjectHead {
                    content_length,
                    metadata,
                }))
            }
            Self::Dir { root } => {
                let target = root.join(key);
                let Ok(stat) = tokio::fs::metadata(&target).await else {
                    return Ok(None);
                };
                let metadata = match tokio::fs::read(meta_path(&target)).await {
                    Ok(raw) => serde_json::from_slice(&raw).unwrap_or_default(),
                    Err(_) => BTreeMap::new(),
                };
                Ok(Some(ObjectHead {
                    content_length: stat.len() as i64,
                    metadata,
                }))
            }
        }
    }

    /// Downloads `key` to a local file.
    pub async fn get_to_file(&self, key: &str, path: &Path) -> anyhow::Result<()> {
        match self {
            Self::S3 { bucket } => {
                let mut cmd = Command::new(aws_cli());
                cmd.arg("s3")
                    .arg("cp")
                    .arg(format!("s3://{bucket}/{key}"))
                    .arg(path)
                    .arg("--only-show-errors");
                run_cli(cmd, "aws s3 cp (download)").await?;
                Ok(())
            }
            Self::Dir { root } => {
                tokio::fs::copy(root.join(key), path)
                    .await
                    .with_context(|| format!("read {key} from archive dir"))?;
                Ok(())
            }
        }
    }
}

fn meta_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".meta.json");
    target.with_file_name(name)
}

async fn run_cli(mut cmd: Command, what: &str) -> anyhow::Result<()> {
    let output = cmd
        .output()
        .await
        .with_context(|| format!("spawn {what}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(anyhow!(
        "{what} failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}
