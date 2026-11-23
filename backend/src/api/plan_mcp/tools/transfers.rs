use super::{arguments, tool};
use crate::{
    api::upload_routes::{begin_upload, finish_upload},
    uploads::{model::UploadInput, MAX_BYTES},
    AppState,
};
use anyhow::{anyhow, ensure};
use base64::{engine::general_purpose::STANDARD, Engine};
use futures::StreamExt;
use reqwest::{Client, Url};
use rmcp::model::{JsonObject, MetaObject, Tool};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;

#[derive(Deserialize, JsonSchema)]
struct Complete {
    id: Uuid,
    #[serde(flatten)]
    input: UploadInput,
}

#[derive(Deserialize, JsonSchema)]
struct ChatFile {
    download_url: String,
    file_id: String,
    #[serde(default)]
    #[schemars(with = "String")]
    file_name: Option<String>,
    #[serde(default)]
    #[schemars(with = "String")]
    #[allow(dead_code)]
    mime_type: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct ChatUpload {
    repo: Option<String>,
    workspace_id: Option<Uuid>,
    #[serde(default)]
    directory: String,
    /// Override the attachment name, or supply it when the client omits file_name.
    filename: Option<String>,
    file: ChatFile,
}

pub(super) fn tools() -> Vec<Tool> {
    let mut meta = MetaObject::new();
    meta.0.insert("openai/fileParams".into(), json!(["file"]));
    vec![
        tool::<ChatUpload>("upload_file", "Install a selected ChatGPT attachment in a repo or workspace. Select the file and destination filename. An existing file at that path is replaced.", false, true).with_meta(meta),
        tool::<UploadInput>("begin_file_upload", "Start a raw file transfer. Supply byte size and base64 SHA-256. PUT the bytes to the returned URL with its signed headers, then call complete_file_upload.", false, false),
        tool::<Complete>("complete_file_upload", "Install the verified upload. Repeat the original metadata with the granted id. An existing destination file is replaced.", false, true),
    ]
}

pub(super) async fn call(
    state: &AppState,
    name: &str,
    params: Option<JsonObject>,
    subject: &str,
) -> anyhow::Result<Value> {
    match name {
        "begin_file_upload" => {
            let input = arguments(params)?;
            let (id, grant) = begin_upload(state, subject, &input).await?;
            Ok(json!({"id":id, "grant":grant}))
        }
        "complete_file_upload" => {
            let Complete { id, input } = arguments(params)?;
            Ok(finish_upload(state, subject, id, input).await?)
        }
        "upload_file" => upload_chat_file(state, subject, arguments(params)?).await,
        _ => anyhow::bail!("unknown file operation"),
    }
}

fn attachment_url(raw: &str) -> anyhow::Result<Url> {
    let url = Url::parse(raw).map_err(|_| anyhow!("invalid attachment URL"))?;
    ensure!(
        url.scheme() == "https"
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && url
                .host_str()
                .is_some_and(|h| h == "oaiusercontent.com" || h.ends_with(".oaiusercontent.com")),
        "expected a ChatGPT attachment URL on oaiusercontent.com"
    );
    Ok(url)
}

async fn upload_chat_file(
    state: &AppState,
    subject: &str,
    args: ChatUpload,
) -> anyhow::Result<Value> {
    static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
    let _permit = SLOTS
        .try_acquire()
        .map_err(|_| anyhow!("File transfers are busy. Try again shortly."))?;
    let url = attachment_url(&args.file.download_url)?;
    ensure!(!args.file.file_id.is_empty(), "missing attachment id");
    let filename = args
        .filename
        .or(args.file.file_name)
        .ok_or_else(|| anyhow!("supply a filename for this attachment"))?;
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(100))
        .build()?;
    let bytes = download_attachment(&client, url).await?;
    let input = UploadInput {
        repo: args.repo,
        workspace_id: args.workspace_id,
        directory: args.directory,
        filename,
        size: bytes.len() as i64,
        checksum: STANDARD.encode(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref()),
    };
    let (id, grant) = begin_upload(state, subject, &input).await?;
    let mut put = client
        .put(grant.url)
        .header(reqwest::header::CONTENT_LENGTH, input.size);
    for (name, value) in grant.headers {
        put = put.header(name, value);
    }
    let response = put
        .body(bytes)
        .send()
        .await
        .map_err(|_| anyhow!("could not stage the attachment"))?;
    ensure!(
        response.status().is_success(),
        "storage rejected the attachment"
    );
    Ok(finish_upload(state, subject, id, input).await?)
}

async fn download_attachment(client: &Client, url: Url) -> anyhow::Result<Vec<u8>> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| anyhow!("could not download the attachment"))?;
    ensure!(
        response.status().is_success(),
        "attachment download expired or was rejected. Select the file again."
    );
    ensure!(
        response.content_length().unwrap_or_default() <= MAX_BYTES as u64,
        "file exceeds 50 MiB"
    );
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| anyhow!("attachment download interrupted"))?;
        ensure!(
            bytes.len() + chunk.len() <= MAX_BYTES as usize,
            "file exceeds 50 MiB"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Router};

    #[tokio::test]
    async fn attachment_transfer_reads_binary_bytes_outside_the_rpc_body() {
        let bytes: Vec<_> = (0..=255).cycle().take(320 * 1024).collect();
        let original = bytes.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route("/file", get(move || async move { original }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let url = format!("http://{address}/file").parse().unwrap();
        let result = download_attachment(&Client::new(), url).await.unwrap();
        server.abort();
        assert_eq!(result, bytes);
        assert!(attachment_url("https://files.oaiusercontent.com/file").is_ok());
        assert!(attachment_url("https://example.com/file").is_err());
    }
}
