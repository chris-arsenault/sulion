use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;

use anyhow::Context;
use base64::prelude::{Engine as _, BASE64_STANDARD};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::Ed25519KeyPair;
use uuid::Uuid;

use crate::secret_protocol::{canonical_use_payload, SignedUseSecretRequest, UseSecretResponse};

pub async fn run(args: &[OsString]) -> anyhow::Result<i32> {
    let parsed = match HelperArgs::parse(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}");
            eprintln!("usage: sulion credential-helper --tool <with-cred|aws> [--secret <id>] -- <command...>");
            return Ok(64);
        }
    };

    let pty_session_id = match std::env::var("SULION_PTY_ID")
        .ok()
        .and_then(|value| value.parse::<Uuid>().ok())
    {
        Some(id) => id,
        None => {
            eprintln!("credential-helper: SULION_PTY_ID is not set or invalid");
            return Ok(65);
        }
    };
    let key_path = match std::env::var("SULION_SECRET_BROKER_KEY_PATH") {
        Ok(path) if !path.trim().is_empty() => PathBuf::from(path),
        _ => {
            eprintln!("credential-helper: SULION_SECRET_BROKER_KEY_PATH is not set");
            return Ok(65);
        }
    };
    let broker_url = std::env::var("SULION_SECRET_BROKER_URL")
        .unwrap_or_else(|_| "http://sulion-broker:8081".to_string());

    let pkcs8 = tokio::fs::read(&key_path)
        .await
        .with_context(|| format!("read PTY secret broker key {}", key_path.display()))?;
    let key_pair = Ed25519KeyPair::from_pkcs8(&pkcs8)
        .map_err(|_| anyhow::anyhow!("invalid PTY secret broker key"))?;
    let nonce = new_nonce();
    let timestamp_unix_seconds = chrono::Utc::now().timestamp();
    let canonical = canonical_use_payload(
        pty_session_id,
        parsed.secret_id.as_deref(),
        &parsed.tool,
        timestamp_unix_seconds,
        &nonce,
    );
    let signature = BASE64_STANDARD.encode(key_pair.sign(canonical.as_bytes()).as_ref());
    let request = SignedUseSecretRequest {
        pty_session_id,
        secret_id: parsed.secret_id.clone(),
        tool: parsed.tool.clone(),
        timestamp_unix_seconds,
        nonce,
        signature,
    };

    let response = reqwest::Client::new()
        .post(format!("{}/v1/use", broker_url.trim_end_matches('/')))
        .json(&request)
        .send()
        .await;
    let response = match response {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            eprintln!(
                "credential-helper: broker denied access for {}{} ({status}): {body}",
                parsed.tool,
                parsed
                    .secret_id
                    .as_ref()
                    .map(|id| format!(":{id}"))
                    .unwrap_or_default()
            );
            return Ok(66);
        }
        Err(err) => {
            eprintln!("credential-helper: broker request failed: {err}");
            return Ok(66);
        }
    };
    let payload = response
        .json::<UseSecretResponse>()
        .await
        .context("invalid broker response")?;

    let mut command = std::process::Command::new(&parsed.command[0]);
    command.args(&parsed.command[1..]);
    command.envs(payload.env);
    Err(command.exec()).context("exec credential command")
}

fn new_nonce() -> String {
    let rng = SystemRandom::new();
    let mut bytes = [0u8; 24];
    rng.fill(&mut bytes).expect("system random");
    BASE64_STANDARD.encode(bytes)
}

struct HelperArgs {
    tool: String,
    secret_id: Option<String>,
    command: Vec<OsString>,
}

impl HelperArgs {
    fn parse(args: &[OsString]) -> Result<Self, &'static str> {
        let mut tool: Option<String> = None;
        let mut secret_id: Option<String> = None;
        let mut index = 0;
        while index < args.len() {
            let arg = args[index].to_string_lossy();
            if arg == "--" {
                index += 1;
                break;
            }
            match arg.as_ref() {
                "--tool" => {
                    index += 1;
                    let Some(value) = args.get(index) else {
                        return Err("credential-helper: --tool requires a value");
                    };
                    tool = Some(value.to_string_lossy().into_owned());
                }
                "--secret" => {
                    index += 1;
                    let Some(value) = args.get(index) else {
                        return Err("credential-helper: --secret requires a value");
                    };
                    secret_id = Some(value.to_string_lossy().into_owned());
                }
                _ => return Err("credential-helper: unknown option"),
            }
            index += 1;
        }
        let Some(tool) = tool else {
            return Err("credential-helper: --tool is required");
        };
        if !matches!(tool.as_str(), "with-cred" | "aws") {
            return Err("credential-helper: --tool must be with-cred or aws");
        }
        if tool == "aws" && secret_id.is_none() {
            return Err("credential-helper: --tool aws requires --secret");
        }
        let command = args[index..].to_vec();
        if command.is_empty() {
            return Err("credential-helper: missing command");
        }
        Ok(Self {
            tool,
            secret_id,
            command,
        })
    }
}
