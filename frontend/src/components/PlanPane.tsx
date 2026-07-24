import {
  type ChangeEvent,
  type FormEvent,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from "react";
import { useShallow } from "zustand/react/shallow";

import {
  addPlanPhase,
  attachPlan,
  createPlan,
  detachPlan,
  getPlan,
  getPlanEvents,
  listRepoPlans,
  updatePlan,
  updatePlanPhase,
} from "../api/client";
import type {
  NewPlanPhaseInput,
  PlanEventView,
  PlanPhaseStatus,
  PlanPhaseView,
  PlanStatus,
  PlanView,
  SessionView,
  UpdatePlanPhaseInput,
} from "../api/types";
import { Icon } from "../icons";
import { useSessions } from "../state/SessionStore";
import { useTabs } from "../state/TabStore";
import "./PlanPane.css";

interface PlanPaneProps {
  repo: string;
  planId?: string;
  active?: boolean;
}

type PlanMutation = (operation: () => Promise<PlanView>) => Promise<void>;

export function PlanPane({ repo, planId, active = true }: PlanPaneProps) {
  return planId ? (
    <PlanDetail repo={repo} planId={planId} active={active} />
  ) : (
    <PlanIndex repo={repo} active={active} />
  );
}

function PlanIndex({ repo, active }: { repo: string; active: boolean }) {
  const [plans, setPlans] = useState<PlanView[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showClosed, setShowClosed] = useState(false);
  const [title, setTitle] = useState("");
  const [summary, setSummary] = useState("");
  const [phaseText, setPhaseText] = useState("");
  const [attachPty, setAttachPty] = useState("");
  const [creating, setCreating] = useState(false);
  const openTab = useTabs((store) => store.openTab);
  const { sessions, refresh, planRevisionKey } = useSessions(
    useShallow((store) => ({
      sessions: store.sessions,
      refresh: store.refresh,
      planRevisionKey: store.plans
        .filter((plan) => plan.repo_name === repo)
        .map((plan) => `${plan.id}:${plan.revision}`)
        .join(","),
    })),
  );
  const repoSessions = useMemo(
    () => sessions.filter((session) => session.repo === repo && session.state === "live"),
    [repo, sessions],
  );
  const onShowClosedChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => setShowClosed(event.target.checked),
    [],
  );
  const onTitleChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => setTitle(event.target.value),
    [],
  );
  const onSummaryChange = useCallback(
    (event: ChangeEvent<HTMLTextAreaElement>) => setSummary(event.target.value),
    [],
  );
  const onPhaseTextChange = useCallback(
    (event: ChangeEvent<HTMLTextAreaElement>) => setPhaseText(event.target.value),
    [],
  );
  const onAttachPtyChange = useCallback(
    (event: ChangeEvent<HTMLSelectElement>) => setAttachPty(event.target.value),
    [],
  );
  const openPlan = useCallback(
    (nextPlanId: string) => openTab({ kind: "plan", repo, planId: nextPlanId }),
    [openTab, repo],
  );

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setPlans(await listRepoPlans(repo, showClosed));
      setError(null);
    } catch (err) {
      setError(messageOf(err));
    } finally {
      setLoading(false);
    }
  }, [repo, showClosed]);

  useEffect(() => {
    if (active) void load();
  }, [active, load, planRevisionKey]);

  const onCreate = useCallback(
    async (event: FormEvent) => {
      event.preventDefault();
      const phases = parsePhases(phaseText);
      if (!title.trim()) {
        setError("Plan title is required.");
        return;
      }
      if (phases.length === 0) {
        setError("Add at least one phase, one per line.");
        return;
      }
      setCreating(true);
      try {
        const created = await createPlan(repo, {
          title: title.trim(),
          summary: summary.trim(),
          phases,
          attach_pty_id: attachPty || undefined,
        });
        setTitle("");
        setSummary("");
        setPhaseText("");
        setAttachPty("");
        setError(null);
        await refresh();
        openTab({ kind: "plan", repo, planId: created.id });
      } catch (err) {
        setError(messageOf(err));
      } finally {
        setCreating(false);
      }
    },
    [attachPty, openTab, phaseText, refresh, repo, summary, title],
  );

  return (
    <div className="plan-pane" data-testid="plan-pane">
      <header className="plan-pane__header">
        <div>
          <div className="plan-pane__eyebrow">{repo}</div>
          <h2>Published plans</h2>
          <p>Lightweight phases agents can publish alongside their detailed work.</p>
        </div>
        <label className="plan-pane__closed-toggle">
          <input
            type="checkbox"
            checked={showClosed}
            onChange={onShowClosedChange}
          />
          Show closed
        </label>
      </header>

      <div className="plan-pane__index">
        <form className="plan-pane__create" onSubmit={onCreate}>
          <div className="plan-pane__section-heading">
            <Icon name="plus" size={14} />
            <span>Start a plan</span>
          </div>
          <label>
            Title
            <input
              value={title}
              onChange={onTitleChange}
              placeholder="Ship native plans and overview"
              maxLength={160}
            />
          </label>
          <label>
            Short description
            <textarea
              value={summary}
              onChange={onSummaryChange}
              placeholder="What this plan is for"
              rows={2}
              maxLength={1_000}
            />
          </label>
          <label>
            Phases
            <textarea
              aria-label="Phases"
              value={phaseText}
              onChange={onPhaseTextChange}
              placeholder={"Backend | Durable data and commands\nFrontend | Plan workspace\nVerify | Tests and docs"}
              rows={5}
            />
            <span className="plan-pane__hint">
              One phase per line. Add a description after <code>|</code>.
            </span>
          </label>
          <label>
            Attach to terminal
            <select
              value={attachPty}
              onChange={onAttachPtyChange}
            >
              <option value="">No terminal yet</option>
              {repoSessions.map((session) => (
                <option key={session.id} value={session.id}>
                  {sessionLabel(session)}
                </option>
              ))}
            </select>
          </label>
          <button type="submit" className="plan-pane__primary" disabled={creating}>
            {creating ? "Starting…" : "Start plan"}
          </button>
        </form>

        <section className="plan-pane__plans" aria-label="Plans">
          <div className="plan-pane__section-heading">
            <Icon name="list" size={14} />
            <span>{loading ? "Loading plans…" : `${plans.length} plans`}</span>
          </div>
          {error ? <div className="plan-pane__error">{error}</div> : null}
          {!loading && plans.length === 0 ? (
            <div className="plan-pane__empty">No published plans in this repo yet.</div>
          ) : null}
          {plans.map((plan) => (
            <PlanCard key={plan.id} plan={plan} onOpen={openPlan} />
          ))}
        </section>
      </div>
    </div>
  );
}

