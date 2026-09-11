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

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;

use crate::db::Pool;
use crate::git_activity::{empty_activity, RepoGitActivity};

mod flow;

use flow::flow_metrics;
pub use flow::{BurndownDay, CfdDay, FlowMetrics, PlanBurndown, ThroughputWeek};

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
        // https://developers.openai.com/api/docs/models/<model>, checked 2026-09-11.
        // OpenAI cache writes cost 1.25 times standard input.
        "gpt-6-astra" => Some(price(10.0, 1.0, 12.5, 12.5, 50.0)),
        "gpt-5.6-sol" | "gpt-5.6" => Some(price(4.0, 0.4, 5.0, 5.0, 20.0)),
        "gpt-5.6-terra" => Some(price(2.0, 0.2, 2.5, 2.5, 12.0)),
        "gpt-5.6-luna" => Some(price(0.2, 0.02, 0.25, 0.25, 1.2)),
        // https://platform.claude.com/docs/en/about-claude/pricing, checked 2026-09-11.
        "claude-opus-5" | "claude-opus-4-8" => Some(price(5.0, 0.5, 6.25, 10.0, 25.0)),
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
    let rows: Vec<DailyUsageRow> = sqlx::query_as(include_str!("metrics/usage.sql"))
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
            as_of: NaiveDate::from_ymd_opt(2026, 9, 11).unwrap_or(now),
            openai_source_url: "https://developers.openai.com/api/docs/pricing",
            anthropic_source_url: "https://platform.claude.com/docs/en/about-claude/pricing",
            note: "Current catalog rates apply to all dates, not historical invoices. Subscription fees, service-tier and long-context multipliers are excluded. Unknown models make estimates incomplete.",
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn current_models_price_every_token_category() {
        let usage = UsageComponents {
            standard_input: 1_000_000,
            cache_read: 2_000_000,
            cache_write: 1_000_000,
            cache_write_1h: 0,
            output: 1_000_000,
        };
        for (model, expected) in [
            ("gpt-6-astra", 74.5),
            ("gpt-5.6-sol", 29.8),
            ("gpt-5.6", 29.8),
            ("gpt-5.6-terra", 16.9),
            ("gpt-5.6-luna", 1.69),
            ("claude-opus-4-8", 37.25),
        ] {
            let mut total = UsageAccumulator::default();
            total.add(usage, Some(model));
            let window = total.window();
            assert_eq!(window.estimated_cost_usd, expected, "{model}");
            assert_eq!(window.unpriced_tokens, 0, "{model}");
        }
    }
}
