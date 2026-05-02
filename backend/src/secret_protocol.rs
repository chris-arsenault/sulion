use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize, Serialize)]
pub struct RegisterPtyCredentialRequest {
    pub pty_session_id: Uuid,
    pub public_key: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SignedUseSecretRequest {
    pub pty_session_id: Uuid,
    pub secret_id: Option<String>,
    pub tool: String,
    pub timestamp_unix_seconds: i64,
    pub nonce: String,
    pub signature: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UseSecretResponse {
    pub env: HashMap<String, String>,
}

pub fn canonical_use_payload(
    pty_session_id: Uuid,
    secret_id: Option<&str>,
    tool: &str,
    timestamp_unix_seconds: i64,
    nonce: &str,
) -> String {
    format!(
        "sulion-secret-use-v1\n{}\n{}\n{}\n{}\n{}\n",
        pty_session_id,
        tool,
        secret_id.unwrap_or(""),
        timestamp_unix_seconds,
        nonce,
    )
}