function PlanDetail({
  repo,
  planId,
  active,
}: {
  repo: string;
  planId: string;
  active: boolean;
}) {
  const [plan, setPlan] = useState<PlanView | null>(null);
  const [events, setEvents] = useState<PlanEventView[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [newPhaseTitle, setNewPhaseTitle] = useState("");
  const [newPhaseDescription, setNewPhaseDescription] = useState("");
  const [attachPty, setAttachPty] = useState("");
  const [editMeta, setEditMeta] = useState(false);
  const [title, setTitle] = useState("");
  const [summary, setSummary] = useState("");
  const { sessions, refresh, ambientRevision } = useSessions(
    useShallow((store) => ({
      sessions: store.sessions,
      refresh: store.refresh,
      ambientRevision:
        store.plans.find((candidate) => candidate.id === planId)?.revision ??
        null,
    })),
  );

  const load = useCallback(async () => {
    try {
      const [nextPlan, nextEvents] = await Promise.all([
        getPlan(planId),
        getPlanEvents(planId),
      ]);
      setPlan(nextPlan);
      setEvents(nextEvents);
      setTitle(nextPlan.title);
      setSummary(nextPlan.summary);
      setError(null);
    } catch (err) {
      setError(messageOf(err));
    }
  }, [planId]);

  useEffect(() => {
    if (active) void load();
  }, [active, ambientRevision, load]);

  const mutate = useCallback(
    async (operation: () => Promise<PlanView>) => {
      setBusy(true);
      try {
        const next = await operation();
        setPlan(next);
        setTitle(next.title);
        setSummary(next.summary);
        setEvents(await getPlanEvents(planId));
        setError(null);
        await refresh();
      } catch (err) {
        setError(messageOf(err));
      } finally {
        setBusy(false);
      }
    },
    [planId, refresh],
  );

  const liveRepoSessions = useMemo(
    () =>
      sessions.filter(
        (session) =>
          session.repo === repo &&
          session.state === "live" &&
          !plan?.attachments.some((attachment) => attachment.pty_session_id === session.id),
      ),
    [plan?.attachments, repo, sessions],
  );
  const unfinishedPhaseCount =
    plan?.phases.filter(
      (phase) => phase.status !== "completed" && phase.status !== "skipped",
    ).length ?? 0;
  const saveMeta = useCallback(
    async (event: FormEvent) => {
      event.preventDefault();
      if (!plan) return;
      await mutate(() =>
        updatePlan(plan.id, { title: title.trim(), summary: summary.trim() }),
      );
      setEditMeta(false);
    },
    [mutate, plan, summary, title],
  );
  const setPlanStatus = useCallback(
    (status: PlanStatus, skipRemaining = false) => {
      if (!plan) return Promise.resolve();
      return mutate(() =>
        updatePlan(plan.id, {
          status,
          skip_remaining: skipRemaining,
        }),
      );
    },
    [mutate, plan],
  );
  const pausePlan = useCallback(() => {
    void setPlanStatus("paused");
  }, [setPlanStatus]);
  const resumePlan = useCallback(() => {
    void setPlanStatus("active");
  }, [setPlanStatus]);
  const completePlan = useCallback(() => {
    void setPlanStatus("completed", unfinishedPhaseCount > 0);
  }, [setPlanStatus, unfinishedPhaseCount]);
  const cancelPlan = useCallback(() => {
    void setPlanStatus("canceled");
  }, [setPlanStatus]);
  const addPhase = useCallback(
    async (event: FormEvent) => {
      event.preventDefault();
      if (!plan || !newPhaseTitle.trim()) return;
      await mutate(() =>
        addPlanPhase(plan.id, {
          title: newPhaseTitle.trim(),
          description: newPhaseDescription.trim(),
        }),
      );
      setNewPhaseTitle("");
      setNewPhaseDescription("");
    },
    [mutate, newPhaseDescription, newPhaseTitle, plan],
  );
  const attach = useCallback(async () => {
    if (!plan || !attachPty) return;
    await mutate(() => attachPlan(plan.id, attachPty));
    setAttachPty("");
  }, [attachPty, mutate, plan]);
  const onTitleChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => setTitle(event.target.value),
    [],
  );
  const onSummaryChange = useCallback(
    (event: ChangeEvent<HTMLTextAreaElement>) => setSummary(event.target.value),
    [],
  );
  const onNewPhaseTitleChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => setNewPhaseTitle(event.target.value),
    [],
  );
  const onNewPhaseDescriptionChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) =>
      setNewPhaseDescription(event.target.value),
    [],
  );
  const onAttachPtyChange = useCallback(
    (event: ChangeEvent<HTMLSelectElement>) => setAttachPty(event.target.value),
    [],
  );
  const beginMetaEdit = useCallback(() => setEditMeta(true), []);
  const cancelMetaEdit = useCallback(() => setEditMeta(false), []);

  if (!plan) {
    return (
      <div className="plan-pane plan-pane--loading" data-testid="plan-pane">
        {error ? <div className="plan-pane__error">{error}</div> : "Loading plan…"}
      </div>
    );
  }

  return (
    <div className="plan-pane" data-testid="plan-pane">
      <header className="plan-pane__header plan-pane__header--detail">
        <div className="plan-pane__title-block">
          <div className="plan-pane__eyebrow">{plan.repo_name} · published plan</div>
          {editMeta ? (
            <form className="plan-pane__meta-form" onSubmit={saveMeta}>
              <input
                aria-label="Plan title"
                value={title}
                onChange={onTitleChange}
                maxLength={160}
              />
              <textarea
                aria-label="Plan description"
                value={summary}
                onChange={onSummaryChange}
                rows={2}
                maxLength={1_000}
              />
              <div className="plan-pane__actions">
                <button type="submit" className="plan-pane__primary" disabled={busy}>
                  Save
                </button>
                <button type="button" onClick={cancelMetaEdit}>
                  Cancel
                </button>
              </div>
            </form>
          ) : (
            <>
              <h2>{plan.title}</h2>
              <p>{plan.summary || "No description"}</p>
            </>
          )}
        </div>
        <div className="plan-pane__header-actions">
          <span className={`plan-status plan-status--${plan.status}`}>
            {plan.status}
          </span>
          {!editMeta ? (
            <button type="button" onClick={beginMetaEdit}>
              Edit
            </button>
          ) : null}
          {plan.status === "active" ? (
            <button type="button" disabled={busy} onClick={pausePlan}>
              Pause
            </button>
          ) : plan.status === "paused" ? (
            <button type="button" disabled={busy} onClick={resumePlan}>
              Resume
            </button>
          ) : null}
          {plan.status === "active" || plan.status === "paused" ? (
            <>
              <button
                type="button"
                disabled={busy}
                onClick={completePlan}
              >
                {unfinishedPhaseCount > 0
                  ? `Complete & skip ${unfinishedPhaseCount}`
                  : "Complete"}
              </button>
              <button
                type="button"
                className="plan-pane__danger"
                disabled={busy}
                onClick={cancelPlan}
              >
                Cancel plan
              </button>
            </>
          ) : null}
        </div>
      </header>

      {error ? <div className="plan-pane__error">{error}</div> : null}

      <div className="plan-pane__detail">
        <main className="plan-pane__phases">
          <div className="plan-pane__section-heading">
            <Icon name="list" size={14} />
            <span>Phases</span>
            <span className="plan-pane__revision">revision {plan.revision}</span>
          </div>
          <PlanProgress phases={plan.phases} />
          <ol className="plan-pane__phase-list">
            {plan.phases.map((phase) => (
              <li key={phase.id}>
                <PhaseEditor
                  phase={phase}
                  planId={plan.id}
                  disabled={busy || plan.status === "completed" || plan.status === "canceled"}
                  onMutate={mutate}
                />
              </li>
            ))}
          </ol>
          {plan.status === "active" || plan.status === "paused" ? (
            <form className="plan-pane__add-phase" onSubmit={addPhase}>
              <input
                value={newPhaseTitle}
                onChange={onNewPhaseTitleChange}
                placeholder="Add a phase"
                aria-label="New phase title"
                maxLength={160}
              />
              <input
                value={newPhaseDescription}
                onChange={onNewPhaseDescriptionChange}
                placeholder="Short description"
                aria-label="New phase description"
                maxLength={1_000}
              />
              <button type="submit" disabled={busy || !newPhaseTitle.trim()}>
                <Icon name="plus" size={12} /> Add
              </button>
            </form>
          ) : null}
        </main>

        <aside className="plan-pane__aside">
          <section>
            <div className="plan-pane__section-heading">Attached terminals</div>
            {plan.attachments.length === 0 ? (
              <div className="plan-pane__muted">No terminals attached.</div>
            ) : (
              <div className="plan-pane__attachments">
                {plan.attachments.map((attachment) => (
                  <AttachmentRow
                    key={attachment.pty_session_id}
                    attachment={attachment}
                    sessions={sessions}
                    planId={plan.id}
                    disabled={busy}
                    onMutate={mutate}
                  />
                ))}
              </div>
            )}
            {plan.status === "active" || plan.status === "paused" ? (
              <div className="plan-pane__attach">
                <select
                  value={attachPty}
                  onChange={onAttachPtyChange}
                  aria-label="Terminal to attach"
                >
                  <option value="">Choose terminal</option>
                  {liveRepoSessions.map((session) => (
                    <option key={session.id} value={session.id}>
                      {sessionLabel(session)}
                    </option>
                  ))}
                </select>
                <button type="button" disabled={busy || !attachPty} onClick={attach}>
                  Attach
                </button>
              </div>
            ) : null}
          </section>

          <section>
            <details>
              <summary>History · {events.length}</summary>
              <ol className="plan-pane__history">
                {events.map((event) => (
                  <li key={event.id}>
                    <strong>{humanEvent(event.event_type)}</strong>
                    <span>{event.note || statusChange(event) || event.actor_kind}</span>
                    <time dateTime={event.created_at}>{relativeAge(event.created_at)}</time>
                  </li>
                ))}
              </ol>
            </details>
          </section>
        </aside>
      </div>
    </div>
  );
}

