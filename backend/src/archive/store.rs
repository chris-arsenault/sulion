//! Where archive objects go.
//!
//! Two backends behind one enum: S3 through the AWS SDK with one reused
//! client (authenticated by the profile the Roles Anywhere bootstrap writes,
//! resolved by the SDK's default chain), and a plain directory for
//! integration tests. An enum rather than a trait object because the call
//! sites are three async methods and nothing else will ever implement them.
//!
//! Earlier this shelled out to the `aws` CLI per call. At thousands of
//! objects the second-long CLI start dominated: an export or a deep verify
//! spent hours spawning processes and minutes moving bytes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::primitives::ByteStream;

#[derive(Clone)]
pub enum ObjectStore {
    S3 {
        bucket: String,
        client: aws_sdk_s3::Client,
    },
    Dir {
        root: PathBuf,
    },
}

impl std::fmt::Debug for ObjectStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.describe())
    }
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
    pub async fn from_env() -> Option<Self> {
        if let Some(dir) = crate::config::env_optional("SULION_ARCHIVE_DIR") {
            return Some(Self::Dir {
                root: PathBuf::from(dir),
            });
        }
        let bucket = crate::config::env_optional("SULION_ARCHIVE_BUCKET")?;
        let config = aws_config::load_from_env().await;
        Some(Self::S3 {
            bucket,
            client: aws_sdk_s3::Client::new(&config),
        })
    }

    pub fn describe(&self) -> String {
        match self {
            Self::S3 { bucket, .. } => format!("s3://{bucket}"),
            Self::Dir { root } => root.display().to_string(),
        }
    }

    /// Uploads a local file under `key` with string metadata.
    pub async fn put_file(
        &self,
        key: &str,
        path: &Path,
        metadata: &BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        match self {
            Self::S3 { bucket, client } => {
                let body = ByteStream::from_path(path)
                    .await
                    .with_context(|| format!("open {} for upload", path.display()))?;
                client
                    .put_object()
                    .bucket(bucket)
                    .key(key)
                    .body(body)
                    .set_metadata(Some(
                        metadata
                            .iter()
                            .map(|(name, value)| (name.clone(), value.clone()))
                            .collect(),
                    ))
                    .send()
                    .await
                    .map_err(|err| anyhow::anyhow!("put {key}: {}", describe_sdk_error(&err)))?;
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
            Self::S3 { bucket, client } => {
                let response = client.head_object().bucket(bucket).key(key).send().await;
                match response {
                    Ok(head) => Ok(Some(ObjectHead {
                        content_length: head.content_length().unwrap_or_default(),
                        metadata: head
                            .metadata()
                            .map(|map| {
                                map.iter()
                                    .map(|(name, value)| (name.clone(), value.clone()))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })),
                    Err(err) => {
                        if err
                            .as_service_error()
                            .is_some_and(|service| service.is_not_found())
                        {
                            return Ok(None);
                        }
                        Err(anyhow::anyhow!("head {key}: {}", describe_sdk_error(&err)))
                    }
                }
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

    /// Downloads `key` to a local file, streaming.
    pub async fn get_to_file(&self, key: &str, path: &Path) -> anyhow::Result<()> {
        match self {
            Self::S3 { bucket, client } => {
                let object = client
                    .get_object()
                    .bucket(bucket)
                    .key(key)
                    .send()
                    .await
                    .map_err(|err| anyhow::anyhow!("get {key}: {}", describe_sdk_error(&err)))?;
                let mut body = object.body.into_async_read();
                let mut file = tokio::fs::File::create(path)
                    .await
                    .with_context(|| format!("create {}", path.display()))?;
                tokio::io::copy(&mut body, &mut file)
                    .await
                    .with_context(|| format!("stream {key} to disk"))?;
                file.sync_all().await?;
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

/// The SDK's error `Display` is the outer wrapper only; the service message
/// is what says "AccessDenied" or "NoSuchBucket".
fn describe_sdk_error<E: std::fmt::Debug + std::error::Error>(err: &SdkError<E>) -> String {
    match err {
        SdkError::ServiceError(service) => format!("{:?}", service.err()),
        other => format!("{other}"),
    }
}
