//! PTY secret redemption and all-terminals grants.
//!
//! An all-terminals grant applies only when the signed program name is one of
//! its `programs`; any other command still needs a terminal or repository
//! grant and is refused without one. Terminal and repository grants rank above
//! all-terminals grants, so an explicit grant replaces an all-terminals value
//! for the same environment variable. Two grants of the same rank that set the
//! same variable still fail: merges within a rank must be explicit.

use super::*;

pub(super) async fn use_secret(
    State(state): State<Arc<BrokerState>>,
    Json(body): Json<SignedUseSecretRequest>,
) -> Result<Json<UseSecretResponse>, BrokerError> {
    verify_signed_use_request(&state, &body).await?;
    let rows = sqlx::query(
        "SELECT DISTINCT ON (s.id) s.id, s.ciphertext, s.nonce, g.precedence \
         FROM secret_broker.effective_grants g \
         JOIN secret_broker.secrets s ON s.id = g.secret_id \
         WHERE g.pty_session_id = $1 \
           AND (g.programs IS NULL OR $2 = ANY(g.programs)) \
         ORDER BY s.id, g.precedence DESC, g.expires_at DESC",
    )
    .bind(body.pty_session_id)
    .bind(&body.program)
    .fetch_all(&state.pool)
    .await?;
    let mut redeemed = Vec::with_capacity(rows.len());
    for row in rows {
        let ciphertext: Vec<u8> = row.get("ciphertext");
        let nonce: Vec<u8> = row.get("nonce");
        redeemed.push(RedeemedSecret {
            secret_id: row.get("id"),
            precedence: row.get("precedence"),
            env: state.crypto.decrypt_env(&ciphertext, &nonce)?,
        });
    }
    let (env, granted_secret_ids) = merge_by_precedence(redeemed).map_err(|key| {
        BrokerError::bad_request(format!("conflicting env var {key} across unlocked secrets"))
    })?;
    if granted_secret_ids.is_empty() {
        return Err(BrokerError::forbidden(
            "no secret is unlocked for this terminal",
        ));
    }
    for secret_id in granted_secret_ids {
        sqlx::query(
            "INSERT INTO secret_broker.use_audit (id, pty_session_id, secret_id, used_at) \
             VALUES ($1, $2, $3, NOW())",
        )
        .bind(Uuid::new_v4())
        .bind(body.pty_session_id)
        .bind(secret_id)
        .execute(&state.pool)
        .await?;
    }
    Ok(Json(UseSecretResponse { env }))
}

pub(super) struct RedeemedSecret {
    pub secret_id: String,
    pub precedence: i32,
    pub env: HashMap<String, String>,
}

/// Merge redeemed secrets. A higher-precedence value replaces a lower one; an
/// equal-precedence collision returns the variable name. Only secrets that
/// contribute at least one variable are returned for auditing.
pub(super) fn merge_by_precedence(
    mut redeemed: Vec<RedeemedSecret>,
) -> Result<(HashMap<String, String>, Vec<String>), String> {
    redeemed.sort_by_key(|secret| std::cmp::Reverse(secret.precedence));
    let mut env = HashMap::new();
    let mut ranks: HashMap<String, i32> = HashMap::new();
    let mut contributors = Vec::new();
    for secret in redeemed {
        let mut contributed = false;
        for (key, value) in secret.env {
            match ranks.get(&key) {
                Some(rank) if *rank > secret.precedence => continue,
                Some(_) => return Err(key),
                None => {
                    ranks.insert(key.clone(), secret.precedence);
                    env.insert(key, value);
                    contributed = true;
                }
            }
        }
        if contributed {
            contributors.push(secret.secret_id);
        }
    }
    Ok((env, contributors))
}

/// Deduplicated program names for an all-terminals grant, matched against the
/// file name of the command `with-cred` runs.
pub(super) fn validate_programs(programs: Vec<String>) -> Result<Vec<String>, BrokerError> {
    let mut valid: Vec<String> = Vec::with_capacity(programs.len());
    for program in programs {
        let program = program.trim().to_string();
        if program.is_empty() || program.contains('/') || program.contains(char::is_whitespace) {
            return Err(BrokerError::bad_request(format!(
                "program must be a command name such as gh: {program:?}"
            )));
        }
        if !valid.contains(&program) {
            valid.push(program);
        }
    }
    if valid.is_empty() {
        return Err(BrokerError::bad_request(
            "an all-terminals grant needs at least one program",
        ));
    }
    Ok(valid)
}