function PlanCard({
  plan,
  onOpen,
}: {
  plan: PlanView;
  onOpen: (planId: string) => void;
}) {
  const open = useCallback(() => onOpen(plan.id), [onOpen, plan.id]);
  return (
    <button type="button" className="plan-pane__plan-card" onClick={open}>
      <span className={`plan-status plan-status--${plan.status}`}>{plan.status}</span>
      <strong>{plan.title}</strong>
      <span>{plan.summary || "No description"}</span>
      <PlanProgress phases={plan.phases} />
    </button>
  );
}

function AttachmentRow({
  attachment,
  sessions,
  planId,
  disabled,
  onMutate,
}: {
  attachment: PlanView["attachments"][number];
  sessions: SessionView[];
  planId: string;
  disabled: boolean;
  onMutate: PlanMutation;
}) {
  const session = sessions.find(
    (candidate) => candidate.id === attachment.pty_session_id,
  );
  const detach = useCallback(() => {
    void onMutate(() => detachPlan(planId, attachment.pty_session_id));
  }, [attachment.pty_session_id, onMutate, planId]);
  return (
    <div>
      <span>
        {session
          ? sessionLabel(session)
          : attachment.pty_session_id.slice(0, 8)}
      </span>
      <button
        type="button"
        disabled={disabled}
        aria-label={`Detach ${attachment.pty_session_id}`}
        onClick={detach}
      >
        Detach
      </button>
    </div>
  );
}

