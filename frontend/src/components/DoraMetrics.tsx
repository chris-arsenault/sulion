import { useCallback, useState } from "react";
import type { ChangeEvent, FormEvent, SyntheticEvent } from "react";

import { updateDeploymentOutcome } from "../api/client";
import type {
  DeploymentOutcome,
  DoraDeployment,
  DoraMetricsView,
  RepoDoraMetrics,
} from "../api/types";
import { Tooltip } from "./ui";
import "./DoraMetrics.css";

export function DoraMetrics({
  data,
  onChanged,
}: {
  data?: DoraMetricsView;
  onChanged: () => Promise<void>;
}) {
  return (
    <section className="metrics-dora" aria-label="DORA metrics">
      <h3>
        DORA <span>· {data?.window_days ?? 30}d</span>
      </h3>
      {!data?.repos.length ? (
        <p className="metrics-chart__empty">
          No production deployment history collected yet.
        </p>
      ) : (
        <>
          <div className="metrics-dora__scroll">
            <table className="metrics-table" aria-label="DORA by repository">
              <thead>
                <tr>
                  <th>repository</th>
                  <th>
                    <Tooltip label="Successful production deployments per day over the full window.">
                      <span>deploy frequency</span>
                    </Tooltip>
                  </th>
                  <th>
                    <Tooltip label="Median time from each commit to its first production deployment. The initial observed deployment is a baseline.">
                      <span>change lead time</span>
                    </Tooltip>
                  </th>
                  <th>
                    <Tooltip label="Share of production deployments requiring immediate intervention. All deployments must have a recorded outcome.">
                      <span>change failure</span>
                    </Tooltip>
                  </th>
                  <th>
                    <Tooltip label="Median failure-to-recovery time for resolved deployment incidents. Unresolved incidents remain open.">
                      <span>failed deploy recovery</span>
                    </Tooltip>
                  </th>
                  <th>
                    <Tooltip label="Share of deployments that were unplanned work caused by a production incident. All deployments must be classified.">
                      <span>deploy rework</span>
                    </Tooltip>
                  </th>
                </tr>
              </thead>
              <tbody>
                {data.repos.map((repo) => (
                  <DoraRow key={repo.repo} repo={repo} />
                ))}
              </tbody>
            </table>
          </div>
          {data.repos.map((repo) => (
            <DeploymentDetails
              key={repo.repo}
              repo={repo}
              onChanged={onChanged}
            />
          ))}
        </>
      )}
      <p className="metrics-note">
        — means unavailable. Failure and rework rates need every deployment
        classified. Production outcomes can be recorded in deployment details.{" "}
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

function DoraRow({ repo }: { repo: RepoDoraMetrics }) {
  return (
    <tr>
      <th scope="row">
        {repo.repo}
        <span className="metrics-dora__detail">
          {repo.checked_at
            ? `${repo.deployments} deployments`
            : "awaiting deployment history"}
        </span>
      </th>
      <td className="tabular">
        {repo.deployments_per_day == null
          ? "—"
          : `${repo.deployments_per_day.toFixed(2)}/day`}
      </td>
      <td className="tabular">
        {hours(repo.change_lead_time_hours_p50)}
        <span className="metrics-dora__detail">
          {repo.lead_time_samples} measured commits
        </span>
      </td>
      <td className="tabular">
        {percent(repo.change_failure_rate)}
        <span className="metrics-dora__detail">
          {repo.classified_outcomes}/{repo.deployments} outcomes
        </span>
      </td>
      <td className="tabular">
        {hours(repo.recovery_time_hours_p50)}
        <span className="metrics-dora__detail">
          {repo.recovered_deployments} recovered · {repo.open_failures} open
        </span>
      </td>
      <td className="tabular">
        {percent(repo.rework_rate)}
        <span className="metrics-dora__detail">
          {repo.classified_rework}/{repo.deployments} classified
        </span>
      </td>
    </tr>
  );
}

function DeploymentDetails({
  repo,
  onChanged,
}: {
  repo: RepoDoraMetrics;
  onChanged: () => Promise<void>;
}) {
  const [open, setOpen] = useState(false);
  const [limit, setLimit] = useState(20);
  const onToggle = useCallback(
    (event: SyntheticEvent<HTMLDetailsElement>) =>
      setOpen(event.currentTarget.open),
    [],
  );
  const showMore = useCallback(() => setLimit((current) => current + 20), []);
  return (
    <details className="metrics-dora__deployments" onToggle={onToggle}>
      <summary>
        {repo.repo} · deployment details
        {repo.collection_error ? (
          <span className="metrics-dora__warning">
            {" "}
            · collection incomplete
          </span>
        ) : null}
      </summary>
      {open ? (
        <>
          {repo.collection_error ? (
            <p className="metrics-dora__warning">{repo.collection_error}</p>
          ) : null}
          {repo.checked_at ? (
            <p className="metrics-note">
              Last collected {new Date(repo.checked_at).toLocaleString()}.
            </p>
          ) : null}
          {repo.recent_deployments.length === 0 ? (
            <p className="metrics-note">
              No supported production deployments recorded in this window.
            </p>
          ) : (
            <ul className="metrics-dora__list">
              {repo.recent_deployments.slice(0, limit).map((deployment) => (
                <DeploymentRecord
                  key={deployment.id}
                  deployment={deployment}
                  onChanged={onChanged}
                />
              ))}
            </ul>
          )}
          {repo.recent_deployments.length > limit ? (
            <button type="button" onClick={showMore}>
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
  onChanged: () => Promise<void>;
}) {
  const [editing, setEditing] = useState(false);
  const close = useCallback(() => setEditing(false), []);
  const edit = useCallback(() => setEditing(true), []);
  return (
    <li>
      <div className="metrics-dora__record">
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
  onChanged: () => Promise<void>;
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
        await updateDeploymentOutcome(deployment.id, input);
        await onChanged();
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
