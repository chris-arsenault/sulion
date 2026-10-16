import { useCallback, useEffect, useRef, useState } from "react";
import type { ChangeEvent, FormEvent, SyntheticEvent } from "react";

import {
  getDoraDeployments,
  getDoraMetrics,
  updateDeploymentOutcome,
} from "../api/client";
import type {
  DeploymentOutcome,
  DoraDeployment,
  DoraDeploymentCursor,
  DoraDeploymentPage,
  DoraMetricsView,
  RepoDoraMetrics,
} from "../api/types";
import { Tooltip } from "./ui";
import "./DoraMetrics.css";

export function DoraMetrics({
  data,
  active = true,
}: {
  data?: DoraMetricsView;
  active?: boolean;
}) {
  const [view, setView] = useState(data);
  const [error, setError] = useState<string | null>(null);
  const refreshVersion = useRef(0);
  useEffect(() => {
    setView(data);
    setError(null);
  }, [data]);
  const summary = view?.summary;
  const refresh = useCallback(async () => {
    const version = ++refreshVersion.current;
    try {
      const updated = await getDoraMetrics();
      if (version === refreshVersion.current) {
        setView(updated);
        setError(null);
      }
    } catch (err) {
      if (version === refreshVersion.current)
        setError(
          `Outcome saved; metrics refresh failed: ${err instanceof Error ? err.message : "unknown error"}`,
        );
    }
  }, []);
  return (
    <section className="metrics-dora" aria-label="DORA metrics">
      <h3>
        DORA <span>· {view?.window_days ?? 30}d · all repositories</span>
      </h3>
      {!summary ? (
        <p className="metrics-chart__empty">
          Aggregate deployment metrics are not available yet.
        </p>
      ) : (
        <>
          <DoraTiles summary={summary} />
          <p className="metrics-note">
            {summary.deployments} deployments ·{" "}
            {summary.collected_repository_count ?? (summary.checked_at ? 1 : 0)}
            /{summary.repository_count ?? 1} repositories collected
          </p>
          {summary.collection_error ? (
            <p className="metrics-dora__warning">{summary.collection_error}</p>
          ) : null}
          <DeploymentDetails
            active={active}
            total={summary.deployments}
            onSaved={refresh}
          />
        </>
      )}
      {error ? (
        <p role="alert" className="metrics-dora__warning">
          {error}
        </p>
      ) : null}
      <p className="metrics-note">
        — means unavailable. Rates require classified outcomes.{" "}
        <a
          href="https://dora.dev/guides/dora-metrics/"
          target="_blank"
          rel="noreferrer"
        >
          DORA definitions
        </a>
      </p>
    </section>
  );
}

function DoraTiles({ summary }: { summary: RepoDoraMetrics }) {
  return (
    <div className="metrics-dora__tiles">
      <DoraTile
        label="deploy frequency"
        value={
          summary.deployments_per_day == null
            ? "—"
            : `${summary.deployments_per_day.toFixed(2)}/day`
        }
        detail="30-day average"
        description="Observed production deployments per day over the full window."
      />
      <DoraTile
        label="change lead time"
        value={hours(summary.change_lead_time_hours_p50)}
        detail={`${summary.lead_time_samples} measured commits`}
        description="Median across measured commits from commit to first production deployment. The initial deployment is a baseline."
      />
      <DoraTile
        label="change failure"
        value={percent(summary.change_failure_rate)}
        detail={`${summary.classified_outcomes}/${summary.deployments} outcomes`}
        description="Share of observed deployments requiring immediate intervention. Every deployment needs an outcome."
      />
      <DoraTile
        label="failed deploy recovery"
        value={hours(summary.recovery_time_hours_p50)}
        detail={`${summary.recovered_deployments} recovered · ${summary.open_failures} open`}
        description="Median failure-to-recovery time across resolved deployment incidents."
      />
      <DoraTile
        label="deploy rework"
        value={percent(summary.rework_rate)}
        detail={`${summary.classified_rework}/${summary.deployments} classified`}
        description="Share of deployments that were unplanned work caused by a production incident."
      />
    </div>
  );
}