function PhaseEditor({
  phase,
  planId,
  disabled,
  onMutate,
}: {
  phase: PlanPhaseView;
  planId: string;
  disabled: boolean;
  onMutate: PlanMutation;
}) {
  const [title, setTitle] = useState(phase.title);
  const [description, setDescription] = useState(phase.description);
  const [status, setStatus] = useState<PlanPhaseStatus>(phase.status);
  const [note, setNote] = useState(phase.status_note ?? "");
  const [position, setPosition] = useState(phase.position);

  useEffect(() => {
    setTitle(phase.title);
    setDescription(phase.description);
    setStatus(phase.status);
    setNote(phase.status_note ?? "");
    setPosition(phase.position);
  }, [phase]);

  const submit = useCallback(
    async (event: FormEvent) => {
      event.preventDefault();
      const patch: UpdatePlanPhaseInput = {};
      if (title.trim() !== phase.title) patch.title = title.trim();
      if (description.trim() !== phase.description) {
        patch.description = description.trim();
      }
      if (status !== phase.status) patch.status = status;
      if (note.trim() !== (phase.status_note ?? "")) {
        patch.status_note = note.trim();
      }
      if (position !== phase.position) patch.position = position;
      await onMutate(() => updatePlanPhase(planId, phase.id, patch));
    },
    [description, note, onMutate, phase, planId, position, status, title],
  );
  const onTitleChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => setTitle(event.target.value),
    [],
  );
  const onDescriptionChange = useCallback(
    (event: ChangeEvent<HTMLTextAreaElement>) =>
      setDescription(event.target.value),
    [],
  );
  const onPositionChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) =>
      setPosition(Number(event.target.value)),
    [],
  );
  const onStatusChange = useCallback(
    (event: ChangeEvent<HTMLSelectElement>) =>
      setStatus(event.target.value as PlanPhaseStatus),
    [],
  );
  const onNoteChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => setNote(event.target.value),
    [],
  );
  const dirty =
    title.trim() !== phase.title ||
    description.trim() !== phase.description ||
    status !== phase.status ||
    note.trim() !== (phase.status_note ?? "") ||
    position !== phase.position;
  const validPosition = Number.isInteger(position) && position >= 1;

  return (
    <form className={`plan-phase plan-phase--${phase.status}`} onSubmit={submit}>
      <span className="plan-phase__position">{phase.position}</span>
      <div className="plan-phase__fields">
        <input
          aria-label={`Phase ${phase.position} title`}
          value={title}
          onChange={onTitleChange}
          disabled={disabled}
          maxLength={160}
        />
        <textarea
          aria-label={`Phase ${phase.position} description`}
          value={description}
          onChange={onDescriptionChange}
          disabled={disabled}
          placeholder="Short description"
          rows={2}
          maxLength={1_000}
        />
        <div className="plan-phase__controls">
          <input
            type="number"
            min={1}
            aria-label={`Phase ${phase.position} position`}
            value={position}
            onChange={onPositionChange}
            disabled={disabled}
            title="Position"
          />
          <select
            aria-label={`Phase ${phase.position} status`}
            value={status}
            onChange={onStatusChange}
            disabled={disabled}
          >
            {PHASE_STATUSES.map((value) => (
              <option key={value} value={value}>
                {value.replace("_", " ")}
              </option>
            ))}
          </select>
          <input
            aria-label={`Phase ${phase.position} status note`}
            value={note}
            onChange={onNoteChange}
            disabled={disabled}
            placeholder="Status note"
            maxLength={1_000}
          />
          <button
            type="submit"
            disabled={disabled || !title.trim() || !validPosition || !dirty}
          >
            Save
          </button>
        </div>
      </div>
    </form>
  );
}

