use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::correlate::{self, CorrelateMsg};
use crate::db;
use crate::ingest::{Ingester, IngesterConfig};

pub const ENABLE_FIXTURES_ENV: &str = "SULION_ENABLE_E2E_FIXTURES";
pub const MOCK_TERMINAL_FIXTURE: &str = "mock-terminal";

const CLAUDE_PROJECT_HASH: &str = "atlas-claude-project";
const CLAUDE_SESSION_UUID: &str = "11111111-1111-4111-8111-111111111111";
const CODEX_PARENT_SESSION_UUID: &str = "019da571-ab6d-72e2-94b2-4fc5544f53d2";
const CODEX_CHILD_SESSION_UUID: &str = "019da789-c2a6-7f80-b71b-4dc90c7f1802";

const CODEX_RICH_LINEAGE_PARENT: &str =
    include_str!("../tests/fixtures/codex-rich-lineage-parent.jsonl");
const CODEX_RICH_LINEAGE_CHILD: &str =
    include_str!("../tests/fixtures/codex-rich-lineage-child.jsonl");
const MOCK_TERMINAL_SCRIPT: &str = r#"#!/usr/bin/env bash
set -euo pipefail

prompt() {
  printf 'mock> '
}

report_resize() {
  local rows cols
  if read -r rows cols < <(stty size 2>/dev/null); then
    printf '\r\nMOCK_RESIZE rows=%s cols=%s\r\n' "$rows" "$cols"
  else
    printf '\r\nMOCK_RESIZE unavailable\r\n'
  fi
  prompt
}

trap report_resize WINCH

printf 'SULION MOCK TERMINAL READY\r\n'
printf 'SNAPSHOT_SENTINEL\r\n'
prompt

while IFS= read -r line; do
  case "$line" in
    status)
      printf '\r\nMOCK_STATUS ok\r\n'
      ;;
    stream)
      printf '\r\nSTREAM_CHUNK_1\r\n'
      sleep 0.1
      printf 'STREAM_CHUNK_2\r\n'
      sleep 0.1
      printf 'STREAM_CHUNK_3\r\n'
      ;;
    exit|die)
      printf '\r\nMOCK_EXIT 7\r\n'
      exit 7
      ;;
    "")
      printf '\r\nMOCK_EMPTY_INPUT\r\n'
      ;;
    *)
      printf '\r\nMOCK_ECHO %s\r\n' "$line"
      ;;
  esac
  prompt
done
"#;

#[derive(Clone, Debug)]
pub struct SeedConfig {
    pub db_url: String,
    pub base_url: String,
    pub repos_root: PathBuf,
    pub library_root: PathBuf,
    pub claude_projects_dir: PathBuf,
    pub codex_sessions_dir: PathBuf,
}

impl SeedConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            db_url: std::env::var("SULION_E2E_DB_URL")
                .or_else(|_| std::env::var("SULION_DB_URL"))
                .context("SULION_E2E_DB_URL or SULION_DB_URL is required")?,
            base_url: std::env::var("SULION_E2E_BASE_URL")
                .context("SULION_E2E_BASE_URL is required")?,
            repos_root: env_path("SULION_REPOS_ROOT")?,
            library_root: env_path("SULION_LIBRARY_ROOT")?,
            claude_projects_dir: env_path("SULION_CLAUDE_PROJECTS")?,
            codex_sessions_dir: env_path("SULION_CODEX_SESSIONS")?,
        })
    }
}

#[derive(Deserialize)]
struct CreatedSession {
    id: Uuid,
}

#[derive(Clone, Copy)]
struct SessionIds {
    claude: Uuid,
    codex: Uuid,
}

pub async fn seed_from_env() -> anyhow::Result<()> {
    let cfg = SeedConfig::from_env()?;
    seed(cfg).await
}

pub async fn seed(cfg: SeedConfig) -> anyhow::Result<()> {
    let pool = db::connect(&cfg.db_url).await.context("connect seed db")?;
    reset_database(&pool).await?;
    prepare_roots(&cfg)?;
    write_mock_terminal_script(&cfg.repos_root)?;
    prepare_repos(&cfg.repos_root)?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("build reqwest client")?;
    let session_ids = create_sessions(&client, &cfg.base_url).await?;
    annotate_sessions(&pool, session_ids).await?;

    write_transcripts(&cfg)?;
    correlate_sessions(&pool, session_ids).await?;
    ingest_transcripts(&pool, &cfg).await?;
    verify_seed(&client, &cfg.base_url, session_ids).await?;

    Ok(())
}

