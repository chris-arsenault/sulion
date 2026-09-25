use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UploadInput {
    pub repo: Option<String>,
    pub workspace_id: Option<Uuid>,
    pub directory: String,
    pub filename: String,
    pub size: i64,
    pub checksum: String,
}

impl UploadInput {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.repo.is_some() != self.workspace_id.is_some(),
            "one destination is required"
        );
        if let Some(repo) = &self.repo {
            anyhow::ensure!(
                crate::workspace::is_valid_repo_name(repo) && repo.len() <= 255,
                "invalid repository"
            );
        }
        anyhow::ensure!(
            (0..=super::MAX_BYTES).contains(&self.size),
            "file exceeds 50 MiB"
        );
        anyhow::ensure!(
            self.filename.len() <= 255 && valid_component(&self.filename),
            "invalid filename"
        );
        anyhow::ensure!(
            self.directory.len() <= 1024
                && (self.directory.is_empty() || self.directory.split('/').all(valid_component)),
            "invalid directory"
        );
        let digest = STANDARD.decode(&self.checksum)?;
        anyhow::ensure!(
            digest.len() == 32 && STANDARD.encode(digest) == self.checksum,
            "invalid SHA-256 checksum"
        );
        Ok(())
    }

    /// Stored as signed S3 metadata, then checked against the completing user.
    pub fn binding(&self, subject: &str) -> String {
        let bytes = serde_json::to_vec(&(subject, self)).expect("upload metadata serializes");
        STANDARD.encode(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref())
    }

    pub fn relative_path(&self) -> String {
        if self.directory.is_empty() {
            self.filename.clone()
        } else {
            format!("{}/{}", self.directory, self.filename)
        }
    }
}

pub fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && !value
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
}

// Contains a bearer URL; do not derive Debug or persist the payload.
#[derive(Deserialize, Serialize)]
pub struct ImportUpload {
    pub id: Uuid,
    pub input: UploadInput,
    pub url: String,
    pub bucket: String,
    pub region: String,
}
