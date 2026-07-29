//! Portfolio metrics for the monitor/metrics surfaces.
//!
//! Everything here is derived from data Sulion already stores — no new
//! agent-side telemetry:
//! - token rollups and daily series from `agent_session_usage` +
//!   `agent_usage_daily` (fresh = total - cached reads),
//! - git activity from `git log` per registered repo (cached in-process),
//!   with agent/human attribution via `Co-Authored-By` trailers,
//! - churn hotspots from `timeline_file_touches` write re-touches,
//! - agile flow (CFD, burndown, throughput, cycle time) replayed from the
//!   append-only `plan_events` history, weighted by optional phase size.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};
use serde::Serialize;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::db::Pool;

const DAILY_WINDOW_DAYS: i64 = 14;
const GIT_CACHE_TTL: Duration = Duration::from_secs(120);
const CHURN_MIN_TURNS: i64 = 3;

// ─── Response shape ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct MetricsResponse {
    pub generated_at: DateTime<Utc>,
    pub usage: UsageMetrics,
    pub git: Vec<RepoGitActivity>,
    pub churn: Vec<ChurnHotspot>,
    pub flow: FlowMetrics,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct UsageWindow {
    pub fresh_tokens: i64,
    pub cached_tokens: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageMetrics {
    pub all_time: UsageWindow,
    pub today: UsageWindow,
    pub last_7d: UsageWindow,
    pub per_repo: Vec<RepoUsage>,
    pub daily: Vec<UsageDay>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoUsage {
    pub repo: String,
    pub all_time: UsageWindow,
    pub today: UsageWindow,
    pub last_7d: UsageWindow,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageDay {
    pub day: NaiveDate,
    pub fresh_tokens: i64,
    pub cached_tokens: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoGitActivity {
    pub repo: String,
    pub commits_24h: i64,
    pub commits_7d: i64,
    pub insertions_24h: i64,
    pub deletions_24h: i64,
    pub insertions_7d: i64,
    pub deletions_7d: i64,
    pub agent_commits_7d: i64,
    pub human_commits_7d: i64,
    pub last_commit_at: Option<DateTime<Utc>>,
    pub daily: Vec<GitDay>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitDay {
    pub day: NaiveDate,
    pub commits: i64,
    pub insertions: i64,
    pub deletions: i64,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ChurnHotspot {
    pub repo: String,
    pub path: String,
    pub write_turns: i64,
    pub sessions: i64,
    pub last_write_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FlowMetrics {
    /// Phases currently in_progress / blocked (unweighted counts).
    pub wip: i64,
    pub blocked: i64,
    /// Completed weight per ISO week (Monday start), oldest first.
    pub throughput_weeks: Vec<ThroughputWeek>,
    /// Median hours from first in_progress to completed, last 30 days.
    pub cycle_time_hours_p50: Option<f64>,
    pub cfd: Vec<CfdDay>,
    pub burndowns: Vec<PlanBurndown>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ThroughputWeek {
    pub week_start: NaiveDate,
    pub completed_weight: i64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CfdDay {
    pub day: NaiveDate,
    pub pending: i64,
    pub in_progress: i64,
    pub blocked: i64,
    pub completed: i64,
    pub skipped: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanBurndown {
    pub plan_id: Uuid,
    pub repo: String,
    pub title: String,
    pub total_weight: i64,
    pub days: Vec<BurndownDay>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BurndownDay {
    pub day: NaiveDate,
    /// Weight not yet completed/skipped as of end of day.
    pub remaining_weight: i64,
    /// Total scope as of end of day — a rising line is scope creep.
    pub total_weight: i64,
}

// ─── Entry point ────────────────────────────────────────────────────

pub async fn portfolio_metrics(pool: &Pool) -> anyhow::Result<MetricsResponse> {
    let usage = usage_metrics(pool).await?;
    let git = git_activity(pool).await?;
    let churn = churn_hotspots(pool).await?;
    let flow = flow_metrics(pool).await?;
    Ok(MetricsResponse {
        generated_at: Utc::now(),
        usage,
        git,
        churn,
        flow,
    })
}

// ─── Token usage ────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct UsageRow {
    repo: Option<String>,
    all_total: i64,
    all_cached: i64,
    today_total: i64,
    today_cached: i64,
    week_total: i64,
    week_cached: i64,
}

fn window(total: i64, cached: i64) -> UsageWindow {
    UsageWindow {
        fresh_tokens: (total - cached).max(0),
        cached_tokens: cached,
        total_tokens: total,
    }
}

async fn usage_metrics(pool: &Pool) -> anyhow::Result<UsageMetrics> {
    // Repo attribution, most direct first:
    // 1. the session's correlated PTY (SessionStart hook),
    // 2. a correlated PTY anywhere up the compaction-parent lineage —
    //    continuation sessions get fresh uuids and no new hook,
    // 3. any PTY whose current pointer names this session,
    // 4. the transcript project hash matched against repo paths
    //    (covers sessions that never correlated at all).
    let rows: Vec<UsageRow> = sqlx::query_as(
        "WITH RECURSIVE lineage AS ( \
            SELECT cs.session_uuid AS origin, cs.pty_session_id, \
                   cs.parent_session_uuid, 0 AS depth \
            FROM claude_sessions cs \
          UNION ALL \
            SELECT l.origin, parent.pty_session_id, parent.parent_session_uuid, \
                   l.depth + 1 \
            FROM lineage l \
            JOIN claude_sessions parent \
              ON parent.session_uuid = l.parent_session_uuid \
            WHERE l.pty_session_id IS NULL AND l.depth < 16 \
         ), \
         lineage_pty AS ( \
            SELECT DISTINCT ON (origin) origin, pty_session_id \
            FROM lineage WHERE pty_session_id IS NOT NULL \
            ORDER BY origin, depth \
         ), \
         base AS ( \
            SELECT u.session_uuid, \
                   COALESCE(p_direct.repo, p_reverse.repo, hash_repo.repo_name) AS repo, \
                   u.total_tokens, u.cached_input_tokens, \
                   today_base.total_tokens AS today_base_total, \
                   today_base.cached_input_tokens AS today_base_cached, \
                   week_base.total_tokens AS week_base_total, \
                   week_base.cached_input_tokens AS week_base_cached \
            FROM agent_session_usage u \
            LEFT JOIN claude_sessions cs ON cs.session_uuid = u.session_uuid \
            LEFT JOIN lineage_pty lp ON lp.origin = u.session_uuid \
            LEFT JOIN pty_sessions p_direct ON p_direct.id = lp.pty_session_id \
            LEFT JOIN LATERAL ( \
                SELECT pr.repo FROM pty_sessions pr \
                 WHERE pr.current_session_uuid = u.session_uuid \
                 LIMIT 1) p_reverse ON TRUE \
            LEFT JOIN LATERAL ( \
                SELECT r.repo_name FROM repo_runtime_state r \
                 WHERE cs.project_hash IS NOT NULL \
                   AND regexp_replace(r.path, '[^A-Za-z0-9]', '-', 'g') = cs.project_hash \
                 LIMIT 1) hash_repo ON TRUE \
            LEFT JOIN LATERAL ( \
                SELECT total_tokens, cached_input_tokens FROM agent_usage_daily d \
                 WHERE d.session_uuid = u.session_uuid AND d.day < CURRENT_DATE \
                 ORDER BY d.day DESC LIMIT 1) today_base ON TRUE \
            LEFT JOIN LATERAL ( \
                SELECT total_tokens, cached_input_tokens FROM agent_usage_daily d \
                 WHERE d.session_uuid = u.session_uuid AND d.day < CURRENT_DATE - 6 \
                 ORDER BY d.day DESC LIMIT 1) week_base ON TRUE \
         ) \
         SELECT repo, \
            COALESCE(SUM(total_tokens), 0)::BIGINT AS all_total, \
            COALESCE(SUM(cached_input_tokens), 0)::BIGINT AS all_cached, \
            COALESCE(SUM(GREATEST(total_tokens - COALESCE(today_base_total, 0), 0)), 0)::BIGINT \
                AS today_total, \
            COALESCE(SUM(GREATEST(cached_input_tokens - COALESCE(today_base_cached, 0), 0)), 0)::BIGINT \
                AS today_cached, \
            COALESCE(SUM(GREATEST(total_tokens - COALESCE(week_base_total, 0), 0)), 0)::BIGINT \
                AS week_total, \
            COALESCE(SUM(GREATEST(cached_input_tokens - COALESCE(week_base_cached, 0), 0)), 0)::BIGINT \
                AS week_cached \
         FROM base GROUP BY repo ORDER BY all_total DESC",
    )
    .fetch_all(pool)
    .await?;

    let mut all = (0i64, 0i64);
    let mut today = (0i64, 0i64);
    let mut week = (0i64, 0i64);
    let mut per_repo = Vec::with_capacity(rows.len());
    for row in rows {
        all.0 += row.all_total;
        all.1 += row.all_cached;
        today.0 += row.today_total;
        today.1 += row.today_cached;
        week.0 += row.week_total;
        week.1 += row.week_cached;
        per_repo.push(RepoUsage {
            repo: row.repo.unwrap_or_else(|| "(unattributed)".to_string()),
            all_time: window(row.all_total, row.all_cached),
            today: window(row.today_total, row.today_cached),
            last_7d: window(row.week_total, row.week_cached),
        });
    }

    #[derive(sqlx::FromRow)]
    struct DailyRow {
        day: NaiveDate,
        total: i64,
        cached: i64,
    }
    let daily: Vec<DailyRow> = sqlx::query_as(
        "WITH seq AS ( \
            SELECT session_uuid, day, total_tokens, cached_input_tokens, \
                   LAG(total_tokens) OVER w AS prev_total, \
                   LAG(cached_input_tokens) OVER w AS prev_cached \
            FROM agent_usage_daily \
            WINDOW w AS (PARTITION BY session_uuid ORDER BY day) \
         ) \
         SELECT day, \
            COALESCE(SUM(GREATEST(total_tokens - COALESCE(prev_total, 0), 0)), 0)::BIGINT AS total, \
            COALESCE(SUM(GREATEST(cached_input_tokens - COALESCE(prev_cached, 0), 0)), 0)::BIGINT \
                AS cached \
         FROM seq WHERE day > CURRENT_DATE - $1::INT \
         GROUP BY day ORDER BY day",
    )
    .bind(DAILY_WINDOW_DAYS as i32)
    .fetch_all(pool)
    .await?;

    Ok(UsageMetrics {
        all_time: window(all.0, all.1),
        today: window(today.0, today.1),
        last_7d: window(week.0, week.1),
        per_repo,
        daily: daily
            .into_iter()
            .map(|row| UsageDay {
                day: row.day,
                fresh_tokens: (row.total - row.cached).max(0),
                cached_tokens: row.cached,
                total_tokens: row.total,
            })
            .collect(),
    })
}

// ─── Git activity ───────────────────────────────────────────────────

struct GitCache {
    at: Instant,
    data: Vec<RepoGitActivity>,
}

static GIT_CACHE: Mutex<Option<GitCache>> = Mutex::const_new(None);

async fn git_activity(pool: &Pool) -> anyhow::Result<Vec<RepoGitActivity>> {
    let mut cache = GIT_CACHE.lock().await;
    if let Some(cached) = cache.as_ref() {
        if cached.at.elapsed() < GIT_CACHE_TTL {
            return Ok(cached.data.clone());
        }
    }
    // repo_runtime_state is the live registry (the 0001 `repos` table is
    // legacy and stale in deployed databases).
    let repos: Vec<(String, String)> = sqlx::query_as(
        "SELECT repo_name, path FROM repo_runtime_state \
          WHERE \"exists\" ORDER BY repo_name",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(repos.len());
    for (name, path) in repos {
        match scan_repo_git(&name, Path::new(&path)).await {
            Ok(activity) => out.push(activity),
            Err(err) => {
                // Keep the repo visible with zeros and say why — a silent
                // skip reads as "no activity anywhere".
                tracing::warn!(repo = %name, %err, "git activity scan failed");
                out.push(empty_activity(&name));
            }
        }
    }
    *cache = Some(GitCache {
        at: Instant::now(),
        data: out.clone(),
    });
    Ok(out)
}

struct CommitStat {
    epoch: i64,
    insertions: i64,
    deletions: i64,
    agent: bool,
}

fn empty_activity(name: &str) -> RepoGitActivity {
    RepoGitActivity {
        repo: name.to_string(),
        commits_24h: 0,
        commits_7d: 0,
        insertions_24h: 0,
        deletions_24h: 0,
        insertions_7d: 0,
        deletions_7d: 0,
        agent_commits_7d: 0,
        human_commits_7d: 0,
        last_commit_at: None,
        daily: Vec::new(),
    }
}

async fn scan_repo_git(name: &str, path: &Path) -> anyhow::Result<RepoGitActivity> {
    // Pass 1: hash, epoch, Co-Authored-By trailers (one line per commit).
    let meta = git_stdout(
        path,
        &[
            "log",
            "--since=14.days",
            "--pretty=%H%x1f%ct%x1f%(trailers:key=Co-authored-by,valueonly,separator=;)",
        ],
    )
    .await?;
    let mut commits: HashMap<String, CommitStat> = HashMap::new();
    for line in meta.lines() {
        let mut parts = line.split('\u{1f}');
        let (Some(hash), Some(epoch)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Ok(epoch) = epoch.trim().parse::<i64>() else {
            continue;
        };
        let trailers = parts.next().unwrap_or("").to_ascii_lowercase();
        let agent = trailers.contains("claude") || trailers.contains("codex");
        commits.insert(
            hash.to_string(),
            CommitStat {
                epoch,
                insertions: 0,
                deletions: 0,
                agent,
            },
        );
    }

    // Pass 2: numstat per commit.
    let numstat = git_stdout(
        path,
        &["log", "--since=14.days", "--pretty=%x01%H", "--numstat"],
    )
    .await?;
    let mut current: Option<&mut CommitStat> = None;
    for line in numstat.lines() {
        if let Some(hash) = line.strip_prefix('\u{01}') {
            current = commits.get_mut(hash.trim());
            continue;
        }
        let Some(stat) = current.as_deref_mut() else {
            continue;
        };
        let mut cols = line.split('\t');
        let (Some(adds), Some(dels)) = (cols.next(), cols.next()) else {
            continue;
        };
        // Binary files report "-"; skip them.
        if let (Ok(adds), Ok(dels)) = (adds.trim().parse::<i64>(), dels.trim().parse::<i64>()) {
            stat.insertions += adds;
            stat.deletions += dels;
        }
    }

    let now = Utc::now().timestamp();
    let day_ago = now - 24 * 3600;
    let week_ago = now - 7 * 24 * 3600;
    let mut activity = empty_activity(name);
    let mut daily: HashMap<NaiveDate, GitDay> = HashMap::new();
    for stat in commits.values() {
        let at = Utc
            .timestamp_opt(stat.epoch, 0)
            .single()
            .unwrap_or_else(Utc::now);
        if activity.last_commit_at.is_none_or(|prev| at > prev) {
            activity.last_commit_at = Some(at);
        }
        if stat.epoch >= week_ago {
            activity.commits_7d += 1;
            activity.insertions_7d += stat.insertions;
            activity.deletions_7d += stat.deletions;
            if stat.agent {
                activity.agent_commits_7d += 1;
            } else {
                activity.human_commits_7d += 1;
            }
        }
        if stat.epoch >= day_ago {
            activity.commits_24h += 1;
            activity.insertions_24h += stat.insertions;
            activity.deletions_24h += stat.deletions;
        }
        let day = at.date_naive();
        let entry = daily.entry(day).or_insert(GitDay {
            day,
            commits: 0,
            insertions: 0,
            deletions: 0,
        });
        entry.commits += 1;
        entry.insertions += stat.insertions;
        entry.deletions += stat.deletions;
    }
    let mut days: Vec<GitDay> = daily.into_values().collect();
    days.sort_by_key(|d| d.day);
    activity.daily = days;
    Ok(activity)
}

async fn git_stdout(path: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = tokio::process::Command::new("git")
        // Command-line config counts as protected, so this authorizes the
        // read even when the mount's uid differs from the service uid.
        .arg("-c")
        .arg(format!("safe.directory={}", path.display()))
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "git {:?} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

// ─── Churn hotspots ─────────────────────────────────────────────────

async fn churn_hotspots(pool: &Pool) -> anyhow::Result<Vec<ChurnHotspot>> {
    let rows: Vec<ChurnHotspot> = sqlx::query_as(
        "SELECT t.repo_name AS repo, t.repo_rel_path AS path, \
                COUNT(DISTINCT (t.session_uuid, t.turn_id))::BIGINT AS write_turns, \
                COUNT(DISTINCT t.session_uuid)::BIGINT AS sessions, \
                MAX(tt.end_timestamp) AS last_write_at \
         FROM timeline_file_touches t \
         JOIN timeline_turns tt \
           ON tt.session_uuid = t.session_uuid AND tt.turn_id = t.turn_id \
         WHERE t.is_write AND tt.end_timestamp >= NOW() - INTERVAL '7 days' \
         GROUP BY t.repo_name, t.repo_rel_path \
         HAVING COUNT(DISTINCT (t.session_uuid, t.turn_id)) >= $1 \
         ORDER BY write_turns DESC, last_write_at DESC \
         LIMIT 40",
    )
    .bind(CHURN_MIN_TURNS)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// ─── Plan flow ──────────────────────────────────────────────────────

fn size_weight(size: Option<&str>) -> i64 {
    match size {
        Some("m") => 2,
        Some("l") => 3,
        _ => 1,
    }
}

#[derive(sqlx::FromRow)]
struct FlowPlan {
    id: Uuid,
    repo_name: String,
    title: String,
    status: String,
}

#[derive(sqlx::FromRow)]
struct FlowPhase {
    id: Uuid,
    plan_id: Uuid,
    status: String,
    size: Option<String>,
    created_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct FlowEvent {
    phase_id: Option<Uuid>,
    event_type: String,
    to_status: Option<String>,
    created_at: DateTime<Utc>,
}

/// Status of a phase as of `at`, replayed from its transition history.
fn status_at(transitions: &[(DateTime<Utc>, String)], at: DateTime<Utc>) -> Option<&str> {
    let mut status: Option<&str> = None;
    for (ts, next) in transitions {
        if *ts > at {
            break;
        }
        status = Some(next.as_str());
    }
    status
}

async fn flow_metrics(pool: &Pool) -> anyhow::Result<FlowMetrics> {
    let plans: Vec<FlowPlan> = sqlx::query_as(
        "SELECT id, repo_name, title, status FROM plans \
         WHERE status IN ('active', 'paused') OR updated_at >= NOW() - INTERVAL '30 days' \
         ORDER BY updated_at DESC LIMIT 100",
    )
    .fetch_all(pool)
    .await?;
    if plans.is_empty() {
        return Ok(FlowMetrics {
            wip: 0,
            blocked: 0,
            throughput_weeks: Vec::new(),
            cycle_time_hours_p50: None,
            cfd: Vec::new(),
            burndowns: Vec::new(),
        });
    }
    let plan_ids: Vec<Uuid> = plans.iter().map(|plan| plan.id).collect();
    let phases: Vec<FlowPhase> = sqlx::query_as(
        "SELECT id, plan_id, status, size, created_at, started_at, completed_at \
         FROM plan_phases WHERE plan_id = ANY($1)",
    )
    .bind(&plan_ids)
    .fetch_all(pool)
    .await?;
    let events: Vec<FlowEvent> = sqlx::query_as(
        "SELECT phase_id, event_type, to_status, created_at FROM plan_events \
         WHERE plan_id = ANY($1) AND phase_id IS NOT NULL \
         ORDER BY created_at, id",
    )
    .bind(&plan_ids)
    .fetch_all(pool)
    .await?;

    let transitions = phase_transitions(&phases, &events);
    let now = Utc::now();
    let days = daily_window(now);
    let completion = completion_stats(&phases, now);

    Ok(FlowMetrics {
        wip: phases
            .iter()
            .filter(|phase| phase.status == "in_progress")
            .count() as i64,
        blocked: phases
            .iter()
            .filter(|phase| phase.status == "blocked")
            .count() as i64,
        throughput_weeks: completion.throughput_weeks,
        cycle_time_hours_p50: completion.cycle_time_hours_p50,
        cfd: cumulative_flow(&phases, &transitions, &days, now),
        burndowns: plan_burndowns(&plans, &phases, &transitions, &days, now),
    })
}

/// Status history per phase: a seeded `pending` at creation, then every
/// recorded status-bearing event in order.
fn phase_transitions(
    phases: &[FlowPhase],
    events: &[FlowEvent],
) -> HashMap<Uuid, Vec<(DateTime<Utc>, String)>> {
    let mut transitions: HashMap<Uuid, Vec<(DateTime<Utc>, String)>> = HashMap::new();
    for phase in phases {
        transitions.insert(phase.id, vec![(phase.created_at, "pending".to_string())]);
    }
    for event in events {
        let (Some(phase_id), Some(to_status)) = (event.phase_id, event.to_status.as_deref()) else {
            continue;
        };
        if matches!(
            event.event_type.as_str(),
            "phase_added" | "phase_status_changed" | "phase_updated"
        ) {
            if let Some(history) = transitions.get_mut(&phase_id) {
                history.push((event.created_at, to_status.to_string()));
            }
        }
    }
    transitions
}

fn daily_window(now: DateTime<Utc>) -> Vec<NaiveDate> {
    let today = now.date_naive();
    (0..DAILY_WINDOW_DAYS)
        .rev()
        .filter_map(|back| today.checked_sub_days(chrono::Days::new(back as u64)))
        .collect()
}

/// End of `day`, clamped to now so the current day reflects only elapsed time.
fn day_end(day: NaiveDate, now: DateTime<Utc>) -> DateTime<Utc> {
    let next = day.checked_add_days(chrono::Days::new(1)).unwrap_or(day);
    Utc.from_utc_datetime(&next.and_hms_opt(0, 0, 0).unwrap_or_default())
        .min(now)
}

/// Cumulative flow across every tracked plan.
fn cumulative_flow(
    phases: &[FlowPhase],
    transitions: &HashMap<Uuid, Vec<(DateTime<Utc>, String)>>,
    days: &[NaiveDate],
    now: DateTime<Utc>,
) -> Vec<CfdDay> {
    days.iter()
        .map(|&day| {
            let at = day_end(day, now);
            let mut bucket = CfdDay {
                day,
                ..CfdDay::default()
            };
            for phase in phases {
                let weight = size_weight(phase.size.as_deref());
                let Some(status) = transitions
                    .get(&phase.id)
                    .and_then(|history| status_at(history, at))
                else {
                    continue;
                };
                match status {
                    "pending" => bucket.pending += weight,
                    "in_progress" => bucket.in_progress += weight,
                    "blocked" => bucket.blocked += weight,
                    "completed" => bucket.completed += weight,
                    "skipped" => bucket.skipped += weight,
                    _ => {}
                }
            }
            bucket
        })
        .collect()
}

/// Remaining weight over time, per open plan.
fn plan_burndowns(
    plans: &[FlowPlan],
    phases: &[FlowPhase],
    transitions: &HashMap<Uuid, Vec<(DateTime<Utc>, String)>>,
    days: &[NaiveDate],
    now: DateTime<Utc>,
) -> Vec<PlanBurndown> {
    let mut burndowns = Vec::new();
    for plan in plans.iter().filter(|plan| plan.status == "active") {
        let plan_phases: Vec<&FlowPhase> = phases
            .iter()
            .filter(|phase| phase.plan_id == plan.id)
            .collect();
        if plan_phases.is_empty() {
            continue;
        }
        let total_weight: i64 = plan_phases
            .iter()
            .map(|phase| size_weight(phase.size.as_deref()))
            .sum();
        let series = days
            .iter()
            .map(|&day| {
                let at = day_end(day, now);
                let mut total = 0i64;
                let mut done = 0i64;
                for phase in &plan_phases {
                    if phase.created_at > at {
                        continue;
                    }
                    let weight = size_weight(phase.size.as_deref());
                    total += weight;
                    let status = transitions
                        .get(&phase.id)
                        .and_then(|history| status_at(history, at));
                    if matches!(status, Some("completed") | Some("skipped")) {
                        done += weight;
                    }
                }
                BurndownDay {
                    day,
                    remaining_weight: total - done,
                    total_weight: total,
                }
            })
            .collect();
        burndowns.push(PlanBurndown {
            plan_id: plan.id,
            repo: plan.repo_name.clone(),
            title: plan.title.clone(),
            total_weight,
            days: series,
        });
    }
    burndowns
}

struct CompletionStats {
    throughput_weeks: Vec<ThroughputWeek>,
    cycle_time_hours_p50: Option<f64>,
}

/// Weekly completed weight over the last four weeks, and median cycle time over
/// the last thirty days.
fn completion_stats(phases: &[FlowPhase], now: DateTime<Utc>) -> CompletionStats {
    let mut throughput: HashMap<NaiveDate, i64> = HashMap::new();
    let four_weeks_ago = now - chrono::Duration::days(28);
    for phase in phases {
        let Some(completed_at) = phase.completed_at else {
            continue;
        };
        if phase.status != "completed" || completed_at < four_weeks_ago {
            continue;
        }
        let day = completed_at.date_naive();
        let week_start = day
            .checked_sub_days(chrono::Days::new(
                day.weekday().num_days_from_monday() as u64
            ))
            .unwrap_or(day);
        *throughput.entry(week_start).or_default() += size_weight(phase.size.as_deref());
    }
    let mut throughput_weeks: Vec<ThroughputWeek> = throughput
        .into_iter()
        .map(|(week_start, completed_weight)| ThroughputWeek {
            week_start,
            completed_weight,
        })
        .collect();
    throughput_weeks.sort_by_key(|week| week.week_start);

    let mut cycle_hours: Vec<f64> = phases
        .iter()
        .filter(|phase| phase.status == "completed")
        .filter_map(|phase| {
            let started = phase.started_at?;
            let completed = phase.completed_at?;
            if completed < now - chrono::Duration::days(30) {
                return None;
            }
            Some((completed - started).num_minutes() as f64 / 60.0)
        })
        .collect();
    cycle_hours.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let cycle_time_hours_p50 =
        (!cycle_hours.is_empty()).then(|| cycle_hours[cycle_hours.len() / 2]);

    CompletionStats {
        throughput_weeks,
        cycle_time_hours_p50,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_weights_default_to_one() {
        assert_eq!(size_weight(None), 1);
        assert_eq!(size_weight(Some("s")), 1);
        assert_eq!(size_weight(Some("m")), 2);
        assert_eq!(size_weight(Some("l")), 3);
    }

    #[test]
    fn status_replay_honours_ordering() {
        let base = Utc::now();
        let history = vec![
            (base, "pending".to_string()),
            (base + chrono::Duration::hours(1), "in_progress".to_string()),
            (base + chrono::Duration::hours(5), "completed".to_string()),
        ];
        assert_eq!(status_at(&history, base - chrono::Duration::hours(1)), None);
        assert_eq!(
            status_at(&history, base + chrono::Duration::hours(2)),
            Some("in_progress")
        );
        assert_eq!(
            status_at(&history, base + chrono::Duration::hours(6)),
            Some("completed")
        );
    }
}