fn env_path(name: &str) -> anyhow::Result<PathBuf> {
    Ok(PathBuf::from(
        std::env::var(name).with_context(|| format!("{name} is required"))?,
    ))
}

pub fn fixtures_enabled() -> bool {
    matches!(
        std::env::var(ENABLE_FIXTURES_ENV).ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

pub fn mock_terminal_script_path(repos_root: &Path) -> PathBuf {
    repos_root.join(".sulion-e2e").join("mock-terminal.sh")
}

async fn reset_database(pool: &db::Pool) -> anyhow::Result<()> {
    sqlx::query(
        "TRUNCATE events, event_blocks, timeline_turns, ingester_state, \
         claude_sessions, pty_sessions, repos RESTART IDENTITY CASCADE",
    )
    .execute(pool)
    .await
    .context("truncate e2e tables")?;
    Ok(())
}

fn prepare_roots(cfg: &SeedConfig) -> anyhow::Result<()> {
    for path in [
        &cfg.repos_root,
        &cfg.library_root,
        &cfg.claude_projects_dir,
        &cfg.codex_sessions_dir,
    ] {
        std::fs::create_dir_all(path).with_context(|| format!("create {}", path.display()))?;
        clear_directory(path)?;
    }
    Ok(())
}

fn clear_directory(path: &Path) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(path).with_context(|| format!("read {}", path.display()))? {
        let entry = entry?;
        let child = entry.path();
        if child.is_dir() {
            std::fs::remove_dir_all(&child)
                .with_context(|| format!("remove {}", child.display()))?;
        } else {
            std::fs::remove_file(&child).with_context(|| format!("remove {}", child.display()))?;
        }
    }
    Ok(())
}

fn write_mock_terminal_script(repos_root: &Path) -> anyhow::Result<()> {
    let path = mock_terminal_script_path(repos_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(&path, MOCK_TERMINAL_SCRIPT)
        .with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms)?;
    }
    Ok(())
}

fn prepare_repos(repos_root: &Path) -> anyhow::Result<()> {
    prepare_atlas_repo(&repos_root.join("atlas"))?;
    prepare_zephyr_repo(&repos_root.join("zephyr"))?;
    Ok(())
}

fn prepare_atlas_repo(repo: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(repo.join("src"))?;
    std::fs::create_dir_all(repo.join("data"))?;
    std::fs::write(repo.join("README.md"), "# Atlas\n\nSeeded e2e workspace.\n")?;
    std::fs::write(
        repo.join("src/lib.rs"),
        "pub fn greeting() -> &'static str {\n    \"old\"\n}\n",
    )?;
    std::fs::write(
        repo.join("data/config.json"),
        "{\n  \"mode\": \"seeded\",\n  \"enabled\": true,\n  \"items\": [1, 2, 3]\n}\n",
    )?;
    std::fs::write(
        repo.join("data/events.ndjson"),
        "{\"kind\":\"alpha\",\"ok\":true}\n{\"kind\":\"beta\",\"ok\":false}\n",
    )?;

    init_git_repo(repo)?;

    std::fs::write(
        repo.join("src/lib.rs"),
        "pub fn greeting() -> &'static str {\n    \"seeded\"\n}\n",
    )?;
    Ok(())
}

fn prepare_zephyr_repo(repo: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(repo.join("docs"))?;
    std::fs::write(
        repo.join("README.md"),
        "# Zephyr\n\nClean repo for e2e coverage.\n",
    )?;
    std::fs::write(
        repo.join("docs/notes.md"),
        "No active sessions are attached to this repo.\n",
    )?;
    init_git_repo(repo)?;
    Ok(())
}

fn init_git_repo(repo: &Path) -> anyhow::Result<()> {
    run_git(repo, &["init"])?;
    run_git(repo, &["config", "user.name", "Sulion E2E"])?;
    run_git(repo, &["config", "user.email", "sulion-e2e@example.com"])?;
    run_git(repo, &["add", "."])?;
    run_git_with_dates(repo, &["commit", "-m", "Initial seeded state"])?;
    Ok(())
}

