use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use anyhow::Context;
use base64::prelude::{Engine as _, BASE64_STANDARD};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use uuid::Uuid;

use crate::secret_protocol::RegisterPtyCredentialRequest;

pub async fn prepare_pty_credential(pty_session_id: Uuid) -> anyhow::Result<Option<PathBuf>> {
    let Some(client) = broker_registration_client() else {
        return Ok(None);
    };
    let key_dir = pty_key_dir();
    tokio::fs::create_dir_all(&key_dir)
        .await
        .with_context(|| format!("create {}", key_dir.display()))?;
    tokio::fs::set_permissions(&key_dir, std::fs::Permissions::from_mode(0o700))
        .await
        .with_context(|| format!("chmod {}", key_dir.display()))?;
    let key_path = key_path_for(&key_dir, pty_session_id);

    let rng = SystemRandom::new();
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng)
        .map_err(|_| anyhow::anyhow!("generate PTY secret broker key"))?;
    let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
        .map_err(|_| anyhow::anyhow!("load generated PTY secret broker key"))?;
    let public_key = BASE64_STANDARD.encode(key_pair.public_key().as_ref());

    client
        .http
        .post(format!(
            "{}/v1/pty-credentials",
            client.broker_url.trim_end_matches('/')
        ))
        .bearer_auth(&client.registration_token)
        .json(&RegisterPtyCredentialRequest {
            pty_session_id,
            public_key,
        })
        .send()
        .await
        .context("register PTY secret broker credential")?
        .error_for_status()
        .context("register PTY secret broker credential")?;

    tokio::fs::write(&key_path, pkcs8.as_ref())
        .await
        .with_context(|| format!("write {}", key_path.display()))?;
    tokio::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
        .await
        .with_context(|| format!("chmod {}", key_path.display()))?;
    Ok(Some(key_path))
}

pub async fn revoke_pty_credential(pty_session_id: Uuid) {
    if let Some(client) = broker_registration_client() {
        if let Err(err) = client
            .http
            .delete(format!(
                "{}/v1/pty-credentials/{}",
                client.broker_url.trim_end_matches('/'),
                pty_session_id
            ))
            .bearer_auth(&client.registration_token)
            .send()
            .await
            .and_then(|response| response.error_for_status())
        {
            tracing::warn!(%pty_session_id, %err, "revoke PTY secret broker credential failed");
        }
    }
    let key_path = key_path_for(&pty_key_dir(), pty_session_id);
    if let Err(err) = tokio::fs::remove_file(&key_path).await {
        if err.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(%pty_session_id, %err, path = %key_path.display(), "remove PTY secret broker key failed");
        }
    }
}

struct BrokerRegistrationClient {
    broker_url: String,
    registration_token: String,
    http: reqwest::Client,
}

fn broker_registration_client() -> Option<BrokerRegistrationClient> {
    let broker_url = std::env::var("SULION_SECRET_BROKER_URL").ok()?;
    let registration_token = std::env::var("SULION_SECRET_BROKER_REGISTRATION_TOKEN").ok()?;
    if broker_url.trim().is_empty() || registration_token.trim().is_empty() {
        return None;
    }
    Some(BrokerRegistrationClient {
        broker_url,
        registration_token,
        http: reqwest::Client::new(),
    })
}

fn pty_key_dir() -> PathBuf {
    PathBuf::from(
        std::env::var("SULION_SECRET_BROKER_KEY_DIR")
            .unwrap_or_else(|_| "/run/sulion/pty-keys".to_string()),
    )
}

fn key_path_for(key_dir: &std::path::Path, pty_session_id: Uuid) -> PathBuf {
    key_dir.join(format!("{pty_session_id}.pkcs8"))
}
