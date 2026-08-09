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
    pub position: i32,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MetaRepoView {
    pub id: Uuid,
    pub name: String,
    pub primary_repo_name: Option<String>,
    pub position: i32,
    pub revision: i64,
    pub members: Vec<MetaRepoMemberView>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateMetaRepoInput {
    pub name: String,
    #[serde(default)]
    pub members: Vec<String>,
    #[serde(default)]
    pub primary_repo_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PatchMetaRepoInput {
    pub expected_revision: i64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub position: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct ReplaceMembersInput {
    pub expected_revision: i64,
    pub members: Vec<String>,
    #[serde(default)]
    pub primary_repo_name: Option<String>,
}

#[derive(FromRow)]
struct MetaRepoRow {
    id: Uuid,
    name: String,
    primary_repo_name: Option<String>,
    position: i32,
    revision: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct MemberRow {
    meta_repo_id: Uuid,
    repo_name: String,
    position: i32,
    exists: bool,
}

pub async fn list(pool: &crate::db::Pool) -> Result<Vec<MetaRepoView>, MetaRepoError> {
    let groups: Vec<MetaRepoRow> = sqlx::query_as(
        "SELECT id, name, primary_repo_name, position, revision, created_at, updated_at \
           FROM meta_repos \
          WHERE deleted_at IS NULL \
          ORDER BY position, LOWER(name), id",
    )
    .fetch_all(pool)
    .await?;
    let members: Vec<MemberRow> = sqlx::query_as(
        "SELECT m.meta_repo_id, m.repo_name, m.position, \
                COALESCE(r.exists, FALSE) AS exists \
           FROM meta_repo_members m \
           JOIN meta_repos g ON g.id = m.meta_repo_id AND g.deleted_at IS NULL \
           LEFT JOIN repo_runtime_state r ON r.repo_name = m.repo_name \
          ORDER BY m.meta_repo_id, m.position",
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
                position: member.position,
                exists: member.exists,
            });
    }

    Ok(groups
        .into_iter()
        .map(|group| MetaRepoView {
            id: group.id,
            name: group.name,
            primary_repo_name: group.primary_repo_name,
            position: group.position,
            revision: group.revision,
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
    input: CreateMetaRepoInput,
) -> Result<MetaRepoView, MetaRepoError> {
    let name = validate_name(&input.name)?;
    let (members, primary_repo_name) = normalize_members(input.members, input.primary_repo_name)?;
    let mut tx = pool.begin().await?;
    ensure_name_available(&mut tx, &name, None).await?;
    ensure_repos_exist(&mut tx, &members).await?;
    ensure_members_available(&mut tx, &members, None).await?;
    let position: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM meta_repos WHERE deleted_at IS NULL",
    )
    .fetch_one(&mut *tx)
    .await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO meta_repos (id, name, primary_repo_name, position) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(&name)
    .bind(primary_repo_name.as_deref())
    .bind(position)
    .execute(&mut *tx)
    .await?;
    insert_members(&mut tx, id, &members).await?;
    tx.commit().await?;
    get(pool, id).await
}

pub async fn patch(
    pool: &crate::db::Pool,
    id: Uuid,
    input: PatchMetaRepoInput,
) -> Result<MetaRepoView, MetaRepoError> {
    if input.name.is_none() && input.position.is_none() {
        return Err(MetaRepoError::BadRequest("no changes supplied".into()));
    }
    let mut tx = pool.begin().await?;
    let current: Option<(String, i32)> = sqlx::query_as(
        "SELECT name, position FROM meta_repos WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((current_name, current_position)) = current else {
        return Err(MetaRepoError::NotFound);
    };
    let name = match input.name {
        Some(name) => validate_name(&name)?,
        None => current_name,
    };
    let position = input.position.unwrap_or(current_position);
    if position < 0 {
        return Err(MetaRepoError::BadRequest(
            "position must not be negative".into(),
        ));
    }
    ensure_name_available(&mut tx, &name, Some(id)).await?;
    let updated = sqlx::query(
        "UPDATE meta_repos \
            SET name = $2, position = $3, revision = revision + 1, updated_at = NOW() \
          WHERE id = $1 AND deleted_at IS NULL AND revision = $4",
    )
    .bind(id)
    .bind(name)
    .bind(position)
    .bind(input.expected_revision)
    .execute(&mut *tx)
    .await?;
    ensure_revision_updated(updated.rows_affected(), input.expected_revision)?;
    tx.commit().await?;
    get(pool, id).await
}

pub async fn replace_members(
    pool: &crate::db::Pool,
    id: Uuid,
    input: ReplaceMembersInput,
) -> Result<MetaRepoView, MetaRepoError> {
    let (members, primary_repo_name) = normalize_members(input.members, input.primary_repo_name)?;
    let mut tx = pool.begin().await?;
    let current_revision: Option<i64> = sqlx::query_scalar(
        "SELECT revision FROM meta_repos WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(current_revision) = current_revision else {
        return Err(MetaRepoError::NotFound);
    };
    if current_revision != input.expected_revision {
        return Err(stale_revision(input.expected_revision));
    }
    ensure_repos_exist(&mut tx, &members).await?;
    ensure_members_available(&mut tx, &members, Some(id)).await?;
    sqlx::query("DELETE FROM meta_repo_members WHERE meta_repo_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    insert_members(&mut tx, id, &members).await?;
    sqlx::query(
        "UPDATE meta_repos \
            SET primary_repo_name = $2, revision = revision + 1, updated_at = NOW() \
          WHERE id = $1",
    )
    .bind(id)
    .bind(primary_repo_name.as_deref())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    get(pool, id).await
}

pub async fn delete(pool: &crate::db::Pool, id: Uuid) -> Result<(), MetaRepoError> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM meta_repo_members WHERE meta_repo_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let deleted = sqlx::query(
        "UPDATE meta_repos \
            SET deleted_at = NOW(), primary_repo_name = NULL, revision = revision + 1, \
                updated_at = NOW() \
          WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    if deleted.rows_affected() == 0 {
        return Err(MetaRepoError::NotFound);
    }
    tx.commit().await?;
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
            SET primary_repo_name = $2, revision = revision + 1, updated_at = NOW() \
          WHERE primary_repo_name = $1 AND deleted_at IS NULL",
    )
    .bind(old_name)
    .bind(new_name)
    .execute(&mut **tx)
    .await?;
    sqlx::query("UPDATE pty_session_repos SET repo_name = $2 WHERE repo_name = $1")
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
        "SELECT g.id, g.primary_repo_name = $1 \
           FROM meta_repo_members m \
           JOIN meta_repos g ON g.id = m.meta_repo_id AND g.deleted_at IS NULL \
          WHERE m.repo_name = $1",
    )
    .bind(repo_name)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((group_id, was_primary)) = group else {
        return Ok(());
    };
    sqlx::query("DELETE FROM meta_repo_members WHERE meta_repo_id = $1 AND repo_name = $2")
        .bind(group_id)
        .bind(repo_name)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "WITH ordered AS ( \
             SELECT repo_name, ROW_NUMBER() OVER (ORDER BY position, repo_name) - 1 AS next_position \
               FROM meta_repo_members WHERE meta_repo_id = $1 \
         ) \
         UPDATE meta_repo_members m SET position = ordered.next_position::INTEGER \
           FROM ordered \
          WHERE m.meta_repo_id = $1 AND m.repo_name = ordered.repo_name",
    )
    .bind(group_id)
    .execute(&mut **tx)
    .await?;
    let next_primary: Option<String> = if was_primary {
        sqlx::query_scalar(
            "SELECT repo_name FROM meta_repo_members \
              WHERE meta_repo_id = $1 ORDER BY position LIMIT 1",
        )
        .bind(group_id)
        .fetch_optional(&mut **tx)
        .await?
    } else {
        None
    };
    sqlx::query(
        "UPDATE meta_repos \
            SET primary_repo_name = CASE WHEN $2 THEN $3 ELSE primary_repo_name END, \
                revision = revision + 1, updated_at = NOW() \
          WHERE id = $1",
    )
    .bind(group_id)
    .bind(was_primary)
    .bind(next_primary)
    .execute(&mut **tx)
    .await?;
    Ok(())
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

fn normalize_members(
    members: Vec<String>,
    primary_repo_name: Option<String>,
) -> Result<(Vec<String>, Option<String>), MetaRepoError> {
    let mut normalized = Vec::with_capacity(members.len());
    let mut seen = HashSet::new();
    for raw in members {
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
        normalized.push(repo);
    }
    let primary = match primary_repo_name {
        Some(raw) => {
            let repo = raw.trim().to_string();
            if !normalized.iter().any(|member| member == &repo) {
                return Err(MetaRepoError::BadRequest(
                    "primary repository must be a member".into(),
                ));
            }
            Some(repo)
        }
        None => normalized.first().cloned(),
    };
    Ok((normalized, primary))
}

async fn ensure_name_available(
    tx: &mut Transaction<'_, Postgres>,
    name: &str,
    except_id: Option<Uuid>,
) -> Result<(), MetaRepoError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM meta_repos \
              WHERE deleted_at IS NULL AND LOWER(name) = LOWER($1) \
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
    if members.is_empty() {
        return Ok(());
    }
    let existing: Vec<String> = sqlx::query_scalar(
        "SELECT repo_name FROM repo_runtime_state \
          WHERE exists = TRUE AND repo_name = ANY($1)",
    )
    .bind(members)
    .fetch_all(&mut **tx)
    .await?;
    let existing: HashSet<String> = existing.into_iter().collect();
    let missing: Vec<&str> = members
        .iter()
        .filter(|repo| !existing.contains(*repo))
        .map(String::as_str)
        .collect();
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
    if members.is_empty() {
        return Ok(());
    }
    let assigned: Vec<String> = sqlx::query_scalar(
        "SELECT repo_name FROM meta_repo_members \
          WHERE repo_name = ANY($1) \
            AND ($2::UUID IS NULL OR meta_repo_id <> $2) \
          ORDER BY repo_name",
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
    for (position, repo_name) in members.iter().enumerate() {
        sqlx::query(
            "INSERT INTO meta_repo_members (meta_repo_id, repo_name, position) \
             VALUES ($1, $2, $3)",
        )
        .bind(group_id)
        .bind(repo_name)
        .bind(position as i32)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

fn ensure_revision_updated(rows: u64, expected: i64) -> Result<(), MetaRepoError> {
    if rows == 0 {
        return Err(stale_revision(expected));
    }
    Ok(())
}

fn stale_revision(expected: i64) -> MetaRepoError {
    MetaRepoError::BadRequest(format!(
        "meta-repository changed since revision {expected}; reload and retry"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_member_becomes_primary() {
        let (members, primary) =
            normalize_members(vec!["alpha".into(), "beta".into()], None).unwrap();
        assert_eq!(members, vec!["alpha", "beta"]);
        assert_eq!(primary.as_deref(), Some("alpha"));
    }

    #[test]
    fn primary_must_be_a_member() {
        let err = normalize_members(vec!["alpha".into()], Some("beta".into())).unwrap_err();
        assert!(err.to_string().contains("must be a member"));
    }
}
