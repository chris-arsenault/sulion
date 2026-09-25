use base64::{engine::general_purpose::STANDARD, Engine};
use futures::StreamExt;
use ring::digest::{Context, SHA256};
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};
use tokio::io::AsyncWriteExt;

use super::{NodeRuntime, RuntimeError};
use crate::uploads::{
    install::Directory,
    model::{ImportUpload, UploadInput},
    store::validate_download_url,
};

impl NodeRuntime {
    pub(super) async fn install_upload(
        &self,
        request: ImportUpload,
    ) -> Result<Value, RuntimeError> {
        validate_download_url(&request.url, &request.bucket, &request.region, request.id)
            .map_err(|_| RuntimeError::BadRequest("Invalid download grant.".into()))?;
        self.install_staged_file(&request.input, &request.url).await
    }

    #[cfg(feature = "integration-tests")]
    pub async fn install_upload_for_test(
        &self,
        input: &UploadInput,
        fixture_url: &str,
    ) -> anyhow::Result<Value> {
        Ok(self.install_staged_file(input, fixture_url).await?)
    }

    async fn upload_root(&self, input: &UploadInput) -> Result<PathBuf, RuntimeError> {
        match (&input.repo, input.workspace_id) {
            (Some(repo), None) => self.repo_root(repo),
            (None, Some(id)) => {
                let workspace = self.load_workspace(id).await?;
                let owner: Option<uuid::Uuid> =
                    sqlx::query_scalar("SELECT node_id FROM workspaces WHERE id=$1")
                        .bind(id)
                        .fetch_one(&self.pool)
                        .await?;
                if owner != Some(self.node_id) || workspace.state == "deleted" {
                    return Err(RuntimeError::BadRequest("Workspace is unavailable.".into()));
                }
                Ok(workspace.path)
            }
            _ => Err(RuntimeError::BadRequest(
                "One destination is required.".into(),
            )),
        }
    }

    async fn install_staged_file(
        &self,
        input: &UploadInput,
        url: &str,
    ) -> Result<Value, RuntimeError> {
        input
            .validate()
            .map_err(|e| RuntimeError::BadRequest(e.to_string()))?;
        let _permit = self
            .upload_slots
            .try_acquire()
            .map_err(|_| RuntimeError::BadRequest("Uploads are busy. Try again shortly.".into()))?;
        let result = tokio::time::timeout(
            Duration::from_secs(220),
            self.download_and_install(input, url),
        )
        .await
        .map_err(|_| RuntimeError::BadRequest("File download timed out. Try again.".into()))??;
        if let Some(id) = input.workspace_id {
            let _ = self.workspace_state.request_refresh(id).await;
        } else if let Some(repo) = &input.repo {
            let _ = self.repo_state.request_refresh(repo).await;
        }
        Ok(result)
    }

    async fn download_and_install(
        &self,
        input: &UploadInput,
        url: &str,
    ) -> Result<Value, RuntimeError> {
        let _guard = self.repo_lifecycle_gate.read().await;
        let root = self.upload_root(input).await?;
        let directory = Directory::root(&root)?.descend(&input.directory, true)?;
        let mut temporary = directory.temporary()?;
        download(url, temporary.take_file(), input.size, &input.checksum)
            .await
            .map_err(|message| RuntimeError::BadRequest(message.into()))?;
        let current_root = self.upload_root(input).await?;
        let current = Directory::root(&current_root)?.descend(&input.directory, false)?;
        if current.identity()? != temporary.directory.identity()? {
            return Err(RuntimeError::BadRequest(
                "The upload destination changed. Try again.".into(),
            ));
        }
        // No await between replacement and returning the result: cancellation
        // during download drops the temporary without exposing partial content.
        temporary.install(&input.filename)?;
        Ok(json!({"path": current_root.join(input.relative_path()), "size": input.size}))
    }
}

async fn download(
    url: &str,
    file: std::fs::File,
    size: i64,
    checksum: &str,
) -> Result<(), &'static str> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(210))
        .build()
        .map_err(|_| "Could not start file download.")?;
    // Reqwest errors can contain the bearer URL. Never forward or log them.
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "File download failed. Try again.")?;
    if !response.status().is_success() {
        return Err("File storage rejected the download. Try again.");
    }
    if response
        .content_length()
        .is_some_and(|length| length != size as u64)
    {
        return Err("The downloaded file size does not match.");
    }
    let mut file = tokio::fs::File::from_std(file);
    let mut stream = response.bytes_stream();
    let mut digest = Context::new(&SHA256);
    let mut total = 0i64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "File download was interrupted. Try again.")?;
        total += chunk.len() as i64;
        if total > size {
            return Err("The downloaded file exceeds its expected size.");
        }
        digest.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|_| "Could not write the uploaded file.")?;
    }
    if total != size || STANDARD.encode(digest.finish().as_ref()) != checksum {
        return Err("The downloaded file checksum or size does not match.");
    }
    file.sync_all()
        .await
        .map_err(|_| "Could not save the uploaded file.")?;
    Ok(())
}
