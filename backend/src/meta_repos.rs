use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

const MAX_NAME_CHARS: usize = 80;

#[derive(Debug, thiserror::Error)]
pub enum MetaRepoError {
    #[error("meta-repository not found")]
    NotFound,
    #[error("{0}")]
    BadRequest(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MetaRepoMemberView {
    pub repo_name: String,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MetaRepoView {
    pub id: Uuid,
    pub name: String,
    pub primary_repo_name: String,
    pub members: Vec<MetaRepoMemberView>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct SaveMetaRepoInput {
    pub name: String,
    pub members: Vec<String>,
    pub primary_repo_name: String,
}

#[derive(FromRow)]
struct MetaRepoRow {
    id: Uuid,
    name: String,
    primary_repo_name: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct MemberRow {
    meta_repo_id: Uuid,
    repo_name: String,
    exists: bool,
}

pub async fn list(pool: &crate::db::Pool) -> Result<Vec<MetaRepoView>, MetaRepoError> {
    let groups: Vec<MetaRepoRow> = sqlx::query_as(
        "SELECT id, name, primary_repo_name, created_at, updated_at \
           FROM meta_repos \
          ORDER BY LOWER(name), id",
    )
    .fetch_all(pool)
    .await?;
    let members: Vec<MemberRow> = sqlx::query_as(
        "SELECT m.meta_repo_id, m.repo_name, COALESCE(r.exists, FALSE) AS exists \
           FROM meta_repo_members m \
           LEFT JOIN repo_runtime_state r ON r.repo_name = m.repo_name \
          ORDER BY m.meta_repo_id, LOWER(m.repo_name), m.repo_name",
    )
    .fetch_all(pool)
    .await?;

    let mut by_group: HashMap<Uuid, Vec<MetaRepoMemberView>> = HashMap::new();
    for member in members {
        by_group
            .entry(member.meta_repo_id)
            .or_default()
            .push(MetaRepoMemberView {
                repo_name: member.repo_name,
                exists: member.exists,
            });
    }

    Ok(groups
        .into_iter()
        .map(|group| MetaRepoView {
            id: group.id,
            name: group.name,
            primary_repo_name: group.primary_repo_name,
            members: by_group.remove(&group.id).unwrap_or_default(),
            created_at: group.created_at,
            updated_at: group.updated_at,
        })
        .collect())
}

pub async fn get(pool: &crate::db::Pool, id: Uuid) -> Result<MetaRepoView, MetaRepoError> {
    list(pool)
        .await?
        .into_iter()
        .find(|group| group.id == id)
        .ok_or(MetaRepoError::NotFound)
}

pub async fn create(
    pool: &crate::db::Pool,
    input: SaveMetaRepoInput,
) -> Result<MetaRepoView, MetaRepoError> {
    let (name, members, primary_repo_name) = normalize_input(input)?;
    let mut tx = pool.begin().await?;
    ensure_name_available(&mut tx, &name, None).await?;
    ensure_repos_exist(&mut tx, &members).await?;
    ensure_members_available(&mut tx, &members, None).await?;

    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO meta_repos (id, name, primary_repo_name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(&name)
        .bind(&primary_repo_name)
        .execute(&mut *tx)
        .await?;
    insert_members(&mut tx, id, &members).await?;
    tx.commit().await?;
    get(pool, id).await
}

pub async fn update(
    pool: &crate::db::Pool,
    id: Uuid,
    input: SaveMetaRepoInput,
) -> Result<MetaRepoView, MetaRepoError> {
    let (name, members, primary_repo_name) = normalize_input(input)?;
    let mut tx = pool.begin().await?;
    let existing_id: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM meta_repos WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    if existing_id.is_none() {
        return Err(MetaRepoError::NotFound);
    }
    ensure_name_available(&mut tx, &name, Some(id)).await?;
    ensure_repos_exist(&mut tx, &members).await?;
    ensure_members_available(&mut tx, &members, Some(id)).await?;

    sqlx::query("DELETE FROM meta_repo_members WHERE meta_repo_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    insert_members(&mut tx, id, &members).await?;
    sqlx::query(
        "UPDATE meta_repos \
            SET name = $2, primary_repo_name = $3, updated_at = NOW() \
          WHERE id = $1",
    )
    .bind(id)
    .bind(name)
    .bind(primary_repo_name)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    get(pool, id).await
}

pub async fn delete(pool: &crate::db::Pool, id: Uuid) -> Result<(), MetaRepoError> {
    let deleted = sqlx::query("DELETE FROM meta_repos WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    if deleted.rows_affected() == 0 {
        return Err(MetaRepoError::NotFound);
    }
    Ok(())
}

pub async fn rename_repo_references(
    tx: &mut Transaction<'_, Postgres>,
    old_name: &str,
    new_name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE meta_repo_members SET repo_name = $2 WHERE repo_name = $1")
        .bind(old_name)
        .bind(new_name)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "UPDATE meta_repos \
            SET primary_repo_name = $2, updated_at = NOW() \
          WHERE primary_repo_name = $1",
    )
    .bind(old_name)
    .bind(new_name)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn remove_repo_membership(
    tx: &mut Transaction<'_, Postgres>,
    repo_name: &str,
) -> Result<(), sqlx::Error> {
    let group: Option<(Uuid, bool)> = sqlx::query_as(
        "SELECT m.meta_repo_id, g.primary_repo_name = $1 \
           FROM meta_repo_members m \
           JOIN meta_repos g ON g.id = m.meta_repo_id \
          WHERE m.repo_name = $1",
    )
    .bind(repo_name)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((group_id, was_primary)) = group else {
        return Ok(());
    };
    sqlx::query("DELETE FROM meta_repo_members WHERE repo_name = $1")
        .bind(repo_name)
        .execute(&mut **tx)
        .await?;
    let next_primary: Option<String> = sqlx::query_scalar(
        "SELECT repo_name FROM meta_repo_members \
          WHERE meta_repo_id = $1 \
          ORDER BY LOWER(repo_name), repo_name LIMIT 1",
    )
    .bind(group_id)
    .fetch_optional(&mut **tx)
    .await?;
    match next_primary {
        Some(next_primary) if was_primary => {
            sqlx::query(
                "UPDATE meta_repos \
                    SET primary_repo_name = $2, updated_at = NOW() \
                  WHERE id = $1",
            )
            .bind(group_id)
            .bind(next_primary)
            .execute(&mut **tx)
            .await?;
        }
        Some(_) => {}
        None => {
            sqlx::query("DELETE FROM meta_repos WHERE id = $1")
                .bind(group_id)
                .execute(&mut **tx)
                .await?;
        }
    }
    Ok(())
}

fn normalize_input(
    input: SaveMetaRepoInput,
) -> Result<(String, Vec<String>, String), MetaRepoError> {
    let name = validate_name(&input.name)?;
    let mut members = Vec::with_capacity(input.members.len());
    let mut seen = HashSet::new();
    for raw in input.members {
        let repo = raw.trim().to_string();
        if !crate::workspace::is_valid_repo_name(&repo) {
            return Err(MetaRepoError::BadRequest(format!(
                "invalid repository name: {repo}"
            )));
        }
        if !seen.insert(repo.clone()) {
            return Err(MetaRepoError::BadRequest(format!(
                "duplicate repository: {repo}"
            )));
        }
        members.push(repo);
    }
    if members.is_empty() {
        return Err(MetaRepoError::BadRequest(
            "meta-repository requires at least one repository".into(),
        ));
    }
    members.sort_by_key(|repo| repo.to_lowercase());
    let primary_repo_name = input.primary_repo_name.trim().to_string();
    if !members.iter().any(|member| member == &primary_repo_name) {
        return Err(MetaRepoError::BadRequest(
            "primary repository must be a member".into(),
        ));
    }
    Ok((name, members, primary_repo_name))
}

fn validate_name(raw: &str) -> Result<String, MetaRepoError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(MetaRepoError::BadRequest(
            "meta-repository name must not be empty".into(),
        ));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(MetaRepoError::BadRequest(format!(
            "meta-repository name must be at most {MAX_NAME_CHARS} characters"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(MetaRepoError::BadRequest(
            "meta-repository name must not contain control characters".into(),
        ));
    }
    Ok(name.to_string())
}

async fn ensure_name_available(
    tx: &mut Transaction<'_, Postgres>,
    name: &str,
    except_id: Option<Uuid>,
) -> Result<(), MetaRepoError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM meta_repos \
              WHERE LOWER(name) = LOWER($1) \
                AND ($2::UUID IS NULL OR id <> $2) \
         )",
    )
    .bind(name)
    .bind(except_id)
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        return Err(MetaRepoError::BadRequest(format!(
            "meta-repository name is already in use: {name}"
        )));
    }
    Ok(())
}

async fn ensure_repos_exist(
    tx: &mut Transaction<'_, Postgres>,
    members: &[String],
) -> Result<(), MetaRepoError> {
    let existing: Vec<String> = sqlx::query_scalar(
        "SELECT repo_name FROM repo_runtime_state \
          WHERE exists = TRUE AND repo_name = ANY($1)",
    )
    .bind(members)
    .fetch_all(&mut **tx)
    .await?;
    let existing: HashSet<String> = existing.into_iter().collect();
    let missing = members
        .iter()
        .filter(|repo| !existing.contains(*repo))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(MetaRepoError::BadRequest(format!(
            "repository not found: {}",
            missing.join(", ")
        )));
    }
    Ok(())
}

async fn ensure_members_available(
    tx: &mut Transaction<'_, Postgres>,
    members: &[String],
    except_id: Option<Uuid>,
) -> Result<(), MetaRepoError> {
    let assigned: Vec<String> = sqlx::query_scalar(
        "SELECT repo_name FROM meta_repo_members \
          WHERE repo_name = ANY($1) \
            AND ($2::UUID IS NULL OR meta_repo_id <> $2) \
          ORDER BY LOWER(repo_name), repo_name",
    )
    .bind(members)
    .bind(except_id)
    .fetch_all(&mut **tx)
    .await?;
    if !assigned.is_empty() {
        return Err(MetaRepoError::BadRequest(format!(
            "repository already belongs to a meta-repository: {}",
            assigned.join(", ")
        )));
    }
    Ok(())
}

async fn insert_members(
    tx: &mut Transaction<'_, Postgres>,
    group_id: Uuid,
    members: &[String],
) -> Result<(), sqlx::Error> {
    for repo_name in members {
        sqlx::query("INSERT INTO meta_repo_members (meta_repo_id, repo_name) VALUES ($1, $2)")
            .bind(group_id)
            .bind(repo_name)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}