function DoraTile({
  label,
  value,
  detail,
  description,
}: {
  label: string;
  value: string;
  detail: string;
  description: string;
}) {
  return (
    <Tooltip label={description}>
      <div className="metrics-tile">
        <span>{label}</span>
        <strong className="tabular">{value}</strong>
        <small>{detail}</small>
      </div>
    </Tooltip>
  );
}

function DeploymentDetails({
  total,
  active,
  onSaved,
}: {
  total: number;
  active: boolean;
  onSaved: () => Promise<void>;
}) {
  const [open, setOpen] = useState(false);
  const [page, setPage] = useState<DoraDeploymentPage | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const request = useRef<AbortController | null>(null);
  const onToggle = useCallback(
    (event: SyntheticEvent<HTMLDetailsElement>) =>
      setOpen(event.currentTarget.open),
    [],
  );
  const load = useCallback(
    async (cursor?: DoraDeploymentCursor) => {
      request.current?.abort();
      const controller = new AbortController();
      request.current = controller;
      setLoading(true);
      setError(null);
      if (!cursor) setPage(null);
      try {
        const result = await getDoraDeployments(
          undefined,
          cursor,
          controller.signal,
        );
        if (!controller.signal.aborted)
          setPage((previous) => ({
            deployments:
              cursor && previous
                ? [...previous.deployments, ...result.deployments]
                : result.deployments,
            next_cursor: result.next_cursor,
          }));
      } catch (err) {
        if (!controller.signal.aborted)
          setError(
            err instanceof Error ? err.message : "Could not load deployments.",
          );
      } finally {
        if (!controller.signal.aborted) setLoading(false);
      }
    },
    [],
  );
  useEffect(() => {
    if (open && active) void load();
    return () => request.current?.abort();
  }, [open, active, load]);
  const showMore = useCallback(() => {
    if (page?.next_cursor) void load(page.next_cursor);
  }, [page?.next_cursor, load]);
  const retry = useCallback(() => {
    void load();
  }, [load]);
  const changed = useCallback(
    async (updated: DoraDeployment) => {
      setPage(
        (current) =>
          current && {
            ...current,
            deployments: current.deployments.map((item) =>
              item.id === updated.id ? updated : item,
            ),
          },
      );
      await onSaved();
    },
    [onSaved],
  );
  return (
    <details className="metrics-dora__deployments" onToggle={onToggle}>
      <summary>Deployment details · {total}</summary>
      {open ? (
        <>
          {error ? (
            <div role="alert" className="metrics-dora__warning">
              {error}{" "}
              <button type="button" onClick={retry}>
                Retry
              </button>
            </div>
          ) : null}
          {loading ? <p role="status">Loading deployments…</p> : null}
          {page?.deployments.length === 0 ? (
            <p className="metrics-note">
              No supported production deployments recorded in this window.
            </p>
          ) : (
            <ul className="metrics-dora__list">
              {page?.deployments.map((deployment) => (
                <DeploymentRecord
                  key={deployment.id}
                  deployment={deployment}
                  onChanged={changed}
                />
              ))}
            </ul>
          )}
          {page?.next_cursor ? (
            <button type="button" onClick={showMore} disabled={loading}>
              Show more deployments
            </button>
          ) : null}
        </>
      ) : null}
    </details>
  );
}

function DeploymentRecord({
  deployment,
  onChanged,
}: {
  deployment: DoraDeployment;
  onChanged: (updated: DoraDeployment) => Promise<void>;
}) {
  const [editing, setEditing] = useState(false);
  const close = useCallback(() => setEditing(false), []);
  const edit = useCallback(() => setEditing(true), []);
  return (
    <li>
      <div className="metrics-dora__record">
        <span>{deployment.repo_name}</span>
        <a href={deployment.source_url} target="_blank" rel="noreferrer">
          {deployment.revision.slice(0, 8)}
        </a>
        <time dateTime={deployment.deployed_at}>
          {new Date(deployment.deployed_at).toLocaleString()}
        </time>
        <span>
          {deployment.outcome}
          {deployment.outcome === "failed"
            ? deployment.recovered_at
              ? " · recovered"
              : " · open"
            : ""}
        </span>
        <button type="button" onClick={edit}>
          Record outcome
        </button>
      </div>
      {editing ? (
        <OutcomeForm
          deployment={deployment}
          onChanged={onChanged}
          onClose={close}
        />
      ) : null}
    </li>
  );
}

