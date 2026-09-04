//! Agile flow for published plans — WIP, blocked, throughput, cycle time,
//! cumulative flow, and per-tree burndown — replayed from the append-only
//! `plan_events` history and weighted by optional phase size.
//!
//! Everything here reads *leaf* phases. A phase with a branch under it is a
//! container: its span and weight already cover its sub-plan's phases, so
//! counting both would measure the same work twice. Burndown likewise rolls a
//! whole plan tree into one line per root, which is what lets scope discovered
//! mid-flight show up as a rising line rather than a second chart.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::db::Pool;

use super::DAILY_WINDOW_DAYS;

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

pub(super) fn size_weight(size: Option<&str>) -> i64 {
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
    root_plan_id: Uuid,
    depth: i32,
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

pub(super) async fn flow_metrics(pool: &Pool) -> anyhow::Result<FlowMetrics> {
    // Whole trees, not individual plans: a branch's phases only make sense
    // rolled into the root they hang off.
    let plans: Vec<FlowPlan> = sqlx::query_as(
        "WITH recent_roots AS ( \
             SELECT root_plan_id, MAX(updated_at) AS touched \
               FROM plans \
              WHERE status IN ('active', 'paused') \
                 OR updated_at >= NOW() - INTERVAL '30 days' \
              GROUP BY root_plan_id \
              ORDER BY touched DESC \
              LIMIT 100 \
         ) \
         SELECT p.id, p.repo_name, p.title, p.status, p.root_plan_id, p.depth \
           FROM plans p JOIN recent_roots r ON r.root_plan_id = p.root_plan_id \
          ORDER BY p.depth, p.created_at",
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
    let phases = leaf_phases(pool, phases).await?;

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

/// Drop phases that have a sub-plan anchored to them. Their span and weight
/// duplicate the branch's own phases, which are already in this set.
async fn leaf_phases(pool: &Pool, phases: Vec<FlowPhase>) -> anyhow::Result<Vec<FlowPhase>> {
    let phase_ids: Vec<Uuid> = phases.iter().map(|phase| phase.id).collect();
    let containers: HashSet<Uuid> = sqlx::query_scalar::<_, Uuid>(
        "SELECT DISTINCT parent_phase_id FROM plan_branch_anchors \
         WHERE parent_phase_id = ANY($1)",
    )
    .bind(&phase_ids)
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();
    Ok(phases
        .into_iter()
        .filter(|phase| !containers.contains(&phase.id))
        .collect())
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

/// Remaining weight over time, one line per open plan tree. A branch's phases
/// roll into its root, so work discovered mid-flight raises the line instead of
/// opening a second chart that hides the plan it belongs to.
fn plan_burndowns(
    plans: &[FlowPlan],
    phases: &[FlowPhase],
    transitions: &HashMap<Uuid, Vec<(DateTime<Utc>, String)>>,
    days: &[NaiveDate],
    now: DateTime<Utc>,
) -> Vec<PlanBurndown> {
    let mut burndowns = Vec::new();
    for plan in plans
        .iter()
        .filter(|plan| plan.status == "active" && plan.depth == 0)
    {
        let tree: HashSet<Uuid> = plans
            .iter()
            .filter(|member| member.root_plan_id == plan.id)
            .map(|member| member.id)
            .collect();
        let plan_phases: Vec<&FlowPhase> = phases
            .iter()
            .filter(|phase| tree.contains(&phase.plan_id))
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
