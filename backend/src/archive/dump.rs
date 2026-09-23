//! The durable database dump that gates every purge cycle.
//!
//! Only tables the archive objects do not already cover: plans, sessions,
//! settings, identities, the rollups, and the per-session skeletons. The
//! transcript-derived tables are excluded because they are what the cycle is
//! about to delete and what the session objects preserve; the code index is
//! excluded because its worker rebuilds it from source checkouts.

use std::collections::BTreeMap;

use anyhow::{anyhow, Context};
use chrono::Utc;
use serde::Serialize;
use tokio::process::Command;

use super::store::ObjectStore;

pub const DUMP_PREFIX: &str = "db";

const EXCLUDED_TABLES: &[&str] = &[
    "events",
    "event_blocks",
    "timeline_turns",
    "timeline_operations",
    "timeline_file_touches",
    "timeline_activity_signals",
    "retrieval_embeddings",
    "retrieval_embedding_sources",
    "retrieval_embedding_backfills",
    "code_roots",
    "code_files",
    "code_symbols",
    "code_references",
    "code_imports",
    "code_index_jobs",
];

#[derive(Debug, Clone, Serialize)]
pub struct DumpOutcome {
    pub key: String,
    pub bytes: i64,
}

/// Runs `pg_dump -Fc` for the durable set and uploads it. `pg_dump` comes
/// from `SULION_PG_DUMP` (the pgdg 18 client in the image) or `PATH`.
pub async fn dump_durable_tables(
    store: &ObjectStore,
    db_url: &str,
) -> anyhow::Result<DumpOutcome> {
    let pg_dump = crate::config::env_optional("SULION_PG_DUMP")
        .unwrap_or_else(|| "pg_dump".to_string());
    let temp = tempfile::NamedTempFile::new().context("create dump temp file")?;
    let mut cmd = Command::new(&pg_dump);
    cmd.arg("--format=custom")
        .arg("--no-owner")
        .arg("--no-privileges")
        .arg("--compress=6")
        .arg("--file")
        .arg(temp.path())
        .arg("--dbname")
        .arg(db_url);
    for table in EXCLUDED_TABLES {
        cmd.arg("--exclude-table").arg(format!("public.{table}"));
    }
    let output = cmd
        .output()
        .await
        .with_context(|| format!("spawn {pg_dump}"))?;
    if !output.status.success() {
        return Err(anyhow!(
            "{pg_dump} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let bytes = std::fs::metadata(temp.path())?.len() as i64;
    if bytes == 0 {
        anyhow::bail!("{pg_dump} produced an empty dump");
    }
    let key = format!(
        "{DUMP_PREFIX}/sulion-durable-{}.dump",
        Utc::now().format("%Y-%m-%dT%H%M%SZ")
    );
    let mut metadata = BTreeMap::new();
    metadata.insert("format".to_string(), "pg_dump-custom".to_string());
    metadata.insert("bytes".to_string(), bytes.to_string());
    store
        .put_file(&key, temp.path(), &metadata)
        .await
        .with_context(|| format!("upload {key}"))?;
    let head = store
        .head(&key)
        .await?
        .ok_or_else(|| anyhow!("{key} is absent after upload"))?;
    if head.content_length != bytes {
        anyhow::bail!("{key}: stored {} bytes, uploaded {bytes}", head.content_length);
    }
    Ok(DumpOutcome { key, bytes })
}