fn run_git(repo: &Path, args: &[&str]) -> anyhow::Result<()> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .with_context(|| format!("git {:?} in {}", args, repo.display()))?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "git {:?} failed in {}: {}",
        args,
        repo.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_git_with_dates(repo: &Path, args: &[&str]) -> anyhow::Result<()> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_DATE", "2025-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2025-01-01T00:00:00Z")
        .output()
        .with_context(|| format!("git {:?} in {}", args, repo.display()))?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "git {:?} failed in {}: {}",
        args,
        repo.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn create_sessions(client: &reqwest::Client, base_url: &str) -> anyhow::Result<SessionIds> {
    let claude = create_session(client, base_url, "atlas").await?;
    let codex = create_session(client, base_url, "atlas").await?;
    Ok(SessionIds { claude, codex })
}

async fn create_session(
    client: &reqwest::Client,
    base_url: &str,
    repo: &str,
) -> anyhow::Result<Uuid> {
    let response = client
        .post(format!("{base_url}/api/sessions"))
        .json(&json!({ "repo": repo }))
        .send()
        .await
        .with_context(|| format!("create session for {repo}"))?;
    if !response.status().is_success() {
        bail!(
            "create session for {repo} failed: {}",
            response.text().await.unwrap_or_default()
        );
    }
    let session: CreatedSession = response.json().await.context("decode created session")?;
    Ok(session.id)
}

async fn annotate_sessions(pool: &db::Pool, session_ids: SessionIds) -> anyhow::Result<()> {
    update_session_label(pool, session_ids.claude, "Atlas Claude").await?;
    update_session_label(pool, session_ids.codex, "Atlas Codex").await?;
    Ok(())
}

async fn update_session_label(pool: &db::Pool, id: Uuid, label: &str) -> anyhow::Result<()> {
    sqlx::query("UPDATE pty_sessions SET label = $2 WHERE id = $1")
        .bind(id)
        .bind(label)
        .execute(pool)
        .await
        .with_context(|| format!("label session {id}"))?;
    Ok(())
}

fn write_transcripts(cfg: &SeedConfig) -> anyhow::Result<()> {
    write_claude_transcript(cfg)?;
    write_codex_transcripts(cfg)?;
    Ok(())
}

fn write_claude_transcript(cfg: &SeedConfig) -> anyhow::Result<()> {
    let session_uuid = parse_uuid(CLAUDE_SESSION_UUID)?;
    let project_dir = cfg.claude_projects_dir.join(CLAUDE_PROJECT_HASH);
    std::fs::create_dir_all(&project_dir)?;
    let path = project_dir.join(format!("{session_uuid}.jsonl"));

    let records = vec![
        json!({
            "type": "user",
            "timestamp": "2026-04-20T01:00:00Z",
            "message": {
                "role": "user",
                "content": "printf 'PROMPT_TIMELINE_SENTINEL\\n'\n"
            }
        }),
        json!({
            "type": "assistant",
            "timestamp": "2026-04-20T01:00:01Z",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "text", "text": "Inspecting src/lib.rs before editing." },
                    { "type": "thinking", "thinking": "Need to confirm the old implementation first." },
                    {
                        "type": "tool_use",
                        "id": "toolu_read_1",
                        "name": "Read",
                        "input": { "file_path": "src/lib.rs" }
                    }
                ]
            }
        }),
        json!({
            "type": "user",
            "timestamp": "2026-04-20T01:00:02Z",
            "message": {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_read_1",
                        "content": "pub fn greeting() -> &'static str {\n    \"old\"\n}\n",
                        "is_error": false
                    }
                ]
            }
        }),
        json!({
            "type": "assistant",
            "timestamp": "2026-04-20T01:00:03Z",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "text", "text": "Updating the implementation and checking the diff." },
                    {
                        "type": "tool_use",
                        "id": "toolu_edit_1",
                        "name": "Edit",
                        "input": { "file_path": "src/lib.rs" }
                    },
                    {
                        "type": "tool_use",
                        "id": "toolu_bash_1",
                        "name": "Bash",
                        "input": { "command": "git diff -- src/lib.rs" }
                    }
                ]
            }
        }),
        json!({
            "type": "user",
            "timestamp": "2026-04-20T01:00:04Z",
            "message": {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_edit_1",
                        "content": "",
                        "is_error": false
                    }
                ]
            },
            "toolUseResult": {
                "filePath": "src/lib.rs",
                "oldString": "pub fn greeting() -> &'static str {\n    \"old\"\n}\n",
                "newString": "pub fn greeting() -> &'static str {\n    \"seeded\"\n}\n",
                "replaceAll": false,
                "structuredPatch": [
                    {
                        "oldString": "pub fn greeting() -> &'static str {\n    \"old\"\n}\n",
                        "newString": "pub fn greeting() -> &'static str {\n    \"seeded\"\n}\n"
                    }
                ]
            }
        }),
        json!({
            "type": "user",
            "timestamp": "2026-04-20T01:00:05Z",
            "message": {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_bash_1",
                        "content": "diff --git a/src/lib.rs b/src/lib.rs\n@@\n-    \"old\"\n+    \"seeded\"\n",
                        "is_error": false
                    }
                ]
            }
        }),
        json!({
            "type": "assistant",
            "timestamp": "2026-04-20T01:00:06Z",
            "message": {
                "role": "assistant",
                "content": [
                    {
                        "type": "text",
                        "text": "src/lib.rs now returns the seeded greeting. You can save this explanation as a reference."
                    }
                ]
            }
        }),
    ];

    std::fs::write(path, render_jsonl(&records)).context("write claude transcript")?;
    Ok(())
}

