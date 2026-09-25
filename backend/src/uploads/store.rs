use std::{collections::BTreeMap, time::Duration};

use anyhow::{bail, Context};
use aws_sdk_s3::{presigning::PresigningConfig, types::ChecksumMode};
use serde::Serialize;
use uuid::Uuid;

#[derive(Clone)]
pub struct StagingStore {
    client: aws_sdk_s3::Client,
    bucket: String,
    region: String,
}

// Grant URLs are credentials: deliberately no Debug implementation.
#[derive(Serialize)]
pub struct UploadGrant {
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub expires_in: u64,
}

impl StagingStore {
    #[cfg(feature = "integration-tests")]
    pub fn for_test() -> Self {
        use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new("test", "test", None, None, "test"))
            .build();
        Self {
            client: aws_sdk_s3::Client::from_conf(config),
            bucket: "sulion-upload-test".into(),
            region: "us-east-1".into(),
        }
    }

    pub async fn from_env() -> Option<Self> {
        let bucket = crate::config::env_optional("SULION_UPLOAD_BUCKET")?;
        let config = aws_config::load_from_env().await;
        let region = config.region()?.as_ref().to_owned();
        Some(Self {
            client: aws_sdk_s3::Client::new(&config),
            bucket,
            region,
        })
    }

    pub fn key(id: Uuid) -> String {
        format!("uploads/{id}")
    }

    pub async fn put_grant(
        &self,
        id: Uuid,
        size: i64,
        checksum: &str,
        binding: &str,
    ) -> anyhow::Result<UploadGrant> {
        if !(0..=super::MAX_BYTES).contains(&size) {
            bail!("invalid upload size");
        }
        let request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(Self::key(id))
            .content_length(size)
            .content_type("application/octet-stream")
            .checksum_sha256(checksum)
            .if_none_match("*")
            .metadata("upload-binding", binding)
            .presigned(PresigningConfig::expires_in(Duration::from_secs(900))?)
            .await
            .context("sign upload")?;
        // The browser supplies Content-Length from the raw Blob. Never ask JS to
        // set this forbidden header, but require it to be part of the signature.
        let url = url::Url::parse(request.uri())?;
        let signed = url
            .query_pairs()
            .find(|(k, _)| k == "X-Amz-SignedHeaders")
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default();
        for required in [
            "content-length",
            "content-type",
            "if-none-match",
            "x-amz-meta-upload-binding",
        ] {
            if !signed.split(';').any(|h| h == required) {
                bail!("upload constraint is not signed");
            }
        }
        let headers = request
            .headers()
            .filter(|(k, _)| !matches!(*k, "content-length" | "host"))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        Ok(UploadGrant {
            url: request.uri().to_owned(),
            headers,
            expires_in: 900,
        })
    }

    pub async fn verify(
        &self,
        id: Uuid,
        size: i64,
        checksum: &str,
        binding: &str,
    ) -> anyhow::Result<bool> {
        let result = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(Self::key(id))
            .checksum_mode(ChecksumMode::Enabled)
            .send()
            .await;
        match result {
            Ok(head) => Ok(matches_object(&head, size, checksum, binding)),
            Err(e) if e.as_service_error().is_some_and(|e| e.is_not_found()) => Ok(false),
            Err(_) => bail!("staging storage is unavailable"),
        }
    }

    pub async fn get_grant(&self, id: Uuid) -> anyhow::Result<String> {
        let request = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(Self::key(id))
            .presigned(PresigningConfig::expires_in(Duration::from_secs(900))?)
            .await
            .context("sign download")?;
        validate_download_url(request.uri(), &self.bucket, &self.region, id)?;
        Ok(request.uri().to_owned())
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }
    pub fn region(&self) -> &str {
        &self.region
    }
}

pub fn validate_download_url(
    raw: &str,
    bucket: &str,
    region: &str,
    id: Uuid,
) -> anyhow::Result<()> {
    let url = url::Url::parse(raw).map_err(|_| anyhow::anyhow!("invalid storage URL"))?;
    if url.scheme() != "https"
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.host_str() != Some(format!("{bucket}.s3.{region}.amazonaws.com").as_str())
        || url.path() != format!("/{}", StagingStore::key(id))
    {
        bail!("invalid storage URL");
    }
    Ok(())
}

fn matches_object(
    head: &aws_sdk_s3::operation::head_object::HeadObjectOutput,
    size: i64,
    checksum: &str,
    binding: &str,
) -> bool {
    head.content_length() == Some(size)
        && head.checksum_sha256() == Some(checksum)
        && head
            .metadata()
            .and_then(|m| m.get("upload-binding"))
            .map(String::as_str)
            == Some(binding)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};

    #[test]
    fn object_verification_binds_owner_destination_and_bytes() {
        use super::super::model::UploadInput;
        let mut input = UploadInput {
            repo: Some("repo".into()),
            workspace_id: None,
            directory: "".into(),
            filename: "file".into(),
            size: 3,
            checksum: "digest".into(),
        };
        let head = aws_sdk_s3::operation::head_object::HeadObjectOutput::builder()
            .content_length(input.size)
            .checksum_sha256(&input.checksum)
            .metadata("upload-binding", input.binding("alice"))
            .build();
        assert!(matches_object(&head, 3, "digest", &input.binding("alice")));
        assert!(!matches_object(&head, 3, "digest", &input.binding("bob")));
        assert!(!matches_object(&head, 4, "digest", &input.binding("alice")));
        assert!(!matches_object(
            &head,
            3,
            "different",
            &input.binding("alice")
        ));
        input.filename = "another-file".into();
        assert!(!matches_object(&head, 3, "digest", &input.binding("alice")));
    }

    #[tokio::test]
    async fn grants_bind_bytes_length_and_conditional_creation() {
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new("test", "test", None, None, "test"))
            .build();
        let store = StagingStore {
            client: aws_sdk_s3::Client::from_conf(config),
            bucket: "sulion-upload-test".into(),
            region: "us-east-1".into(),
        };
        let id = Uuid::new_v4();
        let checksum = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=";
        let grant = store
            .put_grant(id, 0, checksum, "owner-and-destination")
            .await
            .unwrap();
        let url = url::Url::parse(&grant.url).unwrap();
        let query: BTreeMap<_, _> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert!(query["X-Amz-SignedHeaders"].contains("content-length"));
        assert_eq!(grant.headers["if-none-match"], "*");
        assert_eq!(
            grant.headers["x-amz-meta-upload-binding"],
            "owner-and-destination"
        );
        assert!(!grant.headers.contains_key("content-length"));
        assert!(
            query.get("x-amz-checksum-sha256").map(String::as_str) == Some(checksum)
                || (grant
                    .headers
                    .get("x-amz-checksum-sha256")
                    .map(String::as_str)
                    == Some(checksum)
                    && query["X-Amz-SignedHeaders"].contains("x-amz-checksum-sha256"))
        );
        store.get_grant(id).await.unwrap();
        assert!(store
            .put_grant(id, super::super::MAX_BYTES + 1, checksum, "binding")
            .await
            .is_err());
        assert!(
            validate_download_url("http://127.0.0.1/", store.bucket(), store.region(), id).is_err()
        );
    }
}