function OutcomeForm({
  deployment,
  onChanged,
  onClose,
}: {
  deployment: DoraDeployment;
  onChanged: (updated: DoraDeployment) => Promise<void>;
  onClose: () => void;
}) {
  const [outcome, setOutcome] = useState(deployment.outcome);
  const [failed, setFailed] = useState(localTime(deployment.failed_at));
  const [recovered, setRecovered] = useState(
    localTime(deployment.recovered_at),
  );
  const [rework, setRework] = useState(
    deployment.rework == null
      ? "unknown"
      : deployment.rework
        ? "rework"
        : "planned",
  );
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const changeOutcome = useCallback(
    (event: ChangeEvent<HTMLSelectElement>) =>
      setOutcome(event.target.value as DeploymentOutcome["outcome"]),
    [],
  );
  const changeFailed = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => setFailed(event.target.value),
    [],
  );
  const changeRecovered = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => setRecovered(event.target.value),
    [],
  );
  const changeRework = useCallback(
    (event: ChangeEvent<HTMLSelectElement>) => setRework(event.target.value),
    [],
  );
  const submit = useCallback(
    async (event: FormEvent) => {
      event.preventDefault();
      setSaving(true);
      setError(null);
      try {
        const input: DeploymentOutcome = {
          outcome,
          failed_at:
            outcome === "failed" && failed
              ? new Date(failed).toISOString()
              : null,
          recovered_at:
            outcome === "failed" && recovered
              ? new Date(recovered).toISOString()
              : null,
          rework: rework === "unknown" ? null : rework === "rework",
        };
        const updated = await updateDeploymentOutcome(deployment.id, input);
        await onChanged(updated);
        onClose();
      } catch (err) {
        setError(
          err instanceof Error
            ? err.message
            : "Could not record production outcome.",
        );
      } finally {
        setSaving(false);
      }
    },
    [outcome, failed, recovered, rework, deployment.id, onChanged, onClose],
  );
  return (
    <form
      className="metrics-dora__form"
      aria-label={`Production outcome for ${deployment.revision.slice(0, 8)}`}
      onSubmit={submit}
    >
      <label>
        Production outcome
        <select value={outcome} onChange={changeOutcome} disabled={saving}>
          <option value="unknown">Unknown</option>
          <option value="healthy">Healthy</option>
          <option value="failed">Required intervention</option>
        </select>
      </label>
      {outcome === "failed" ? (
        <>
          <label>
            Failure time (local)
            <input
              type="datetime-local"
              step="0.001"
              value={failed}
              required
              min={localTime(deployment.deployed_at)}
              onChange={changeFailed}
              disabled={saving}
            />
          </label>
          <label>
            Recovery time (local, optional)
            <input
              type="datetime-local"
              step="0.001"
              value={recovered}
              min={failed || localTime(deployment.deployed_at)}
              onChange={changeRecovered}
              disabled={saving}
            />
          </label>
        </>
      ) : null}
      <label>
        Deployment purpose
        <select value={rework} onChange={changeRework} disabled={saving}>
          <option value="unknown">Unknown</option>
          <option value="planned">Planned work</option>
          <option value="rework">Unplanned incident rework</option>
        </select>
      </label>
      <div className="metrics-dora__actions">
        <button type="submit" disabled={saving}>
          {saving ? "Saving…" : "Save outcome"}
        </button>
        <button type="button" disabled={saving} onClick={onClose}>
          Cancel
        </button>
      </div>
      {error ? (
        <p role="alert" className="metrics-dora__warning">
          {error}
        </p>
      ) : null}
    </form>
  );
}

function hours(value: number | null): string {
  if (value == null) return "—";
  if (value < 1) return `${Math.round(value * 60)}m`;
  if (value < 48) return `${value.toFixed(1)}h`;
  return `${(value / 24).toFixed(1)}d`;
}

function percent(value: number | null): string {
  return value == null ? "—" : `${(value * 100).toFixed(1)}%`;
}

function localTime(iso: string | null): string {
  if (!iso) return "";
  const date = new Date(iso);
  return new Date(date.getTime() - date.getTimezoneOffset() * 60_000)
    .toISOString()
    .slice(0, 23);
}