function PlanProgress({ phases }: { phases: PlanPhaseView[] }) {
  const done = phases.filter(
    (phase) => phase.status === "completed" || phase.status === "skipped",
  ).length;
  const blocked = phases.filter((phase) => phase.status === "blocked").length;
  const percentage = phases.length === 0 ? 0 : Math.round((done / phases.length) * 100);
  const progressStyle = useMemo(() => ({ width: `${percentage}%` }), [percentage]);
  return (
    <span className="plan-progress">
      <span>
        <i
          // eslint-disable-next-line local/no-inline-styles -- plan progress is data-driven
          style={progressStyle}
        />
      </span>
      <small>
        {done}/{phases.length} complete{blocked > 0 ? ` · ${blocked} blocked` : ""}
      </small>
    </span>
  );
}

const PHASE_STATUSES: PlanPhaseStatus[] = [
  "pending",
  "in_progress",
  "blocked",
  "completed",
  "skipped",
];

function parsePhases(value: string): NewPlanPhaseInput[] {
  return value
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const [title, ...description] = line.split("|");
      return {
        title: title!.trim(),
        description: description.join("|").trim(),
      };
    })
    .filter((phase) => phase.title.length > 0);
}

function sessionLabel(session: SessionView): string {
  return session.label?.trim() || session.id.slice(0, 8);
}

function humanEvent(value: string): string {
  return value.replaceAll("_", " ");
}

function statusChange(event: PlanEventView): string | null {
  if (!event.to_status) return null;
  return event.from_status
    ? `${event.from_status} → ${event.to_status}`
    : event.to_status;
}

function relativeAge(iso: string): string {
  const delta = Date.now() - new Date(iso).getTime();
  if (!Number.isFinite(delta)) return iso;
  const minutes = Math.max(0, Math.floor(delta / 60_000));
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : "Plan operation failed.";
}