async fn verify_signed_use_request(
    state: &BrokerState,
    body: &SignedUseSecretRequest,
) -> Result<(), BrokerError> {
    if body.nonce.trim().is_empty() || body.nonce.len() > 128 {
        return Err(BrokerError::unauthorized("invalid nonce"));
    }
    let now = chrono::Utc::now().timestamp();
    if (body.timestamp_unix_seconds - now).abs() > 60 {
        return Err(BrokerError::unauthorized("stale secret-use request"));
    }
    let row = sqlx::query(
        "SELECT public_key \
         FROM secret_broker.pty_credentials \
         WHERE pty_session_id = $1 AND revoked_at IS NULL",
    )
    .bind(body.pty_session_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else {
        return Err(BrokerError::unauthorized("unknown PTY credential"));
    };
    let public_key: String = row.get("public_key");
    let public_key = BASE64_STANDARD
        .decode(public_key.as_bytes())
        .map_err(|_| BrokerError::unauthorized("invalid registered PTY key"))?;
    let signature = BASE64_STANDARD
        .decode(body.signature.as_bytes())
        .map_err(|_| BrokerError::unauthorized("invalid request signature"))?;
    let canonical = canonical_use_payload(
        body.pty_session_id,
        &body.program,
        body.timestamp_unix_seconds,
        &body.nonce,
    );
    signature::UnparsedPublicKey::new(&signature::ED25519, public_key)
        .verify(canonical.as_bytes(), &signature)
        .map_err(|_| BrokerError::unauthorized("invalid request signature"))?;

    sqlx::query(
        "DELETE FROM secret_broker.pty_use_nonces WHERE seen_at < NOW() - INTERVAL '5 minutes'",
    )
    .execute(&state.pool)
    .await?;
    let inserted = sqlx::query(
        "INSERT INTO secret_broker.pty_use_nonces (pty_session_id, nonce, seen_at) \
         VALUES ($1, $2, NOW()) \
         ON CONFLICT DO NOTHING",
    )
    .bind(body.pty_session_id)
    .bind(&body.nonce)
    .execute(&state.pool)
    .await?;
    if inserted.rows_affected() == 0 {
        return Err(BrokerError::unauthorized("replayed secret-use request"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Terminal and repository grants in `effective_grants.precedence`.
    const EXPLICIT_PRECEDENCE: i32 = 1;

    fn secret(id: &str, precedence: i32, pairs: &[(&str, &str)]) -> RedeemedSecret {
        RedeemedSecret {
            secret_id: id.to_string(),
            precedence,
            env: pairs
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        }
    }

    #[test]
    fn explicit_grant_replaces_all_terminals_value() {
        let (env, used) = merge_by_precedence(vec![
            secret("gh-read", 0, &[("GH_TOKEN", "read")]),
            secret("gh-write", EXPLICIT_PRECEDENCE, &[("GH_TOKEN", "write")]),
        ])
        .unwrap();
        assert_eq!(env["GH_TOKEN"], "write");
        assert_eq!(used, vec!["gh-write"]);
    }

    #[test]
    fn all_terminals_value_fills_keys_explicit_grants_leave_unset() {
        let (env, mut used) = merge_by_precedence(vec![
            secret("gh-read", 0, &[("GH_TOKEN", "read")]),
            secret("aws", EXPLICIT_PRECEDENCE, &[("AWS_ACCESS_KEY_ID", "a")]),
        ])
        .unwrap();
        used.sort();
        assert_eq!(env["GH_TOKEN"], "read");
        assert_eq!(used, vec!["aws", "gh-read"]);
    }

    #[test]
    fn same_precedence_collision_is_rejected() {
        for rank in [0, EXPLICIT_PRECEDENCE] {
            let err = merge_by_precedence(vec![
                secret("one", rank, &[("GH_TOKEN", "1")]),
                secret("two", rank, &[("GH_TOKEN", "2")]),
            ])
            .unwrap_err();
            assert_eq!(err, "GH_TOKEN");
        }
    }

    #[test]
    fn programs_are_command_names_and_are_deduplicated() {
        let programs = validate_programs(vec![" gh ".into(), "gh".into(), "git".into()]).unwrap();
        assert_eq!(programs, vec!["gh", "git"]);
        for invalid in [
            vec![],
            vec!["/usr/bin/gh".to_string()],
            vec!["gh git".to_string()],
        ] {
            assert!(validate_programs(invalid).is_err());
        }
    }
}