fn write_codex_transcripts(cfg: &SeedConfig) -> anyhow::Result<()> {
    let parent = parse_uuid(CODEX_PARENT_SESSION_UUID)?;
    let child = parse_uuid(CODEX_CHILD_SESSION_UUID)?;
    write_codex_rollout(&cfg.codex_sessions_dir, parent, CODEX_RICH_LINEAGE_PARENT)?;
    write_codex_rollout(&cfg.codex_sessions_dir, child, CODEX_RICH_LINEAGE_CHILD)?;
    Ok(())
}

fn write_codex_rollout(root: &Path, session_uuid: Uuid, contents: &str) -> anyhow::Result<()> {
    let day_dir = root.join("2026").join("04").join("19");
    std::fs::create_dir_all(&day_dir)?;
    let path = day_dir.join(format!("rollout-2026-04-19T01-53-43-{session_uuid}.jsonl"));
    std::fs::write(path, contents).context("write codex rollout")?;
    Ok(())
}

async fn correlate_sessions(pool: &db::Pool, session_ids: SessionIds) -> anyhow::Result<()> {
    correlate::apply(
        pool,
        &CorrelateMsg {
            pty_id: session_ids.claude,
            session_uuid: parse_uuid(CLAUDE_SESSION_UUID)?,
            agent: "claude-code".to_string(),
        },
    )
    .await
    .context("correlate claude session")?;

    correlate::apply(
        pool,
        &CorrelateMsg {
            pty_id: session_ids.codex,
            session_uuid: parse_uuid(CODEX_PARENT_SESSION_UUID)?,
            agent: "codex".to_string(),
        },
    )
    .await
    .context("correlate codex session")?;

    Ok(())
}

async fn ingest_transcripts(pool: &db::Pool, cfg: &SeedConfig) -> anyhow::Result<()> {
    let ingester = Ingester::new();
    let ingester_cfg = IngesterConfig::new(cfg.claude_projects_dir.clone())
        .with_codex_sessions_dir(cfg.codex_sessions_dir.clone());
    ingester
        .tick(pool, &ingester_cfg)
        .await
        .context("ingest seeded transcripts")?;
    Ok(())
}

async fn verify_seed(
    client: &reqwest::Client,
    base_url: &str,
    session_ids: SessionIds,
) -> anyhow::Result<()> {
    wait_for_timeline(client, base_url, session_ids.claude).await?;
    wait_for_timeline(client, base_url, session_ids.codex).await?;
    Ok(())
}

async fn wait_for_timeline(
    client: &reqwest::Client,
    base_url: &str,
    session_id: Uuid,
) -> anyhow::Result<()> {
    for _ in 0..30 {
        let response = client
            .get(format!("{base_url}/api/sessions/{session_id}/timeline"))
            .send()
            .await
            .with_context(|| format!("load timeline for {session_id}"))?;
        if response.status().is_success() {
            let body: serde_json::Value = response.json().await.context("decode timeline")?;
            let turns = body
                .get("turns")
                .and_then(|value| value.as_array())
                .map(|values| !values.is_empty())
                .unwrap_or(false);
            if turns {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err(anyhow!(
        "timed out waiting for seeded timeline for {session_id}"
    ))
}

fn parse_uuid(raw: &str) -> anyhow::Result<Uuid> {
    Uuid::parse_str(raw).with_context(|| format!("invalid uuid {raw}"))
}

fn render_jsonl(records: &[serde_json::Value]) -> String {
    let mut out = records
        .iter()
        .map(serde_json::Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    out
}
