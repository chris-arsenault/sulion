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
  branchPlan,
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
  PlanAncestorView,
  PlanBranchView,
  PlanEventView,
  PlanPhaseStatus,
  PlanPhaseView,
  PlanStatus,
  PlanView,
  SessionView,
  UpdatePlanPhaseInput,
} from "../api/types";
import { Icon } from "../icons";
import type { Maybe } from "../lib/types";
import { useSessions } from "../state/SessionStore";
import { FileRefText } from "./common/FileRefText";
import { Overlay } from "./ui";
import "./PlanModal.css";

interface PlanModalProps {
  open: boolean;
  repo: string | null;
  planId?: string;
  onClose: () => void;
}

type PlanMutation = (operation: () => Promise<PlanView>) => Promise<void>;

/** Published plans live in a compact modal, not a workspace tab. It opens
 * either on a repo's plan index (list + create) or straight onto one plan's
 * detail; navigation between the two is local modal state. */
export function PlanModal({ open, repo, planId, onClose }: PlanModalProps) {
  const [selectedPlanId, setSelectedPlanId] = useState<Maybe<string>>(planId);
  const [detailTitle, setDetailTitle] = useState<string | null>(null);

  // Re-seed whenever the modal is (re)opened against a new target.
  useEffect(() => {
    if (open) {
      setSelectedPlanId(planId);
      setDetailTitle(null);
    }
  }, [open, planId, repo]);

  const backToIndex = useCallback(() => {
    setSelectedPlanId(undefined);
    setDetailTitle(null);
  }, []);
  const openDetail = useCallback((nextPlanId: string) => {
    setSelectedPlanId(nextPlanId);
    setDetailTitle(null);
  }, []);

  if (!open || !repo) return null;

  const inDetail = Boolean(selectedPlanId);

  return (
    <Overlay
      open={open}
      onClose={onClose}
      modal
      className="plan-modal"
      width={620}
      maxWidth="min(94vw, 620px)"
      maxHeight="82vh"
      leading={
        inDetail ? (
          <button
            type="button"
            className="plan-modal__back"
            onClick={backToIndex}
            aria-label="Back to plans"
          >
            <Icon name="chevron-right" size={16} />
          </button>
        ) : (
          <Icon name="list-checks" size={16} />
        )
      }
      title={inDetail ? detailTitle ?? "Plan" : "Published plans"}
      subtitle={inDetail ? `${repo} · plan` : repo}
    >
      {selectedPlanId ? (
        <PlanDetail
          repo={repo}
          planId={selectedPlanId}
          onTitle={setDetailTitle}
          onOpenPlan={openDetail}
        />
      ) : (
        <PlanIndex repo={repo} onOpenPlan={openDetail} />
      )}
    </Overlay>
  );
}

function PlanIndex({
  repo,
  onOpenPlan,
}: {
  repo: string;
  onOpenPlan: (planId: string) => void;
}) {
  const [plans, setPlans] = useState<PlanView[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showClosed, setShowClosed] = useState(false);
  const [showCreate, setShowCreate] = useState(false);
  const [title, setTitle] = useState("");
  const [summary, setSummary] = useState("");
  const [phaseText, setPhaseText] = useState("");
  const [attachPty, setAttachPty] = useState("");
  const [creating, setCreating] = useState(false);
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
  const orderedPlans = useMemo(() => nestPlans(plans), [plans]);
  const onShowClosedChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => setShowClosed(event.target.checked),
    [],
  );
  const toggleCreate = useCallback(() => setShowCreate((open) => !open), []);
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
    void load();
  }, [load, planRevisionKey]);

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
        setShowCreate(false);
        setError(null);
        await refresh();
        onOpenPlan(created.id);
      } catch (err) {
        setError(messageOf(err));
      } finally {
        setCreating(false);
      }
    },
    [attachPty, onOpenPlan, phaseText, refresh, repo, summary, title],
  );

  // The create form defaults open when the repo has no plans yet, and can be
  // toggled otherwise. It stays put while the list loads so it never flickers.
  const formOpen = showCreate || plans.length === 0;

  return (
    <div className="plan-modal__index">
      <div className="plan-modal__toolbar">
        <button
          type="button"
          className="plan-modal__primary"
          onClick={toggleCreate}
          aria-expanded={formOpen}
        >
          <Icon name="plus" size={12} /> New plan
        </button>
        <label className="plan-modal__closed-toggle">
          <input type="checkbox" checked={showClosed} onChange={onShowClosedChange} />
          Show closed
        </label>
      </div>

      {error ? <div className="plan-modal__error">{error}</div> : null}

      {formOpen ? (
        <form className="plan-modal__create" onSubmit={onCreate}>
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
              rows={4}
            />
            <span className="plan-modal__hint">
              One phase per line. Add a description after <code>|</code>.
            </span>
          </label>
          <label>
            Attach to terminal
            <select value={attachPty} onChange={onAttachPtyChange}>
              <option value="">No terminal yet</option>
              {repoSessions.map((session) => (
                <option key={session.id} value={session.id}>
                  {sessionLabel(session)}
                </option>
              ))}
            </select>
          </label>
          <div className="plan-modal__create-actions">
            <button type="submit" className="plan-modal__primary" disabled={creating}>
              {creating ? "Starting…" : "Start plan"}
            </button>
          </div>
        </form>
      ) : null}

      <div className="plan-modal__list" aria-label="Plans">
        {loading && plans.length === 0 ? (
          <div className="plan-modal__muted">Loading plans…</div>
        ) : null}
        {!loading && plans.length === 0 ? (
          <div className="plan-modal__empty">No published plans in this repo yet.</div>
        ) : null}
        {orderedPlans.map((plan) => (
          <PlanRow key={plan.id} plan={plan} onOpen={onOpenPlan} />
        ))}
      </div>
    </div>
  );
}

