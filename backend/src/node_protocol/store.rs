use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use ring::digest;
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use super::model::{NodeHello, NodeView, DEDICATED_NODE_ID};
use super::NodeProtocolError;
use crate::db::Pool;

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

    pub(crate) async fn request_pairing(
        &self,
        node_id: Uuid,
        public_key: &[u8],
    ) -> Result<(), NodeProtocolError> {
        if node_id != DEDICATED_NODE_ID || public_key.len() != 32 {
            return Err(NodeProtocolError::AuthenticationFailed);
        }

        sqlx::query(
            "INSERT INTO dev_nodes \
                (id, display_name, credential_kind, pending_public_key, \
                 connection_state, enrolled_at) \
             VALUES ($1, 'sulion-enclave', 'ed25519', $2, 'pending', NULL) \
             ON CONFLICT (id) DO UPDATE SET \
                pending_public_key = EXCLUDED.pending_public_key, \
                connection_state = CASE \
                    WHEN dev_nodes.public_key IS NULL THEN 'pending' \
                    ELSE dev_nodes.connection_state \
                END, \
                updated_at = NOW() \
             WHERE dev_nodes.credential_kind = 'ed25519' \
               AND dev_nodes.pending_public_key IS DISTINCT FROM EXCLUDED.pending_public_key",
        )
        .bind(node_id)
        .bind(public_key)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub(crate) async fn approve_pairing(&self, node_id: Uuid) -> Result<(), NodeProtocolError> {
        let mut tx = self.pool.begin().await?;
        let approved: Uuid = sqlx::query_scalar(
            "UPDATE dev_nodes SET public_key = pending_public_key, pending_public_key = NULL, \
                    connection_id = NULL, connection_state = 'enrolled', \
                    enrolled_at = NOW(), node_disconnected_at = NOW(), updated_at = NOW() \
              WHERE id = $1 AND credential_kind = 'ed25519' \
                AND pending_public_key IS NOT NULL \
          RETURNING id",
        )
        .bind(node_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(NodeProtocolError::NotFound)?;
        mark_sessions_disconnected(&mut tx, approved).await?;
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn credential(
        &self,
        node_id: Uuid,
    ) -> Result<Option<NodeCredential>, NodeProtocolError> {
        Ok(sqlx::query_as::<_, NodeCredential>(
            "SELECT public_key, credential_kind FROM dev_nodes WHERE id = $1",
        )
        .bind(node_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub(crate) async fn record_connection(
        &self,
        hello: &NodeHello,
        connection_id: Uuid,
    ) -> Result<(), NodeProtocolError> {
        let mut tx = self.pool.begin().await?;
        // Lock the node row for the boot transition. Sessions from a prior
        // boot are deliberately NOT ended here: shells live in the devenv
        // server and can outlive a node restart. The first complete
        // heartbeat of the new boot adopts the sessions the node still
        // hosts and ends the rest (see `heartbeat`).
        sqlx::query_as::<_, (Option<Uuid>,)>(
            "SELECT boot_id FROM dev_nodes WHERE id = $1 FOR UPDATE",
        )
        .bind(hello.node_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(NodeProtocolError::UnknownNode)?;

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
            // Adoption: a session still hosted across a node restart carries
            // the boot id it was created under. The node reporting it live
            // re-stamps it onto the current boot — this is what makes a node
            // release leave shells running instead of recording their loss.
            sqlx::query(
                "UPDATE pty_sessions SET node_boot_id = $2, node_disconnected_at = NULL \
                  WHERE node_id = $1 AND state = 'live' AND id = ANY($3) \
                    AND node_boot_id IS DISTINCT FROM $2",
            )
            .bind(node_id)
            .bind(boot_id)
            .bind(live_session_ids)
            .execute(&mut *tx)
            .await?;
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
            reconcile_missing_sessions(&mut tx, node_id, live_session_ids).await?;
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
        let rows = sqlx::query_as::<_, NodeRow>(
            "SELECT id, display_name, protocol_version, boot_id, connection_state, \
                    connected_at, last_heartbeat_at, node_disconnected_at, \
                    pending_public_key \
               FROM dev_nodes ORDER BY display_name, id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| NodeView {
                id: row.id,
                display_name: row.display_name,
                protocol_version: row.protocol_version,
                boot_id: row.boot_id,
                connection_state: row.connection_state,
                connected_at: row.connected_at,
                last_heartbeat_at: row.last_heartbeat_at,
                node_disconnected_at: row.node_disconnected_at,
                heartbeat_timeout_seconds: self.heartbeat_timeout_seconds,
                pending_key_fingerprint: row
                    .pending_public_key
                    .as_deref()
                    .map(public_key_fingerprint),
            })
            .collect())
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
struct NodeRow {
    id: Uuid,
    display_name: String,
    protocol_version: Option<i32>,
    boot_id: Option<Uuid>,
    connection_state: String,
    connected_at: Option<DateTime<Utc>>,
    last_heartbeat_at: Option<DateTime<Utc>>,
    node_disconnected_at: Option<DateTime<Utc>>,
    pending_public_key: Option<Vec<u8>>,
}

#[derive(Debug, FromRow)]
struct ExpiredConnection {
    id: Uuid,
    boot_id: Uuid,
    connection_id: Uuid,
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

/// Orphans every live session for this node that the node's complete
/// inventory does not list — whatever boot it was created under. Sessions the
/// node still hosts were adopted onto the current boot just above. The rest
/// lost their process without a supervised exit (devenv gone, host reboot, or
/// an exit that raced a disconnect), so they land in 'orphaned' — resumable
/// into a fresh PTY — never 'dead', which is reserved for a reported shell
/// exit.
async fn reconcile_missing_sessions(
    tx: &mut Transaction<'_, Postgres>,
    node_id: Uuid,
    live_session_ids: &[Uuid],
) -> Result<(), NodeProtocolError> {
    sqlx::query(
        "UPDATE pty_sessions SET state = 'orphaned', ended_at = COALESCE(ended_at, NOW()), \
                runtime_end_reason = 'node_inventory_missing', node_disconnected_at = NULL, \
                agent_runtime_state = CASE \
                    WHEN agent_runtime_state IN ('starting', 'running') THEN 'exited' \
                    ELSE agent_runtime_state END, \
                agent_runtime_ended_at = CASE \
                    WHEN agent_runtime_state IN ('starting', 'running') \
                    THEN COALESCE(agent_runtime_ended_at, NOW()) \
                    ELSE agent_runtime_ended_at END \
          WHERE node_id = $1 AND state = 'live' \
            AND NOT (id = ANY($2))",
    )
    .bind(node_id)
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

fn public_key_fingerprint(public_key: &[u8]) -> String {
    format!(
        "SHA256:{}",
        base64::engine::general_purpose::STANDARD_NO_PAD
            .encode(digest::digest(&digest::SHA256, public_key))
    )
}
