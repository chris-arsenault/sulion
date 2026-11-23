use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use super::{NodeRuntime, RuntimeError};

#[derive(Deserialize)]
struct Checkout {
    repo: Option<String>,
    workspace_id: Option<Uuid>,
}

impl NodeRuntime {
    pub(super) async fn data_set(&self, request: Value) -> Result<Value, RuntimeError> {
        let target: Checkout = serde_json::from_value(request["arguments"].clone())
            .map_err(|e| RuntimeError::BadRequest(e.to_string()))?;
        let _guard = self.repo_lifecycle_gate.read().await;
        let root = self
            .document_root(target.repo.as_deref(), target.workspace_id)
            .await?;
        let mut child = tokio::process::Command::new("python3")
            .args(["-I", "-c", include_str!("data_set.py")])
            .arg(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        stdin
            .write_all(&serde_json::to_vec(&request).map_err(anyhow::Error::from)?)
            .await?;
        drop(stdin);
        let output = tokio::time::timeout(Duration::from_secs(55), child.wait_with_output())
            .await
            .map_err(|_| RuntimeError::BadRequest("Dataset operation timed out".into()))??;
        if !output.status.success() {
            return Err(RuntimeError::BadRequest(
                String::from_utf8_lossy(&output.stderr).trim().into(),
            ));
        }
        if matches!(
            request["tool"].as_str(),
            Some("define_data_set" | "fill_data_set_items" | "import_data_set")
        ) || request["arguments"]["destination_path"].is_string()
        {
            if let Some(id) = target.workspace_id {
                let _ = self.workspace_state.request_refresh(id).await;
            } else if let Some(repo) = target.repo {
                let _ = self.repo_state.request_refresh(&repo).await;
            }
        }
        serde_json::from_slice(&output.stdout).map_err(|e| anyhow::Error::from(e).into())
    }
}
