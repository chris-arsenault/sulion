use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use ring::digest;
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use super::model::{EnrollNodeRequest, EnrollNodeResponse, EnrollmentToken, NodeHello, NodeView};
use super::NodeProtocolError;
use crate::db::Pool;

const DEFAULT_ENROLLMENT_TTL_SECONDS: u64 = 900;
const MIN_ENROLLMENT_TTL_SECONDS: u64 = 60;
const MAX_ENROLLMENT_TTL_SECONDS: u64 = 86_400;

#[derive(Clone)]
pub(crate) struct NodeStore {
    pool: Pool,
    heartbeat_timeout_seconds: i64,
}

impl NodeStore {
    pub(crate) fn new(pool: Pool, heartbeat_timeout_seconds: u64) -> Self {
        Self {
            pool,
            heartbeat_timeout_seconds: heartbeat_timeout_seconds as i64,
        }
    }

    pub(crate) async fn create_enrollment_token(
        &self,
        display_name: &str,
        target_node_id: Uuid,
        ttl_seconds: Option<u64>,
    ) -> Result<EnrollmentToken, NodeProtocolError> {
        let display_name = validate_display_name(display_name)?;
        let ttl_seconds = ttl_seconds.unwrap_or(DEFAULT_ENROLLMENT_TTL_SECONDS);
        if !(MIN_ENROLLMENT_TTL_SECONDS..=MAX_ENROLLMENT_TTL_SECONDS).contains(&ttl_seconds) {
            return Err(NodeProtocolError::InvalidRequest(format!(
                "ttl_seconds must be between {MIN_ENROLLMENT_TTL_SECONDS} and {MAX_ENROLLMENT_TTL_SECONDS}"
            )));
        }

        let mut tx = self.pool.begin().await?;
        let node = sqlx::query(
            "INSERT INTO dev_nodes (id, display_name, credential_kind) \
             VALUES ($1, $2, 'ed25519') \
             ON CONFLICT (id) DO UPDATE SET \
                display_name = EXCLUDED.display_name, updated_at = NOW() \
             WHERE dev_nodes.credential_kind = 'ed25519'",
        )
        .bind(target_node_id)
        .bind(&display_name)
        .execute(&mut *tx)
        .await?;
        if node.rows_affected() == 0 {
            return Err(NodeProtocolError::InvalidRequest(
                "node ID belongs to the standalone runtime".into(),
            ));
        }

        let token = super::random_url_token(32)?;
        let expires_at: DateTime<Utc> = sqlx::query_scalar(
            "INSERT INTO dev_node_enrollment_tokens \
                (id, token_hash, display_name, target_node_id, expires_at) \
             VALUES ($1, $2, $3, $4, NOW() + make_interval(secs => $5::INT)) \
             RETURNING expires_at",
        )
        .bind(Uuid::new_v4())
        .bind(token_hash(&token))
        .bind(&display_name)
        .bind(target_node_id)
        .bind(ttl_seconds as i32)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;

        Ok(EnrollmentToken {
            token,
            expires_at,
            target_node_id,
        })
    }

    pub(crate) async fn enroll(
        &self,
        request: EnrollNodeRequest,
    ) -> Result<EnrollNodeResponse, NodeProtocolError> {
        let public_key = decode_public_key(&request.public_key)?;
        let mut tx = self.pool.begin().await?;
        let token = consume_enrollment_token(&mut tx, &request.token).await?;
        let display_name: String = sqlx::query_scalar(
            "UPDATE dev_nodes SET public_key = $2, \
                    connection_id = NULL, connection_state = 'enrolled', \
                    node_disconnected_at = NOW(), updated_at = NOW() \
              WHERE id = $1 AND credential_kind = 'ed25519' \
          RETURNING display_name",
        )
        .bind(token.target_node_id)
        .bind(&public_key)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(NodeProtocolError::NotFound)?;
        mark_sessions_disconnected(&mut tx, token.target_node_id).await?;
        tx.commit().await?;

        Ok(EnrollNodeResponse {
            node_id: token.target_node_id,
            display_name,
            protocol_version: super::model::NODE_PROTOCOL_VERSION,
        })
    }

