use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};

use anyhow::Context;
use ring::signature::Ed25519KeyPair;
use uuid::Uuid;

use crate::secret_protocol::{SignedUseSecretRequest, UseSecretResponse};

pub async fn run(args: &[OsString]) -> anyhow::Result<i32> {
    let command = match parse_command(args) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("credential-helper: {message}");
            eprintln!("usage: with-cred -- <command...>");
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
    let program = program_name(&command[0]);

    let pkcs8 = tokio::fs::read(&key_path)
        .await
        .with_context(|| format!("read PTY secret broker key {}", key_path.display()))?;
    let key_pair = Ed25519KeyPair::from_pkcs8(&pkcs8)
        .map_err(|_| anyhow::anyhow!("invalid PTY secret broker key"))?;
    let request = SignedUseSecretRequest::sign(&key_pair, pty_session_id, program);

    // Trusts the pinned control certificate in addition to public roots.
    let response = crate::node_protocol::tls::control_http_client()
        .post(format!("{}/v1/use", broker_url.trim_end_matches('/')))
        .json(&request)
        .send()
        .await;
    let response = match response {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            eprintln!("credential-helper: broker denied access ({status}): {body}");
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

    let mut process = std::process::Command::new(&command[0]);
    process.args(&command[1..]);
    process.envs(payload.env);
    Err(process.exec()).context("exec credential command")
}

/// The command's file name, which every-terminal grants are matched against.
fn program_name(command: &OsString) -> String {
    Path::new(command)
        .file_name()
        .unwrap_or(command)
        .to_string_lossy()
        .into_owned()
}

/// The command after the leading `--`.
fn parse_command(args: &[OsString]) -> Result<Vec<OsString>, &'static str> {
    match args.split_first() {
        Some((separator, command)) if separator == "--" && !command.is_empty() => {
            Ok(command.to_vec())
        }
        _ => Err("expected `-- <command...>`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn runs_the_command_after_the_separator() {
        assert_eq!(
            parse_command(&args(&["--", "make", "test"])).unwrap(),
            args(&["make", "test"])
        );
        for invalid in [
            &["--"][..],
            &["make"],
            &["--secret", "x", "--", "make"],
            &[],
        ] {
            assert!(parse_command(&args(invalid)).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn program_name_is_the_command_file_name() {
        assert_eq!(program_name(&"gh".into()), "gh");
        assert_eq!(program_name(&"/usr/bin/gh".into()), "gh");
    }
}
