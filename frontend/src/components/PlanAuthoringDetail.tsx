import { useState, type FormEvent } from "react";
import type { AuthoredPlan, PlanAuthoringPatch, PlanPhaseView, PlanView } from "../api/types";

type SaveAuthoring = (patch: PlanAuthoringPatch) => Promise<boolean>;

interface AuthoringProps {
  plan: PlanView;
  authoring: AuthoredPlan;
  canEdit: boolean;
  onSave: SaveAuthoring;
}

export function PlanAuthoringDetail({ plan, authoring, canEdit, onSave }: AuthoringProps) {
  const entries = authoring.entries.filter((entry) => !["boundary", "principle", "assumption"].includes(entry.kind));
  return (
    <section aria-label="Plan authoring" className="plan-guidance">
      <DetailEditor revision={authoring.revision} value={authoring.technical_details} canEdit={canEdit} onSave={onSave} />
      {entries.length > 0 && (
        <details open>
          <summary>Plan entries</summary>
          <ul>
            {entries.map((entry) => (
              <li key={entry.id}>
                <strong>{entry.kind}{entry.retired ? " (retired)" : ""}</strong> {entry.text}
                {entry.generalization && <p>Generalized behavior: {entry.generalization}</p>}
                {entry.test && <p>Test: {entry.test}</p>}
                {entry.result && <p>Result: {entry.result.replaceAll("_", " ")}</p>}
                {entry.evidence && <p>Evidence: {entry.evidence}</p>}
                {entry.rationale && <p>{entry.rationale}</p>}
                {entry.source && <small>{entry.source}</small>}
              </li>
            ))}
          </ul>
        </details>
      )}
      {plan.inherited_authoring?.some((ancestor) => ancestor.entries.some((entry) => !entry.retired && entry.kind === "requirement")) && (
        <details>
          <summary>Inherited requirements</summary>
          <ul>
            {plan.inherited_authoring.flatMap((ancestor) => ancestor.entries.filter((entry) => !entry.retired && entry.kind === "requirement").map((entry) => <li key={entry.id}>{entry.text}</li>))}
          </ul>
        </details>
      )}
    </section>
  );
}

export function PhaseAuthoring({ phase, plan, authoring, canEdit, onSave }: AuthoringProps & { phase: PlanPhaseView }) {
  const detail = authoring.phases[phase.id];
  const knownEntries = [...authoring.entries, ...(plan.inherited_authoring ?? []).flatMap((ancestor) => ancestor.entries)];
  const review = (authoring.needs_review?.[phase.id] ?? []).map((id) => knownEntries.find((entry) => entry.id === id)?.text ?? id);
  return (
    <details className="plan-modal__phase-authoring">
      <summary>Implementation and acceptance</summary>
      <DetailEditor revision={authoring.revision} value={detail?.implementation ?? ""} acceptance={detail?.acceptance ?? ""} phaseId={phase.id} canEdit={canEdit && !phase.retired_at} onSave={onSave} />
      {detail?.successors.length ? <p>Successors: {detail.successors.map((id) => plan.phases.find((candidate) => candidate.id === id)?.title ?? id).join(", ")}</p> : null}
      {detail?.follows_up?.length ? <p>Follows up: {detail.follows_up.map((id) => plan.phases.find((candidate) => candidate.id === id)?.title ?? id).join(", ")}</p> : null}
      {review.length > 0 && <p>Needs review: {review.join(", ")}</p>}
      {detail?.evidence.map((evidence, index) => (
        <p key={`${evidence.revision}-${index}`}>Evidence at revision {evidence.revision}: {evidence.observation} {evidence.reference}</p>
      ))}
    </details>
  );
}

function DetailEditor({ value, acceptance, phaseId, revision, canEdit, onSave }: {
  value: string;
  acceptance?: string;
  phaseId?: string;
  revision: number;
  canEdit: boolean;
  onSave: SaveAuthoring;
}) {
  const [draft, setDraft] = useState<{ revision: number; text: string; acceptance: string } | null>(null);
  const label = phaseId ? "Implementation" : "Technical details";
  const save = async (event: FormEvent) => {
    event.preventDefault();
    if (!draft) return;
    const operation = phaseId
      ? { op: "edit_phase" as const, id: phaseId, implementation: draft.text, acceptance: draft.acceptance }
      : { op: "edit_plan" as const, technical_details: draft.text };
    if (await onSave({ revision: draft.revision, operations: [operation] })) setDraft(null);
  };
  if (draft) {
    return (
      <form className="plan-modal__meta-form" onSubmit={save}>
        <label>{label}<textarea value={draft.text} onChange={(event) => setDraft({ ...draft, text: event.target.value })} /></label>
        {phaseId && <label>Acceptance<textarea value={draft.acceptance} onChange={(event) => setDraft({ ...draft, acceptance: event.target.value })} /></label>}
        <div className="plan-modal__actions">
          <button type="submit" disabled={!canEdit}>Save details</button>
          <button type="button" onClick={() => setDraft(null)}>Cancel</button>
        </div>
      </form>
    );
  }
  return (
    <div>
      {value && <p className="plan-authoring-text">{value}</p>}
      {acceptance && <p className="plan-authoring-text"><strong>Acceptance</strong> {acceptance}</p>}
      {canEdit && <button type="button" onClick={() => setDraft({ revision, text: value, acceptance: acceptance ?? "" })}>Edit {label.toLowerCase()}</button>}
    </div>
  );
}