    pub(crate) async fn credential(
        &self,
        node_id: Uuid,
    ) -> Result<NodeCredential, NodeProtocolError> {
        sqlx::query_as::<_, NodeCredential>(
            "SELECT public_key, credential_kind FROM dev_nodes WHERE id = $1",
        )
        .bind(node_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(NodeProtocolError::UnknownNode)
    }

    pub(crate) async fn record_connection(
        &self,
        hello: &NodeHello,
        connection_id: Uuid,
    ) -> Result<(), NodeProtocolError> {
        let mut tx = self.pool.begin().await?;
        let previous_boot_id: Option<Uuid> = sqlx::query_as::<_, (Option<Uuid>,)>(
            "SELECT boot_id FROM dev_nodes WHERE id = $1 FOR UPDATE",
        )
        .bind(hello.node_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(NodeProtocolError::UnknownNode)?
        .0;
        if let Some(previous_boot_id) = previous_boot_id {
            if previous_boot_id != hello.boot_id {
                end_sessions_from_prior_boot(
                    &mut tx,
                    hello.node_id,
                    previous_boot_id,
                    hello.boot_id,
                )
                .await?;
            }
        }

        sqlx::query(
            "UPDATE dev_nodes SET protocol_version = $2, boot_id = $3, connection_id = $4, \
                    connection_state = 'connected', connected_at = NOW(), \
                    last_heartbeat_at = NOW(), node_disconnected_at = NULL, updated_at = NOW() \
              WHERE id = $1",
        )
        .bind(hello.node_id)
        .bind(hello.protocol_version as i32)
        .bind(hello.boot_id)
        .bind(connection_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn heartbeat(
        &self,
        node_id: Uuid,
        boot_id: Uuid,
        connection_id: Uuid,
        live_session_ids: &[Uuid],
        inventory_complete: bool,
    ) -> Result<bool, NodeProtocolError> {
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE dev_nodes SET last_heartbeat_at = NOW(), connection_state = 'connected', \
                    node_disconnected_at = NULL, updated_at = NOW() \
              WHERE id = $1 AND boot_id = $2 AND connection_id = $3",
        )
        .bind(node_id)
        .bind(boot_id)
        .bind(connection_id)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(false);
        }
        if !live_session_ids.is_empty() {
            sqlx::query(
                "UPDATE pty_sessions SET node_disconnected_at = NULL \
                  WHERE node_id = $1 AND node_boot_id = $2 AND state = 'live' \
                    AND id = ANY($3)",
            )
            .bind(node_id)
            .bind(boot_id)
            .bind(live_session_ids)
            .execute(&mut *tx)
            .await?;
        }
        if inventory_complete {
            reconcile_missing_sessions(&mut tx, node_id, boot_id, live_session_ids).await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    pub(crate) async fn disconnect(
        &self,
        node_id: Uuid,
        boot_id: Uuid,
        connection_id: Uuid,
    ) -> Result<bool, NodeProtocolError> {
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE dev_nodes SET connection_state = 'disconnected', connection_id = NULL, \
                    node_disconnected_at = NOW(), updated_at = NOW() \
              WHERE id = $1 AND boot_id = $2 AND connection_id = $3",
        )
        .bind(node_id)
        .bind(boot_id)
        .bind(connection_id)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() > 0 {
            sqlx::query(
                "UPDATE pty_sessions SET node_disconnected_at = COALESCE(node_disconnected_at, NOW()) \
                  WHERE node_id = $1 AND node_boot_id = $2 AND state = 'live'",
            )
            .bind(node_id)
            .bind(boot_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(updated.rows_affected() > 0)
    }

    pub(crate) async fn expire_heartbeats(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<(Uuid, Uuid, Uuid)>, NodeProtocolError> {
        let cutoff = now - Duration::seconds(self.heartbeat_timeout_seconds);
        let mut tx = self.pool.begin().await?;
        let expired: Vec<ExpiredConnection> = sqlx::query_as(
            "SELECT id, boot_id, connection_id FROM dev_nodes \
              WHERE connection_state = 'connected' AND last_heartbeat_at < $1 \
                AND connection_id IS NOT NULL AND boot_id IS NOT NULL \
              FOR UPDATE",
        )
        .bind(cutoff)
        .fetch_all(&mut *tx)
        .await?;
        for item in &expired {
            sqlx::query(
                "UPDATE dev_nodes SET connection_state = 'disconnected', connection_id = NULL, \
                        node_disconnected_at = $4, updated_at = $4 \
                  WHERE id = $1 AND boot_id = $2 AND connection_id = $3",
            )
            .bind(item.id)
            .bind(item.boot_id)
            .bind(item.connection_id)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE pty_sessions SET node_disconnected_at = COALESCE(node_disconnected_at, $3) \
                  WHERE node_id = $1 AND node_boot_id = $2 AND state = 'live'",
            )
            .bind(item.id)
            .bind(item.boot_id)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(expired
            .into_iter()
            .map(|item| (item.id, item.boot_id, item.connection_id))
            .collect())
    }

    pub(crate) async fn list_nodes(&self) -> Result<Vec<NodeView>, NodeProtocolError> {
        Ok(sqlx::query_as::<_, NodeView>(
            "SELECT id, display_name, protocol_version, boot_id, connection_state, \
                    connected_at, last_heartbeat_at, node_disconnected_at, \
                    $1::BIGINT AS heartbeat_timeout_seconds \
               FROM dev_nodes ORDER BY display_name, id",
        )
        .bind(self.heartbeat_timeout_seconds)
        .fetch_all(&self.pool)
        .await?)
    }

    pub(crate) async fn ensure_internal_node(
        &self,
        node_id: Uuid,
        display_name: &str,
    ) -> Result<(), NodeProtocolError> {
        let display_name = validate_display_name(display_name)?;
        let result = sqlx::query(
            "INSERT INTO dev_nodes (id, display_name, credential_kind) \
             VALUES ($1, $2, 'internal') \
             ON CONFLICT (id) DO UPDATE SET \
                display_name = EXCLUDED.display_name, updated_at = NOW() \
             WHERE dev_nodes.credential_kind = 'internal'",
        )
        .bind(node_id)
        .bind(display_name)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(NodeProtocolError::InvalidRequest(
                "standalone node ID belongs to the remote development node".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, FromRow)]
pub(crate) struct NodeCredential {
    pub public_key: Option<Vec<u8>>,
    pub credential_kind: String,
}

#[derive(Debug, FromRow)]
struct EnrollmentTokenRow {
    target_node_id: Uuid,
}

#[derive(Debug, FromRow)]
struct ExpiredConnection {
    id: Uuid,
    boot_id: Uuid,
    connection_id: Uuid,
}

async fn consume_enrollment_token(
    tx: &mut Transaction<'_, Postgres>,
    token: &str,
) -> Result<EnrollmentTokenRow, NodeProtocolError> {
    if token.len() < 32 || token.len() > 256 {
        return Err(NodeProtocolError::InvalidEnrollmentToken);
    }
    sqlx::query_as::<_, EnrollmentTokenRow>(
        "UPDATE dev_node_enrollment_tokens SET used_at = NOW() \
          WHERE token_hash = $1 AND used_at IS NULL AND expires_at > NOW() \
      RETURNING target_node_id",
    )
    .bind(token_hash(token))
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(NodeProtocolError::InvalidEnrollmentToken)
}

async fn mark_sessions_disconnected(
    tx: &mut Transaction<'_, Postgres>,
    node_id: Uuid,
) -> Result<(), NodeProtocolError> {
    sqlx::query(
        "UPDATE pty_sessions SET node_disconnected_at = COALESCE(node_disconnected_at, NOW()) \
          WHERE node_id = $1 AND state = 'live'",
    )
    .bind(node_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn end_sessions_from_prior_boot(
    tx: &mut Transaction<'_, Postgres>,
    node_id: Uuid,
    previous_boot_id: Uuid,
    current_boot_id: Uuid,
) -> Result<(), NodeProtocolError> {
    sqlx::query(
        "UPDATE pty_sessions SET state = 'dead', ended_at = COALESCE(ended_at, NOW()), \
                runtime_end_reason = 'node_reboot', node_disconnected_at = NOW(), \
                agent_runtime_state = CASE \
                    WHEN agent_runtime_state IN ('starting', 'running') THEN 'exited' \
                    ELSE agent_runtime_state END, \
                agent_runtime_ended_at = CASE \
                    WHEN agent_runtime_state IN ('starting', 'running') \
                    THEN COALESCE(agent_runtime_ended_at, NOW()) \
                    ELSE agent_runtime_ended_at END \
          WHERE node_id = $1 AND node_boot_id = $2 AND node_boot_id <> $3 \
            AND state = 'live'",
    )
    .bind(node_id)
    .bind(previous_boot_id)
    .bind(current_boot_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn reconcile_missing_sessions(
    tx: &mut Transaction<'_, Postgres>,
    node_id: Uuid,
    boot_id: Uuid,
    live_session_ids: &[Uuid],
) -> Result<(), NodeProtocolError> {
    sqlx::query(
        "UPDATE pty_sessions SET state = 'dead', ended_at = COALESCE(ended_at, NOW()), \
                runtime_end_reason = 'node_inventory_missing', node_disconnected_at = NULL, \
                agent_runtime_state = CASE \
                    WHEN agent_runtime_state IN ('starting', 'running') THEN 'exited' \
                    ELSE agent_runtime_state END, \
                agent_runtime_ended_at = CASE \
                    WHEN agent_runtime_state IN ('starting', 'running') \
                    THEN COALESCE(agent_runtime_ended_at, NOW()) \
                    ELSE agent_runtime_ended_at END \
          WHERE node_id = $1 AND node_boot_id = $2 AND state = 'live' \
            AND NOT (id = ANY($3))",
    )
    .bind(node_id)
    .bind(boot_id)
    .bind(live_session_ids)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn validate_display_name(value: &str) -> Result<String, NodeProtocolError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 80 || value.chars().any(char::is_control) {
        return Err(NodeProtocolError::InvalidRequest(
            "display_name must be 1-80 printable characters".into(),
        ));
    }
    Ok(value.to_string())
}

fn decode_public_key(value: &str) -> Result<Vec<u8>, NodeProtocolError> {
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| NodeProtocolError::InvalidRequest("invalid public key encoding".into()))?;
    if decoded.len() != 32 {
        return Err(NodeProtocolError::InvalidRequest(
            "Ed25519 public key must be 32 bytes".into(),
        ));
    }
    Ok(decoded)
}

fn token_hash(token: &str) -> Vec<u8> {
    digest::digest(&digest::SHA256, token.as_bytes())
        .as_ref()
        .to_vec()
}
