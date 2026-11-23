use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use std::collections::{BTreeMap, HashSet};
use uuid::Uuid;

use super::{required_text, MAX_TITLE_CHARS};
use crate::db::Pool;
use serde_json::{json, Value};

pub fn add_plan_urls(value: &mut Value, base: &url::Url) {
    let Some(id) = value["id"].as_str() else {
        return;
    };
    let mut url = base.clone();
    url.query_pairs_mut().append_pair("plan", id);
    value["url"] = json!(url.as_str());
    if let Some(phases) = value["phases"].as_array_mut() {
        for phase in phases {
            if let Some(id) = phase["id"].as_str() {
                let mut phase_url = url.clone();
                phase_url.query_pairs_mut().append_pair("phase", id);
                phase["url"] = json!(phase_url.as_str());
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, sqlx::FromRow)]
pub struct Diagram {
    pub workspace_id: Uuid,
    pub room_id: String,
    pub title: String,
}

impl Diagram {
    pub fn result(&self) -> serde_json::Value {
        serde_json::json!({
            "workspace_id": self.workspace_id, "room_id": self.room_id,
            "title": self.title, "url": self.url()
        })
    }

    pub fn url(&self) -> String {
        format!("https://arconis.ahara.io/#room={}", self.room_id)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DiagramPhaseLink {
    pub phase_id: Uuid,
    pub object_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagramAttachment {
    #[serde(flatten)]
    pub diagram: Diagram,
    pub url: String,
    pub phase_links: Vec<DiagramPhaseLinkView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagramPhaseLinkView {
    #[serde(flatten)]
    pub link: DiagramPhaseLink,
    pub url: String,
}

pub async fn register(
    pool: &Pool,
    repo: &str,
    workspace_id: Uuid,
    room_id: &str,
    title: &str,
) -> anyhow::Result<Diagram> {
    validate_repo(repo)?;
    anyhow::ensure!(
        room_id.len() == 48
            && room_id
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
        "expected an ordinary Arconis room ID"
    );
    let title = required_text(title, "diagram title", MAX_TITLE_CHARS)?;
    let result = sqlx::query_as(
        "INSERT INTO repo_arconis_diagrams (repo_name, workspace_id, room_id, title) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (repo_name, workspace_id) DO UPDATE SET title = EXCLUDED.title \
         WHERE repo_arconis_diagrams.room_id = EXCLUDED.room_id \
         RETURNING workspace_id, room_id, title",
    )
    .bind(repo)
    .bind(workspace_id)
    .bind(room_id)
    .bind(title)
    .fetch_optional(pool)
    .await?;
    result.ok_or_else(|| {
        anyhow::anyhow!("this workspace is already registered with a different room")
    })
}

pub async fn list(pool: &Pool, repo: &str) -> anyhow::Result<Vec<Diagram>> {
    validate_repo(repo)?;
    Ok(sqlx::query_as(
        "SELECT workspace_id, room_id, title FROM repo_arconis_diagrams \
         WHERE repo_name = $1 ORDER BY title, workspace_id",
    )
    .bind(repo)
    .fetch_all(pool)
    .await?)
}

pub(super) async fn attachments(
    connection: &mut PgConnection,
    repo: &str,
    links: &BTreeMap<Uuid, Vec<DiagramPhaseLink>>,
) -> anyhow::Result<Vec<DiagramAttachment>> {
    if links.is_empty() {
        return Ok(Vec::new());
    }
    let diagrams: Vec<Diagram> = sqlx::query_as(
        "SELECT workspace_id, room_id, title FROM repo_arconis_diagrams \
         WHERE repo_name = $1 AND workspace_id = ANY($2) ORDER BY title, workspace_id",
    )
    .bind(repo)
    .bind(links.keys().copied().collect::<Vec<_>>())
    .fetch_all(connection)
    .await?;
    Ok(diagrams
        .into_iter()
        .map(|diagram| {
            let url = diagram.url();
            let phase_links = links[&diagram.workspace_id]
                .iter()
                .cloned()
                .map(|link| {
                    let url = format!("{url}&item={}", link.object_id);
                    DiagramPhaseLinkView { link, url }
                })
                .collect();
            DiagramAttachment {
                url,
                phase_links,
                diagram,
            }
        })
        .collect())
}

pub(super) async fn validate_attachment(
    connection: &mut PgConnection,
    repo: &str,
    plan_id: Uuid,
    workspace_id: Uuid,
    links: &[DiagramPhaseLink],
) -> anyhow::Result<()> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM repo_arconis_diagrams WHERE repo_name = $1 AND workspace_id = $2)",
    )
    .bind(repo)
    .bind(workspace_id)
    .fetch_one(&mut *connection)
    .await?;
    anyhow::ensure!(
        exists,
        "register the Arconis workspace in this plan's repo first"
    );

    let mut pairs = HashSet::new();
    for link in links {
        let mut bytes = link.object_id.bytes();
        anyhow::ensure!(
            bytes
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
                && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')),
            "invalid Arconis object ID: {}",
            link.object_id
        );
        anyhow::ensure!(
            pairs.insert((link.phase_id, &link.object_id)),
            "duplicate Arconis phase and object link"
        );
    }
    let phases: HashSet<_> = links.iter().map(|link| link.phase_id).collect();
    let found: i64 =
        sqlx::query_scalar("SELECT count(*) FROM plan_phases WHERE plan_id = $1 AND id = ANY($2)")
            .bind(plan_id)
            .bind(phases.iter().copied().collect::<Vec<_>>())
            .fetch_one(connection)
            .await?;
    anyhow::ensure!(
        found as usize == phases.len(),
        "diagram link phase is outside this plan"
    );
    Ok(())
}

fn validate_repo(repo: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        crate::workspace::is_valid_repo_name(repo),
        "invalid repo name"
    );
    Ok(())
}