function PlanDetail({
  repo,
  planId,
  onTitle,
  onOpenPlan,
}: {
  repo: string;
  planId: string;
  onTitle: (title: string) => void;
  onOpenPlan: (planId: string) => void;
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
  // Position of the phase whose branch form is open, or null for none.
  const [branchAnchor, setBranchAnchor] = useState<number | null>(null);
  const [branchTitle, setBranchTitle] = useState("");
  const [branchCovers, setBranchCovers] = useState("");
  const [branchPhaseText, setBranchPhaseText] = useState("");
  const { sessions, refresh, ambientRevision } = useSessions(
    useShallow((store) => ({
      sessions: store.sessions,
      refresh: store.refresh,
      ambientRevision:
        store.plans.find((candidate) => candidate.id === planId)?.revision ?? null,
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
      onTitle(nextPlan.title);
      setError(null);
    } catch (err) {
      setError(messageOf(err));
    }
  }, [onTitle, planId]);

  useEffect(() => {
    void load();
  }, [ambientRevision, load]);

  const mutate = useCallback(
    async (operation: () => Promise<PlanView>) => {
      setBusy(true);
      try {
        const next = await operation();
        setPlan(next);
        setTitle(next.title);
        setSummary(next.summary);
        onTitle(next.title);
        setEvents(await getPlanEvents(planId));
        setError(null);
        await refresh();
      } catch (err) {
        setError(messageOf(err));
      } finally {
        setBusy(false);
      }
    },
    [onTitle, planId, refresh],
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
      await mutate(() => updatePlan(plan.id, { title: title.trim(), summary: summary.trim() }));
      setEditMeta(false);
    },
    [mutate, plan, summary, title],
  );
  const setPlanStatus = useCallback(
    (status: PlanStatus, skipRemaining = false) => {
      if (!plan) return Promise.resolve();
      return mutate(() =>
        updatePlan(plan.id, { status, skip_remaining: skipRemaining }),
      );
    },
    [mutate, plan],
  );
  const pausePlan = useCallback(() => void setPlanStatus("paused"), [setPlanStatus]);
  const resumePlan = useCallback(() => void setPlanStatus("active"), [setPlanStatus]);
  const completePlan = useCallback(
    () => void setPlanStatus("completed", unfinishedPhaseCount > 0),
    [setPlanStatus, unfinishedPhaseCount],
  );
  const cancelPlan = useCallback(() => void setPlanStatus("canceled"), [setPlanStatus]);
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
  const openBranchForm = useCallback((position: number) => {
    setBranchAnchor(position);
    setBranchCovers(String(position));
    setBranchTitle("");
    setBranchPhaseText("");
  }, []);
  const closeBranchForm = useCallback(() => setBranchAnchor(null), []);
  // Creating a branch opens it: the branch is where the work continues.
  const createBranch = useCallback(
    async (event: FormEvent) => {
      event.preventDefault();
      if (!plan || !branchTitle.trim()) return;
      const covers = branchCovers
        .split(/[,\s]+/)
        .map((value) => value.trim())
        .filter(Boolean);
      setBusy(true);
      try {
        const branch = await branchPlan(plan.id, {
          title: branchTitle.trim(),
          phases: parsePhases(branchPhaseText),
          parent_phase_refs: covers,
        });
        setBranchAnchor(null);
        setError(null);
        await refresh();
        onOpenPlan(branch.id);
      } catch (err) {
        setError(messageOf(err));
      } finally {
        setBusy(false);
      }
    },
    [branchCovers, branchPhaseText, branchTitle, onOpenPlan, plan, refresh],
  );
  const returnToParent = useCallback(
    async (status: PlanStatus) => {
      if (!plan?.parent_plan_id) return;
      const parentId = plan.parent_plan_id;
      setBusy(true);
      try {
        await updatePlan(plan.id, {
          status,
          skip_remaining: status === "completed" && unfinishedPhaseCount > 0,
        });
        setError(null);
        await refresh();
        onOpenPlan(parentId);
      } catch (err) {
        setError(messageOf(err));
      } finally {
        setBusy(false);
      }
    },
    [onOpenPlan, plan, refresh, unfinishedPhaseCount],
  );
  const returnCompleted = useCallback(
    () => void returnToParent("completed"),
    [returnToParent],
  );
  const returnCanceled = useCallback(
    () => void returnToParent("canceled"),
    [returnToParent],
  );
  const onBranchTitleChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => setBranchTitle(event.target.value),
    [],
  );
  const onBranchCoversChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => setBranchCovers(event.target.value),
    [],
  );
  const onBranchPhaseTextChange = useCallback(
    (event: ChangeEvent<HTMLTextAreaElement>) =>
      setBranchPhaseText(event.target.value),
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
  const onNewPhaseTitleChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => setNewPhaseTitle(event.target.value),
    [],
  );
  const onNewPhaseDescriptionChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => setNewPhaseDescription(event.target.value),
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
      <div className="plan-modal__loading">
        {error ? <div className="plan-modal__error">{error}</div> : "Loading plan…"}
      </div>
    );
  }

  const editable = plan.status === "active" || plan.status === "paused";
  const locked = plan.status === "completed" || plan.status === "canceled";

  return (
    <div className="plan-modal__detail">
      {error ? <div className="plan-modal__error">{error}</div> : null}

      {plan.ancestors.length > 0 ? (
        <nav className="plan-modal__breadcrumb" aria-label="Plan ancestry">
          {plan.ancestors.map((ancestor) => (
            <PlanCrumb key={ancestor.id} plan={ancestor} onOpen={onOpenPlan} />
          ))}
          <span className="plan-modal__crumb plan-modal__crumb--current">
            {plan.title}
          </span>
        </nav>
      ) : null}

      <div className="plan-modal__meta">
        {editMeta ? (
          <form className="plan-modal__meta-form" onSubmit={saveMeta}>
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
            <div className="plan-modal__actions">
              <button type="submit" className="plan-modal__primary" disabled={busy}>
                Save
              </button>
              <button type="button" onClick={cancelMetaEdit}>
                Cancel
              </button>
            </div>
          </form>
        ) : (
          <p className="plan-modal__summary">
            {plan.summary ? (
              <FileRefText text={plan.summary} repo={repo} />
            ) : (
              "No description"
            )}
          </p>
        )}
        <div className="plan-modal__status-row">
          <span className={`plan-status plan-status--${plan.status}`}>{plan.status}</span>
          <span className="plan-modal__revision">revision {plan.revision}</span>
          <span className="plan-modal__status-actions">
            {!editMeta && !locked ? (
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
            {editable && plan.parent_plan_id ? (
              <>
                <button
                  type="button"
                  className="plan-modal__primary"
                  disabled={busy}
                  onClick={returnCompleted}
                >
                  {unfinishedPhaseCount > 0
                    ? `Return & skip ${unfinishedPhaseCount}`
                    : "Return to parent"}
                </button>
                <button
                  type="button"
                  className="plan-modal__danger"
                  disabled={busy}
                  onClick={returnCanceled}
                >
                  Abandon branch
                </button>
              </>
            ) : editable ? (
              <>
                <button type="button" disabled={busy} onClick={completePlan}>
                  {unfinishedPhaseCount > 0
                    ? `Complete & skip ${unfinishedPhaseCount}`
                    : "Complete"}
                </button>
                <button
                  type="button"
                  className="plan-modal__danger"
                  disabled={busy}
                  onClick={cancelPlan}
                >
                  Cancel
                </button>
              </>
            ) : null}
          </span>
        </div>
        <PlanProgress phases={plan.phases} />
      </div>

      <ol className="plan-modal__phase-list">
        {plan.phases.map((phase) => (
          <li key={phase.id}>
            <PhaseRow
              phase={phase}
              planId={plan.id}
              repo={repo}
              disabled={busy || locked}
              onMutate={mutate}
              canBranch={editable}
              onBranch={openBranchForm}
            />
            {plan.branches
              .filter((branch) => branch.anchor_phase_ids.includes(phase.id))
              .map((branch) => (
                <BranchChip key={branch.id} branch={branch} onOpen={onOpenPlan} />
              ))}
            {branchAnchor === phase.position ? (
              <form className="plan-modal__branch-form" onSubmit={createBranch}>
                <input
                  value={branchTitle}
                  onChange={onBranchTitleChange}
                  placeholder="Sub-plan title"
                  aria-label="Sub-plan title"
                  maxLength={160}
                />
                <input
                  value={branchCovers}
                  onChange={onBranchCoversChange}
                  placeholder="Covers phases, e.g. 4,5,6"
                  aria-label="Parent phases covered"
                />
                <textarea
                  value={branchPhaseText}
                  onChange={onBranchPhaseTextChange}
                  placeholder={"One phase per line\nTitle|Description|size"}
                  aria-label="Sub-plan phases"
                  rows={3}
                />
                <div className="plan-modal__actions">
                  <button
                    type="submit"
                    className="plan-modal__primary"
                    disabled={busy || !branchTitle.trim() || !branchPhaseText.trim()}
                  >
                    Branch
                  </button>
                  <button type="button" onClick={closeBranchForm}>
                    Cancel
                  </button>
                </div>
              </form>
            ) : null}
          </li>
        ))}
      </ol>

      {editable ? (
        <form className="plan-modal__add-phase" onSubmit={addPhase}>
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

      <section className="plan-modal__section">
        <div className="plan-modal__section-heading">
          <Icon name="link" size={12} />
          <span>Attached terminals</span>
        </div>
        {plan.attachments.length === 0 ? (
          <div className="plan-modal__muted">No terminals attached.</div>
        ) : (
          <div className="plan-modal__attachments">
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
        {editable ? (
          <div className="plan-modal__attach">
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

      <details className="plan-modal__history">
        <summary>History · {events.length}</summary>
        <ol>
          {events.map((event) => (
            <li key={event.id}>
              <strong>{humanEvent(event.event_type)}</strong>
              <span>{event.note || statusChange(event) || event.actor_kind}</span>
              <time dateTime={event.created_at}>{relativeAge(event.created_at)}</time>
            </li>
          ))}
        </ol>
      </details>
    </div>
  );
}

function PlanCrumb({
  plan,
  onOpen,
}: {
  plan: PlanAncestorView;
  onOpen: (planId: string) => void;
}) {
  const open = useCallback(() => onOpen(plan.id), [onOpen, plan.id]);
  return (
    <button type="button" className="plan-modal__crumb" onClick={open}>
      {plan.title}
    </button>
  );
}

function BranchChip({
  branch,
  onOpen,
}: {
  branch: PlanBranchView;
  onOpen: (planId: string) => void;
}) {
  const open = useCallback(() => onOpen(branch.id), [branch.id, onOpen]);
  return (
    <button type="button" className="plan-modal__branch-chip" onClick={open}>
      <Icon name="git-branch" size={12} />
      <span className={`plan-status plan-status--${branch.status}`}>
        {branch.status}
      </span>
      <strong>{branch.title}</strong>
      <span className="plan-modal__muted">
        {branch.completed_phases}/{branch.total_phases}
      </span>
    </button>
  );
}

/** Roots in server order, each followed by its own subtree depth-first, so a
 * branch reads as belonging to the plan above it rather than as a peer. */
function nestPlans(plans: PlanView[]): PlanView[] {
  const children = new Map<string, PlanView[]>();
  for (const plan of plans) {
    if (!plan.parent_plan_id) continue;
    const siblings = children.get(plan.parent_plan_id);
    if (siblings) siblings.push(plan);
    else children.set(plan.parent_plan_id, [plan]);
  }
  const out: PlanView[] = [];
  const visit = (plan: PlanView) => {
    out.push(plan);
    for (const child of children.get(plan.id) ?? []) visit(child);
  };
  const known = new Set(plans.map((plan) => plan.id));
  for (const plan of plans) {
    // A branch whose parent is filtered out of this list still needs a home.
    if (!plan.parent_plan_id || !known.has(plan.parent_plan_id)) visit(plan);
  }
  return out;
}

function PlanRow({
  plan,
  onOpen,
}: {
  plan: PlanView;
  onOpen: (planId: string) => void;
}) {
  const open = useCallback(() => onOpen(plan.id), [onOpen, plan.id]);
  return (
    <button
      type="button"
      // Depth is unbounded; indentation stops at 4 so a deep tree stays legible
      // in a 620px modal.
      className={`plan-modal__plan-row plan-modal__plan-row--depth-${Math.min(plan.depth, 4)}`}
      onClick={open}
    >
      <span className="plan-modal__plan-main">
        {plan.depth > 0 ? <Icon name="git-branch" size={12} /> : null}
        <span className={`plan-status plan-status--${plan.status}`}>{plan.status}</span>
        <strong>{plan.title}</strong>
      </span>
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
    <div className="plan-modal__attachment">
      <span>
        {session ? sessionLabel(session) : attachment.pty_session_id.slice(0, 8)}
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

function PhaseRow({
  phase,
  planId,
  repo,
  disabled,
  onMutate,
  canBranch,
  onBranch,
}: {
  phase: PlanPhaseView;
  planId: string;
  repo: string;
  disabled: boolean;
  onMutate: PlanMutation;
  canBranch: boolean;
  onBranch: (position: number) => void;
}) {
  const [editing, setEditing] = useState(false);
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
      if (description.trim() !== phase.description) patch.description = description.trim();
      if (status !== phase.status) patch.status = status;
      if (note.trim() !== (phase.status_note ?? "")) patch.status_note = note.trim();
      if (position !== phase.position) patch.position = position;
      await onMutate(() => updatePlanPhase(planId, phase.id, patch));
      setEditing(false);
    },
    [description, note, onMutate, phase, planId, position, status, title],
  );
  const beginEdit = useCallback(() => setEditing(true), []);
  const branch = useCallback(() => onBranch(phase.position), [onBranch, phase.position]);
  const cancelEdit = useCallback(() => {
    setTitle(phase.title);
    setDescription(phase.description);
    setStatus(phase.status);
    setNote(phase.status_note ?? "");
    setPosition(phase.position);
    setEditing(false);
  }, [phase]);
  const onTitleChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => setTitle(event.target.value),
    [],
  );
  const onDescriptionChange = useCallback(
    (event: ChangeEvent<HTMLTextAreaElement>) => setDescription(event.target.value),
    [],
  );
  const onPositionChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => setPosition(Number(event.target.value)),
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

  if (!editing) {
    return (
      <div className={`plan-phase plan-phase--${phase.status}`}>
        <span className="plan-phase__position">{phase.position}</span>
        <div className="plan-phase__body">
          <span className="plan-phase__title">{phase.title}</span>
          {phase.description ? (
            <span className="plan-phase__desc">
              <FileRefText text={phase.description} repo={repo} />
            </span>
          ) : null}
          {phase.status_note ? (
            <span className="plan-phase__note">
              <FileRefText text={phase.status_note} repo={repo} />
            </span>
          ) : null}
        </div>
        <span className="plan-phase__tail">
          <span className="plan-phase__status">{phase.status.replace("_", " ")}</span>
          {canBranch && !disabled ? (
            <button
              type="button"
              className="plan-phase__edit"
              aria-label={`Branch from phase ${phase.position}`}
              title="Open a sub-plan under this phase"
              onClick={branch}
            >
              <Icon name="git-branch" size={12} />
            </button>
          ) : null}
          {!disabled ? (
            <button
              type="button"
              className="plan-phase__edit"
              aria-label={`Edit phase ${phase.position}`}
              onClick={beginEdit}
            >
              <Icon name="pencil" size={12} />
            </button>
          ) : null}
        </span>
      </div>
    );
  }

  return (
    <form className={`plan-phase plan-phase--${phase.status} plan-phase--editing`} onSubmit={submit}>
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
        </div>
        <div className="plan-phase__edit-actions">
          <button
            type="submit"
            disabled={disabled || !title.trim() || !validPosition || !dirty}
          >
            Save
          </button>
          <button type="button" onClick={cancelEdit} disabled={disabled}>
            Cancel
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
