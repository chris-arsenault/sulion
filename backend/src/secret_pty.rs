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
        tracing::debug!(
            %pty_session_id,
            "no secret broker configured; PTY starts without a broker credential"
        );
        return Ok(None);
    };
    // Each step here can fail for an unrelated reason and the whole path runs
    // before a session exists, so a failure surfaces only as a session that
    // would not start. Traced individually so the failing step is named.
    tracing::debug!(
        %pty_session_id,
        broker_url = %client.broker_url,
        uid = unsafe { libc::geteuid() },
        "preparing a PTY secret broker credential",
    );
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

    // Reaching the broker and being refused by it are different problems with
    // different fixes, and the refusal reason is the whole diagnosis. Reported
    // separately, with the broker's own body, because this surfaces to an
    // operator as a session that would not start.
    let endpoint = format!(
        "{}/v1/pty-credentials",
        client.broker_url.trim_end_matches('/')
    );
    tracing::debug!(%pty_session_id, %endpoint, "registering the PTY credential with the broker");
    let response = client
        .http
        .post(&endpoint)
        .bearer_auth(&client.registration_token)
        .json(&RegisterPtyCredentialRequest {
            pty_session_id,
            public_key,
        })
        .send()
        .await
        .inspect_err(|error| {
            // The transport error names what actually went wrong — DNS, TLS,
            // connection refused — and it is otherwise flattened away by the
            // time this reaches a browser.
            tracing::error!(%pty_session_id, %endpoint, ?error, "secret broker is unreachable");
        })
        .with_context(|| format!("reach the secret broker at {endpoint}"))?;
    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|error| format!("<unreadable body: {error}>"));
        let body = body.chars().take(300).collect::<String>();
        tracing::error!(
            %pty_session_id,
            %endpoint,
            %status,
            %body,
            "secret broker refused a PTY credential",
        );
        anyhow::bail!("secret broker refused a PTY credential: {status} from {endpoint}: {body}");
    }

    tokio::fs::write(&key_path, pkcs8.as_ref())
        .await
        .with_context(|| format!("write {}", key_path.display()))?;
    tokio::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
        .await
        .with_context(|| format!("chmod {}", key_path.display()))?;
    tracing::debug!(%pty_session_id, path = %key_path.display(), "PTY broker credential ready");
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
        // Trusts the pinned control certificate in addition to public roots.
        http: crate::node_protocol::tls::control_http_client(),
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
