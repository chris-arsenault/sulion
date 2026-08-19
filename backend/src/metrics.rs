//! Portfolio metrics for the monitor/metrics surfaces.
//!
//! Everything here is derived from data Sulion already stores — no new
//! agent-side telemetry:
//! - token rollups, daily series, and model attribution from
//!   `agent_model_usage_daily`, split into input, cache reads, and output,
//! - node-materialized git activity with agent/human attribution via
//!   `Co-Authored-By` trailers,
//! - churn hotspots from `timeline_file_touches` write re-touches,
//! - agile flow (CFD, burndown, throughput, cycle time) replayed from the
//!   append-only `plan_events` history, weighted by optional phase size.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::db::Pool;
use crate::git_activity::{empty_activity, RepoGitActivity};

const DAILY_WINDOW_DAYS: i64 = 14;
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
    /// Standard input plus cache writes. Cache reads are deliberately excluded
    /// so the three headline categories do not overlap.
    pub input_tokens: i64,
    /// Subset of `input_tokens` written into a provider prompt cache.
    pub cache_write_input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    /// API list-price equivalent for models in the catalog.
    pub estimated_cost_usd: f64,
    /// Tokens omitted from the estimate because their model has no known rate.
    pub unpriced_tokens: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageMetrics {
    pub all_time: UsageWindow,
    pub today: UsageWindow,
    pub last_7d: UsageWindow,
    pub per_repo: Vec<RepoUsage>,
    pub by_model: Vec<ModelUsage>,
    pub model_window_days: i64,
    pub daily: Vec<UsageDay>,
    pub pricing: UsagePricingMetadata,
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
    pub input_tokens: i64,
    pub cache_write_input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub estimated_cost_usd: f64,
    pub unpriced_tokens: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelUsage {
    pub model: String,
    pub agent: String,
    pub usage: UsageWindow,
    pub price: Option<ModelPrice>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ModelPrice {
    pub input_usd_per_million: f64,
    pub cached_input_usd_per_million: f64,
    pub cache_write_usd_per_million: f64,
    pub cache_write_1h_usd_per_million: f64,
    pub output_usd_per_million: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsagePricingMetadata {
    pub basis: &'static str,
    pub as_of: NaiveDate,
    pub openai_source_url: &'static str,
    pub anthropic_source_url: &'static str,
    pub note: &'static str,
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

#[derive(Debug, Clone, Copy, Default)]
struct UsageComponents {
    standard_input: i64,
    cache_read: i64,
    cache_write: i64,
    cache_write_1h: i64,
    output: i64,
}

impl UsageComponents {
    fn total(self) -> i64 {
        self.standard_input
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
            .saturating_add(self.cache_write_1h)
            .saturating_add(self.output)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct UsageAccumulator {
    usage: UsageComponents,
    cost_nanos: i128,
    unpriced_tokens: i64,
}

impl UsageAccumulator {
    fn add(&mut self, usage: UsageComponents, model: Option<&str>) {
        self.usage.standard_input = self
            .usage
            .standard_input
            .saturating_add(usage.standard_input);
        self.usage.cache_read = self.usage.cache_read.saturating_add(usage.cache_read);
        self.usage.cache_write = self.usage.cache_write.saturating_add(usage.cache_write);
        self.usage.cache_write_1h = self
            .usage
            .cache_write_1h
            .saturating_add(usage.cache_write_1h);
        self.usage.output = self.usage.output.saturating_add(usage.output);
        if let Some(price) = model.and_then(catalog_price) {
            self.cost_nanos += price.cost_nanos(usage);
        } else {
            self.unpriced_tokens = self.unpriced_tokens.saturating_add(usage.total());
        }
    }

    fn window(self) -> UsageWindow {
        UsageWindow {
            input_tokens: self
                .usage
                .standard_input
                .saturating_add(self.usage.cache_write)
                .saturating_add(self.usage.cache_write_1h),
            cache_write_input_tokens: self
                .usage
                .cache_write
                .saturating_add(self.usage.cache_write_1h),
            cached_input_tokens: self.usage.cache_read,
            output_tokens: self.usage.output,
            estimated_cost_usd: ((self.cost_nanos as f64 / 1_000_000_000.0) * 1_000_000.0).round()
                / 1_000_000.0,
            unpriced_tokens: self.unpriced_tokens,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct CatalogPrice {
    public: ModelPrice,
    input_nanos: i64,
    cached_input_nanos: i64,
    cache_write_nanos: i64,
    cache_write_1h_nanos: i64,
    output_nanos: i64,
}

impl CatalogPrice {
    fn cost_nanos(self, usage: UsageComponents) -> i128 {
        i128::from(usage.standard_input) * i128::from(self.input_nanos)
            + i128::from(usage.cache_read) * i128::from(self.cached_input_nanos)
            + i128::from(usage.cache_write) * i128::from(self.cache_write_nanos)
            + i128::from(usage.cache_write_1h) * i128::from(self.cache_write_1h_nanos)
            + i128::from(usage.output) * i128::from(self.output_nanos)
    }
}

fn price(
    input: f64,
    cached_input: f64,
    cache_write: f64,
    cache_write_1h: f64,
    output: f64,
) -> CatalogPrice {
    CatalogPrice {
        public: ModelPrice {
            input_usd_per_million: input,
            cached_input_usd_per_million: cached_input,
            cache_write_usd_per_million: cache_write,
            cache_write_1h_usd_per_million: cache_write_1h,
            output_usd_per_million: output,
        },
        input_nanos: (input * 1_000.0).round() as i64,
        cached_input_nanos: (cached_input * 1_000.0).round() as i64,
        cache_write_nanos: (cache_write * 1_000.0).round() as i64,
        cache_write_1h_nanos: (cache_write_1h * 1_000.0).round() as i64,
        output_nanos: (output * 1_000.0).round() as i64,
    }
}

fn catalog_price(model: &str) -> Option<CatalogPrice> {
    match model {
        "gpt-5.6-sol" => Some(price(5.0, 0.5, 6.25, 6.25, 30.0)),
        "claude-opus-5" => Some(price(5.0, 0.5, 6.25, 10.0, 25.0)),
        "claude-fable-5" => Some(price(10.0, 1.0, 12.5, 20.0, 50.0)),
        _ => None,
    }
}

#[derive(sqlx::FromRow)]
struct DailyUsageRow {
    day: NaiveDate,
    repo: Option<String>,
    agent: String,
    model: String,
    standard_input: i64,
    cache_read: i64,
    cache_write: i64,
    cache_write_1h: i64,
    output: i64,
}

impl DailyUsageRow {
    fn components(&self) -> UsageComponents {
        UsageComponents {
            standard_input: self.standard_input,
            cache_read: self.cache_read,
            cache_write: self.cache_write,
            cache_write_1h: self.cache_write_1h,
            output: self.output,
        }
    }
}

#[derive(Default)]
struct RepoAccumulators {
    all_time: UsageAccumulator,
    today: UsageAccumulator,
    last_7d: UsageAccumulator,
}

fn repo_name(repo: Option<&str>) -> String {
    repo.unwrap_or("(unattributed)").to_string()
}

async fn usage_metrics(pool: &Pool) -> anyhow::Result<UsageMetrics> {
    // Repo attribution, most direct first: correlated PTY, compaction-parent
    // lineage, a reverse PTY pointer, then the transcript project hash.
    let rows: Vec<DailyUsageRow> = sqlx::query_as(
        "WITH RECURSIVE lineage AS ( \
            SELECT cs.session_uuid AS origin, cs.pty_session_id, \
                   cs.parent_session_uuid, 0 AS depth \
            FROM claude_sessions cs \
          UNION ALL \
            SELECT l.origin, parent.pty_session_id, parent.parent_session_uuid, \
                   l.depth + 1 \
            FROM lineage l \
            JOIN claude_sessions parent ON parent.session_uuid = l.parent_session_uuid \
            WHERE l.pty_session_id IS NULL AND l.depth < 16 \
         ), lineage_pty AS ( \
            SELECT DISTINCT ON (origin) origin, pty_session_id \
            FROM lineage WHERE pty_session_id IS NOT NULL ORDER BY origin, depth \
         ), dimensions AS ( \
            SELECT cs.session_uuid, metadata.model, \
                COALESCE(p_direct.repo, p_reverse.repo, hash_repo.repo_name) AS repo \
            FROM claude_sessions cs \
            LEFT JOIN agent_session_metadata metadata ON metadata.session_uuid = cs.session_uuid \
            LEFT JOIN lineage_pty lp ON lp.origin = cs.session_uuid \
            LEFT JOIN pty_sessions p_direct ON p_direct.id = lp.pty_session_id \
            LEFT JOIN LATERAL ( \
                SELECT pr.repo FROM pty_sessions pr \
                WHERE pr.current_session_uuid = cs.session_uuid LIMIT 1 \
            ) p_reverse ON TRUE \
            LEFT JOIN LATERAL ( \
                SELECT r.repo_name FROM repo_runtime_state r \
                WHERE cs.project_hash IS NOT NULL \
                  AND regexp_replace(r.path, '[^A-Za-z0-9]', '-', 'g') = cs.project_hash \
                LIMIT 1 \
            ) hash_repo ON TRUE \
         ) \
         SELECT d.day, dimensions.repo, d.agent, d.model, \
            COALESCE(SUM(d.input_tokens), 0)::BIGINT AS standard_input, \
            COALESCE(SUM(d.cached_input_tokens), 0)::BIGINT AS cache_read, \
            COALESCE(SUM(d.cache_write_input_tokens), 0)::BIGINT AS cache_write, \
            COALESCE(SUM(d.cache_write_1h_input_tokens), 0)::BIGINT AS cache_write_1h, \
            COALESCE(SUM(d.output_tokens), 0)::BIGINT AS output \
         FROM agent_model_usage_daily d \
         LEFT JOIN dimensions ON dimensions.session_uuid = d.session_uuid \
         GROUP BY d.day, dimensions.repo, d.agent, d.model \
         ORDER BY d.day",
    )
    .fetch_all(pool)
    .await?;

    let now = Utc::now().date_naive();
    let week_start = now.checked_sub_days(chrono::Days::new(6)).unwrap_or(now);
    let daily_start = now
        .checked_sub_days(chrono::Days::new((DAILY_WINDOW_DAYS - 1) as u64))
        .unwrap_or(now);
    let mut days = BTreeMap::new();
    for back in (0..DAILY_WINDOW_DAYS).rev() {
        if let Some(day) = now.checked_sub_days(chrono::Days::new(back as u64)) {
            days.insert(day, UsageAccumulator::default());
        }
    }

    let mut all_time = UsageAccumulator::default();
    let mut repos: BTreeMap<String, RepoAccumulators> = BTreeMap::new();
    let mut today = UsageAccumulator::default();
    let mut last_7d = UsageAccumulator::default();
    let mut models: BTreeMap<(String, String), UsageAccumulator> = BTreeMap::new();
    let mut agents_by_model: BTreeSet<(String, String)> = BTreeSet::new();
    for row in rows {
        let components = row.components();
        all_time.add(components, Some(&row.model));
        repos
            .entry(repo_name(row.repo.as_deref()))
            .or_default()
            .all_time
            .add(components, Some(&row.model));
        if row.day >= daily_start && row.day <= now {
            days.entry(row.day)
                .or_default()
                .add(components, Some(&row.model));
            let key = (row.agent.clone(), row.model.clone());
            agents_by_model.insert(key.clone());
            models
                .entry(key)
                .or_default()
                .add(components, Some(&row.model));
        }
        if row.day == now {
            today.add(components, Some(&row.model));
            repos
                .entry(repo_name(row.repo.as_deref()))
                .or_default()
                .today
                .add(components, Some(&row.model));
        }
        if row.day >= week_start && row.day <= now {
            last_7d.add(components, Some(&row.model));
            repos
                .entry(repo_name(row.repo.as_deref()))
                .or_default()
                .last_7d
                .add(components, Some(&row.model));
        }
    }

    let mut per_repo: Vec<RepoUsage> = repos
        .into_iter()
        .map(|(repo, usage)| RepoUsage {
            repo,
            all_time: usage.all_time.window(),
            today: usage.today.window(),
            last_7d: usage.last_7d.window(),
        })
        .collect();
    per_repo.sort_by_key(|row| {
        std::cmp::Reverse(
            row.all_time
                .input_tokens
                .saturating_add(row.all_time.cached_input_tokens)
                .saturating_add(row.all_time.output_tokens),
        )
    });

    let mut by_model: Vec<ModelUsage> = agents_by_model
        .into_iter()
        .map(|(agent, model)| {
            let usage = models
                .remove(&(agent.clone(), model.clone()))
                .unwrap_or_default();
            ModelUsage {
                price: catalog_price(&model).map(|catalog| catalog.public),
                model,
                agent,
                usage: usage.window(),
            }
        })
        .collect();
    by_model.sort_by_key(|row| {
        std::cmp::Reverse(
            row.usage
                .input_tokens
                .saturating_add(row.usage.cached_input_tokens)
                .saturating_add(row.usage.output_tokens),
        )
    });

    let daily = days
        .into_iter()
        .map(|(day, usage)| {
            let window = usage.window();
            UsageDay {
                day,
                input_tokens: window.input_tokens,
                cache_write_input_tokens: window.cache_write_input_tokens,
                cached_input_tokens: window.cached_input_tokens,
                output_tokens: window.output_tokens,
                estimated_cost_usd: window.estimated_cost_usd,
                unpriced_tokens: window.unpriced_tokens,
            }
        })
        .collect();

    Ok(UsageMetrics {
        all_time: all_time.window(),
        today: today.window(),
        last_7d: last_7d.window(),
        per_repo,
        by_model,
        model_window_days: DAILY_WINDOW_DAYS,
        daily,
        pricing: UsagePricingMetadata {
            basis: "API list price (standard tier)",
            as_of: NaiveDate::from_ymd_opt(2026, 8, 19).unwrap_or(now),
            openai_source_url: "https://developers.openai.com/api/docs/models/gpt-5.6-sol",
            anthropic_source_url: "https://claude.com/pricing",
            note: "Subscription fees and long-context multipliers are not included. Unknown models are excluded from the estimate.",
        },
    })
}

// ─── Git activity ───────────────────────────────────────────────────

async fn git_activity(pool: &Pool) -> anyhow::Result<Vec<RepoGitActivity>> {
    let repos: Vec<(String, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT repo_name, git_activity_json FROM repo_runtime_state \
          WHERE \"exists\" ORDER BY repo_name",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(repos.len());
    for (name, stored) in repos {
        let Some(stored) = stored else {
            out.push(empty_activity(&name));
            continue;
        };
        match serde_json::from_value::<RepoGitActivity>(stored) {
            Ok(mut activity) => {
                activity.repo = name;
                out.push(activity);
            }
            Err(err) => {
                tracing::warn!(repo = %name, %err, "stored git activity is invalid");
                out.push(empty_activity(&name));
            }
        }
    }
    Ok(out)
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
    fn api_estimate_prices_each_token_category_separately() {
        let usage = UsageComponents {
            standard_input: 1_000_000,
            cache_read: 1_000_000,
            cache_write: 1_000_000,
            cache_write_1h: 1_000_000,
            output: 1_000_000,
        };
        let mut total = UsageAccumulator::default();
        total.add(usage, Some("claude-opus-5"));
        let window = total.window();

        assert_eq!(window.input_tokens, 3_000_000);
        assert_eq!(window.cache_write_input_tokens, 2_000_000);
        assert_eq!(window.cached_input_tokens, 1_000_000);
        assert_eq!(window.output_tokens, 1_000_000);
        assert_eq!(window.estimated_cost_usd, 46.75);
        assert_eq!(window.unpriced_tokens, 0);
    }

    #[test]
    fn unknown_models_are_reported_instead_of_guessed() {
        let usage = UsageComponents {
            standard_input: 100,
            cache_read: 200,
            cache_write: 300,
            cache_write_1h: 0,
            output: 400,
        };
        let mut total = UsageAccumulator::default();
        total.add(usage, Some("unknown-model"));
        let window = total.window();

        assert_eq!(window.estimated_cost_usd, 0.0);
        assert_eq!(window.unpriced_tokens, 1_000);
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
