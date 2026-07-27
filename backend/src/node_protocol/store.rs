use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use ring::digest;
use serde_json::Value;
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use super::model::{
    EnrollNodeRequest, EnrollNodeResponse, EnrollmentToken, NodeHello, NodeOperationKind,
    NodeOperationView, NodeView, OperationResultPayload,
};
use super::{ConnectionAcceptance, NodeProtocolError};
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
        target_node_id: Option<Uuid>,
        ttl_seconds: Option<u64>,
    ) -> Result<EnrollmentToken, NodeProtocolError> {
        let display_name = validate_display_name(display_name)?;
        let ttl_seconds = ttl_seconds.unwrap_or(DEFAULT_ENROLLMENT_TTL_SECONDS);
        if !(MIN_ENROLLMENT_TTL_SECONDS..=MAX_ENROLLMENT_TTL_SECONDS).contains(&ttl_seconds) {
            return Err(NodeProtocolError::InvalidRequest(format!(
                "ttl_seconds must be between {MIN_ENROLLMENT_TTL_SECONDS} and {MAX_ENROLLMENT_TTL_SECONDS}"
            )));
        }
        if let Some(node_id) = target_node_id {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS( \
                    SELECT 1 FROM dev_nodes \
                     WHERE id = $1 AND credential_kind = 'ed25519' AND revoked_at IS NULL \
                 )",
            )
            .bind(node_id)
            .fetch_one(&self.pool)
            .await?;
            if !exists {
                return Err(NodeProtocolError::NotFound);
            }
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
        .fetch_one(&self.pool)
        .await?;
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
        let fingerprint = credential_fingerprint(&public_key);
        let mut tx = self.pool.begin().await?;
        let token = consume_enrollment_token(&mut tx, &request.token).await?;
        let response = match token.target_node_id {
            Some(node_id) => rotate_credential(&mut tx, node_id, &public_key, &fingerprint).await?,
            None => create_node(&mut tx, &token.display_name, &public_key, &fingerprint).await?,
        };
        tx.commit().await?;
        Ok(response)
    }

    pub(crate) async fn revoke(&self, node_id: Uuid) -> Result<(), NodeProtocolError> {
        let mut tx = self.pool.begin().await?;
        let credential_kind: String =
            sqlx::query_scalar("SELECT credential_kind FROM dev_nodes WHERE id = $1 FOR UPDATE")
                .bind(node_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(NodeProtocolError::NotFound)?;
        if credential_kind != "ed25519" {
            return Err(NodeProtocolError::InvalidRequest(
                "internal standalone nodes do not have revocable credentials".into(),
            ));
        }
        let result = sqlx::query(
            "UPDATE dev_nodes \
                SET revoked_at = NOW(), connection_state = 'revoked', \
                    connection_id = NULL, node_disconnected_at = NOW(), updated_at = NOW() \
              WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(node_id)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            return Err(NodeProtocolError::NotFound);
        }
        sqlx::query(
            "UPDATE dev_node_credentials SET revoked_at = COALESCE(revoked_at, NOW()) \
              WHERE node_id = $1 AND replaced_at IS NULL",
        )
        .bind(node_id)
        .execute(&mut *tx)
        .await?;
        mark_sessions_disconnected(&mut tx, node_id).await?;
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn credential(
        &self,
        node_id: Uuid,
    ) -> Result<NodeCredential, NodeProtocolError> {
        let credential = sqlx::query_as::<_, NodeCredential>(
            "SELECT public_key, credential_kind, revoked_at \
               FROM dev_nodes WHERE id = $1",
        )
        .bind(node_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(NodeProtocolError::UnknownNode)?;
        if credential.revoked_at.is_some() {
            return Err(NodeProtocolError::Revoked);
        }
        Ok(credential)
    }

    pub(crate) async fn record_connection(
        &self,
        hello: &NodeHello,
        connection_id: Uuid,
    ) -> Result<ConnectionAcceptance, NodeProtocolError> {
        let mut tx = self.pool.begin().await?;
        let previous_boot_id: Option<Uuid> = sqlx::query_as::<_, (Option<Uuid>,)>(
            "SELECT boot_id FROM dev_nodes \
              WHERE id = $1 AND revoked_at IS NULL FOR UPDATE",
        )
        .bind(hello.node_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(NodeProtocolError::UnknownNode)?
        .0;
        let acceptance = if previous_boot_id == Some(hello.boot_id) {
            ConnectionAcceptance::SameBoot
        } else {
            if let Some(previous_boot_id) = previous_boot_id {
                end_sessions_from_prior_boot(
                    &mut tx,
                    hello.node_id,
                    previous_boot_id,
                    hello.boot_id,
                )
                .await?;
                sqlx::query(
                    "UPDATE dev_node_boots SET disconnected_at = COALESCE(disconnected_at, NOW()) \
                      WHERE node_id = $1 AND boot_id = $2",
                )
                .bind(hello.node_id)
                .bind(previous_boot_id)
                .execute(&mut *tx)
                .await?;
            }
            ConnectionAcceptance::NewBoot
        };

        upsert_boot(&mut tx, hello).await?;
        sqlx::query(
            "UPDATE dev_nodes SET \
                protocol_version = $2, control_protocol_min = $3, control_protocol_max = $4, \
                build_git_sha = $5, capabilities = $6, docker_policy = $7, docker_info = $8, \
                path_contract_version = $9, boot_id = $10, connection_id = $11, \
                connection_state = 'connected', compatibility_error = NULL, \
                observed_release_digest = $12, connected_at = NOW(), \
                last_heartbeat_at = NOW(), node_disconnected_at = NULL, updated_at = NOW() \
              WHERE id = $1",
        )
        .bind(hello.node_id)
        .bind(hello.protocol_version as i32)
        .bind(hello.supported_control_min as i32)
        .bind(hello.supported_control_max as i32)
        .bind(&hello.build_git_sha)
        .bind(serde_json::to_value(&hello.capabilities)?)
        .bind(hello.docker_policy.as_str())
        .bind(serde_json::to_value(&hello.docker_info)?)
        .bind(hello.path_contract_version as i32)
        .bind(hello.boot_id)
        .bind(connection_id)
        .bind(&hello.observed_release_digest)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(acceptance)
    }

    pub(crate) async fn record_incompatible(
        &self,
        hello: &NodeHello,
        reason: &str,
    ) -> Result<(), NodeProtocolError> {
        sqlx::query(
            "UPDATE dev_nodes SET \
                protocol_version = $2, control_protocol_min = $3, control_protocol_max = $4, \
                build_git_sha = $5, capabilities = $6, docker_policy = $7, docker_info = $8, \
                path_contract_version = $9, boot_id = $10, connection_id = NULL, \
                connection_state = 'incompatible', compatibility_error = $11, \
                observed_release_digest = $12, node_disconnected_at = NOW(), updated_at = NOW() \
              WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(hello.node_id)
        .bind(hello.protocol_version as i32)
        .bind(hello.supported_control_min as i32)
        .bind(hello.supported_control_max as i32)
        .bind(&hello.build_git_sha)
        .bind(serde_json::to_value(&hello.capabilities)?)
        .bind(hello.docker_policy.as_str())
        .bind(serde_json::to_value(&hello.docker_info)?)
        .bind(hello.path_contract_version as i32)
        .bind(hello.boot_id)
        .bind(reason)
        .bind(&hello.observed_release_digest)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub(crate) async fn heartbeat(
        &self,
        node_id: Uuid,
        boot_id: Uuid,
        connection_id: Uuid,
        live_session_ids: &[Uuid],
        inventory_complete: bool,
        observed_release_digest: Option<&str>,
    ) -> Result<bool, NodeProtocolError> {
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE dev_nodes SET last_heartbeat_at = NOW(), connection_state = 'connected', \
                    node_disconnected_at = NULL, observed_release_digest = COALESCE($4, observed_release_digest), \
                    updated_at = NOW() \
              WHERE id = $1 AND boot_id = $2 AND connection_id = $3 \
                AND revoked_at IS NULL",
        )
        .bind(node_id)
        .bind(boot_id)
        .bind(connection_id)
        .bind(observed_release_digest)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(false);
        }
        sqlx::query(
            "UPDATE dev_node_boots SET last_seen_at = NOW(), observed_release_digest = COALESCE($3, observed_release_digest) \
              WHERE node_id = $1 AND boot_id = $2",
        )
        .bind(node_id)
        .bind(boot_id)
        .bind(observed_release_digest)
        .execute(&mut *tx)
        .await?;
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
              WHERE id = $1 AND boot_id = $2 AND connection_id = $3 \
                AND revoked_at IS NULL",
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
            sqlx::query(
                "UPDATE dev_node_boots SET disconnected_at = COALESCE(disconnected_at, NOW()) \
                  WHERE node_id = $1 AND boot_id = $2",
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
                "UPDATE dev_nodes SET connection_state = 'stale', connection_id = NULL, \
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
}

impl NodeStore {
    pub(crate) async fn list_nodes(&self) -> Result<Vec<NodeView>, NodeProtocolError> {
        let rows = sqlx::query_as::<_, NodeView>(
            "SELECT id, display_name, \
                    CASE WHEN revoked_at IS NOT NULL THEN 'revoked' \
                         WHEN credential_kind = 'internal' THEN 'internal' \
                         ELSE 'active' END AS credential_status, \
                    protocol_version, build_git_sha, capabilities, docker_policy, docker_info, \
                    path_contract_version, boot_id, connection_state, compatibility_error, \
                    desired_release_digest, observed_release_digest, drain_state, connected_at, \
                    last_heartbeat_at, node_disconnected_at, $1::BIGINT AS heartbeat_timeout_seconds \
               FROM dev_nodes ORDER BY display_name, id",
        )
        .bind(self.heartbeat_timeout_seconds)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
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
                "standalone node ID belongs to an enrolled remote node".into(),
            ));
        }
        Ok(())
    }

    pub(crate) async fn request_operation(
        &self,
        node_id: Uuid,
        idempotency_key: &str,
        kind: NodeOperationKind,
        resource_id: Option<Uuid>,
        payload: Value,
    ) -> Result<NodeOperationView, NodeProtocolError> {
        validate_idempotency_key(idempotency_key)?;
        let operation_id = Uuid::new_v4();
        let operation = sqlx::query_as::<_, NodeOperationView>(
            "INSERT INTO dev_node_operations \
                (operation_id, idempotency_key, node_id, kind, resource_id, request_payload) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (node_id, idempotency_key) DO UPDATE \
                SET idempotency_key = EXCLUDED.idempotency_key \
             RETURNING operation_id, idempotency_key, node_id, kind, resource_id, \
                       request_payload, requested_at, dispatched_at, completed_at, status, \
                       result, error_code, error_message, dispatch_boot_id, dispatch_count",
        )
        .bind(operation_id)
        .bind(idempotency_key)
        .bind(node_id)
        .bind(kind.as_str())
        .bind(resource_id)
        .bind(&payload)
        .fetch_one(&self.pool)
        .await?;
        if operation.kind != kind.as_str()
            || operation.resource_id != resource_id
            || operation.request_payload != payload
        {
            return Err(NodeProtocolError::IdempotencyConflict);
        }
        Ok(operation)
    }

    pub(crate) async fn mark_dispatched(
        &self,
        operation_id: Uuid,
        node_id: Uuid,
        boot_id: Uuid,
    ) -> Result<Option<NodeOperationView>, NodeProtocolError> {
        let operation = sqlx::query_as::<_, NodeOperationView>(
            "UPDATE dev_node_operations SET status = 'dispatched', \
                    dispatched_at = COALESCE(dispatched_at, NOW()), \
                    dispatch_boot_id = $3, dispatch_count = dispatch_count + 1 \
              WHERE operation_id = $1 AND node_id = $2 \
                AND status IN ('pending', 'dispatched') \
          RETURNING operation_id, idempotency_key, node_id, kind, resource_id, \
                    request_payload, requested_at, dispatched_at, completed_at, status, \
                    result, error_code, error_message, dispatch_boot_id, dispatch_count",
        )
        .bind(operation_id)
        .bind(node_id)
        .bind(boot_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(operation)
    }

    pub(crate) async fn pending_operations(
        &self,
        node_id: Uuid,
    ) -> Result<Vec<NodeOperationView>, NodeProtocolError> {
        Ok(sqlx::query_as::<_, NodeOperationView>(
            "SELECT operation_id, idempotency_key, node_id, kind, resource_id, \
                    request_payload, requested_at, dispatched_at, completed_at, status, \
                    result, error_code, error_message, dispatch_boot_id, dispatch_count \
               FROM dev_node_operations \
              WHERE node_id = $1 AND status IN ('pending', 'dispatched') \
              ORDER BY requested_at, operation_id",
        )
        .bind(node_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub(crate) async fn complete_operation(
        &self,
        node_id: Uuid,
        operation_id: Uuid,
        result: &OperationResultPayload,
    ) -> Result<Option<NodeOperationView>, NodeProtocolError> {
        let status = match result.status {
            super::model::OperationResultStatus::Succeeded => "succeeded",
            super::model::OperationResultStatus::Failed => "failed",
        };
        Ok(sqlx::query_as::<_, NodeOperationView>(
            "UPDATE dev_node_operations SET status = $3, completed_at = NOW(), \
                    result = $4, error_code = $5, error_message = $6 \
              WHERE operation_id = $1 AND node_id = $2 \
                AND status IN ('pending', 'dispatched') \
          RETURNING operation_id, idempotency_key, node_id, kind, resource_id, \
                    request_payload, requested_at, dispatched_at, completed_at, status, \
                    result, error_code, error_message, dispatch_boot_id, dispatch_count",
        )
        .bind(operation_id)
        .bind(node_id)
        .bind(status)
        .bind(&result.result)
        .bind(&result.error_code)
        .bind(&result.error_message)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub(crate) async fn operation(
        &self,
        operation_id: Uuid,
    ) -> Result<Option<NodeOperationView>, NodeProtocolError> {
        Ok(sqlx::query_as::<_, NodeOperationView>(
            "SELECT operation_id, idempotency_key, node_id, kind, resource_id, \
                    request_payload, requested_at, dispatched_at, completed_at, status, \
                    result, error_code, error_message, dispatch_boot_id, dispatch_count \
               FROM dev_node_operations WHERE operation_id = $1",
        )
        .bind(operation_id)
        .fetch_optional(&self.pool)
        .await?)
    }
}

#[derive(Debug, FromRow)]
pub(crate) struct NodeCredential {
    pub public_key: Option<Vec<u8>>,
    pub credential_kind: String,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromRow)]
struct EnrollmentTokenRow {
    display_name: String,
    target_node_id: Option<Uuid>,
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
    let row = sqlx::query_as::<_, EnrollmentTokenRow>(
        "UPDATE dev_node_enrollment_tokens SET used_at = NOW() \
          WHERE token_hash = $1 AND used_at IS NULL AND expires_at > NOW() \
      RETURNING display_name, target_node_id",
    )
    .bind(token_hash(token))
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(NodeProtocolError::InvalidEnrollmentToken)?;
    Ok(row)
}

async fn create_node(
    tx: &mut Transaction<'_, Postgres>,
    display_name: &str,
    public_key: &[u8],
    fingerprint: &str,
) -> Result<EnrollNodeResponse, NodeProtocolError> {
    let node_id = Uuid::new_v4();
    let generation: i32 = sqlx::query_scalar(
        "INSERT INTO dev_nodes \
            (id, display_name, public_key, credential_fingerprint, credential_kind) \
         VALUES ($1, $2, $3, $4, 'ed25519') \
         RETURNING credential_generation",
    )
    .bind(node_id)
    .bind(display_name)
    .bind(public_key)
    .bind(fingerprint)
    .fetch_one(&mut **tx)
    .await?;
    insert_credential(tx, node_id, generation, public_key, fingerprint).await?;
    Ok(EnrollNodeResponse {
        node_id,
        display_name: display_name.to_string(),
        credential_generation: generation,
        protocol_version: super::model::NODE_PROTOCOL_VERSION,
    })
}

async fn rotate_credential(
    tx: &mut Transaction<'_, Postgres>,
    node_id: Uuid,
    public_key: &[u8],
    fingerprint: &str,
) -> Result<EnrollNodeResponse, NodeProtocolError> {
    let row: Option<(String, i32)> = sqlx::query_as(
        "UPDATE dev_nodes SET public_key = $2, credential_fingerprint = $3, \
                credential_generation = credential_generation + 1, \
                credential_kind = 'ed25519', connection_id = NULL, \
                connection_state = 'enrolled', compatibility_error = NULL, \
                node_disconnected_at = NOW(), updated_at = NOW() \
          WHERE id = $1 AND revoked_at IS NULL \
      RETURNING display_name, credential_generation",
    )
    .bind(node_id)
    .bind(public_key)
    .bind(fingerprint)
    .fetch_optional(&mut **tx)
    .await?;
    let (display_name, generation) = row.ok_or(NodeProtocolError::NotFound)?;
    sqlx::query(
        "UPDATE dev_node_credentials SET replaced_at = COALESCE(replaced_at, NOW()) \
          WHERE node_id = $1 AND generation = $2",
    )
    .bind(node_id)
    .bind(generation - 1)
    .execute(&mut **tx)
    .await?;
    insert_credential(tx, node_id, generation, public_key, fingerprint).await?;
    mark_sessions_disconnected(tx, node_id).await?;
    Ok(EnrollNodeResponse {
        node_id,
        display_name,
        credential_generation: generation,
        protocol_version: super::model::NODE_PROTOCOL_VERSION,
    })
}

async fn insert_credential(
    tx: &mut Transaction<'_, Postgres>,
    node_id: Uuid,
    generation: i32,
    public_key: &[u8],
    fingerprint: &str,
) -> Result<(), NodeProtocolError> {
    sqlx::query(
        "INSERT INTO dev_node_credentials \
            (node_id, generation, public_key, credential_fingerprint) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(node_id)
    .bind(generation)
    .bind(public_key)
    .bind(fingerprint)
    .execute(&mut **tx)
    .await?;
    Ok(())
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

async fn upsert_boot(
    tx: &mut Transaction<'_, Postgres>,
    hello: &NodeHello,
) -> Result<(), NodeProtocolError> {
    sqlx::query(
        "INSERT INTO dev_node_boots \
            (node_id, boot_id, build_git_sha, protocol_version, observed_release_digest) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (node_id, boot_id) DO UPDATE SET \
            last_seen_at = NOW(), disconnected_at = NULL, \
            build_git_sha = EXCLUDED.build_git_sha, \
            protocol_version = EXCLUDED.protocol_version, \
            observed_release_digest = EXCLUDED.observed_release_digest",
    )
    .bind(hello.node_id)
    .bind(hello.boot_id)
    .bind(&hello.build_git_sha)
    .bind(hello.protocol_version as i32)
    .bind(&hello.observed_release_digest)
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

fn validate_idempotency_key(value: &str) -> Result<(), NodeProtocolError> {
    if value.is_empty()
        || value.len() > 200
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
    {
        return Err(NodeProtocolError::InvalidRequest(
            "invalid idempotency key".into(),
        ));
    }
    Ok(())
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

fn credential_fingerprint(public_key: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(digest::digest(&digest::SHA256, public_key).as_ref())
}
