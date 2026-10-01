use std::collections::HashMap;

use base64::prelude::{Engine as _, BASE64_STANDARD};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::Ed25519KeyPair;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize, Serialize)]
pub struct RegisterPtyCredentialRequest {
    pub pty_session_id: Uuid,
    pub public_key: String,
    #[serde(default)]
    pub repo: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SignedUseSecretRequest {
    pub pty_session_id: Uuid,
    /// File name of the program `with-cred` will exec. Gates every-terminal
    /// secrets; terminal and repository grants ignore it.
    pub program: String,
    pub timestamp_unix_seconds: i64,
    pub nonce: String,
    pub signature: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UseSecretResponse {
    pub env: HashMap<String, String>,
}

impl SignedUseSecretRequest {
    /// A fresh request for `program`, signed with the credential's key.
    pub fn sign(key_pair: &Ed25519KeyPair, pty_session_id: Uuid, program: String) -> Self {
        let mut nonce = [0u8; 24];
        SystemRandom::new().fill(&mut nonce).expect("system random");
        let nonce = BASE64_STANDARD.encode(nonce);
        let timestamp_unix_seconds = chrono::Utc::now().timestamp();
        let canonical =
            canonical_use_payload(pty_session_id, &program, timestamp_unix_seconds, &nonce);
        Self {
            pty_session_id,
            program,
            timestamp_unix_seconds,
            nonce,
            signature: BASE64_STANDARD.encode(key_pair.sign(canonical.as_bytes()).as_ref()),
        }
    }
}

pub fn canonical_use_payload(
    pty_session_id: Uuid,
    program: &str,
    timestamp_unix_seconds: i64,
    nonce: &str,
) -> String {
    format!(
        "sulion-secret-use-v2\n{pty_session_id}\n{program}\n{timestamp_unix_seconds}\n{nonce}\n"
    )
}
