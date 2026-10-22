import { type ChangeEvent, type MouseEvent, useCallback } from "react";
import type { PlanBoundary, PlanGuidance } from "../api/types";
import { FileRefText } from "./common/FileRefText";

const BOUNDARY_SOURCES = {
  user_requirement: "User requirement",
  user_correction: "User correction",
  agent_proposal: "Agent proposal",
} as const;

export function GuidanceFields({
  value,
  onChange,
  disabled = false,
}: {
  value: PlanGuidance;
  onChange: (value: PlanGuidance) => void;
  disabled?: boolean;
}) {
  const outcome = useCallback(
    (event: ChangeEvent<HTMLTextAreaElement>) => {
      onChange({ ...value, outcome: event.target.value });
    },
    [onChange, value],
  );
  const principles = useCallback(
    (event: ChangeEvent<HTMLTextAreaElement>) => {
      onChange({ ...value, principles: event.target.value.split("\n") });
    },
    [onChange, value],
  );
  const assumptions = useCallback(
    (event: ChangeEvent<HTMLTextAreaElement>) => {
      onChange({ ...value, assumptions: event.target.value.split("\n") });
    },
    [onChange, value],
  );
  const addBoundary = useCallback(() => {
    onChange({ ...value, boundaries: [...value.boundaries, { source: "agent_proposal", text: "" }] });
  }, [onChange, value]);
  const boundaryText = useCallback((event: ChangeEvent<HTMLTextAreaElement>) => {
    const index = Number(event.currentTarget.dataset.index);
    onChange({ ...value, boundaries: value.boundaries.map((item, i) => i === index ? { ...item, text: event.target.value } : item) });
  }, [onChange, value]);
  const boundarySource = useCallback((event: ChangeEvent<HTMLSelectElement>) => {
    const index = Number(event.currentTarget.dataset.index);
    const source = event.target.value as PlanBoundary["source"];
    onChange({ ...value, boundaries: value.boundaries.map((item, i) => i === index ? { ...item, source } : item) });
  }, [onChange, value]);
  const removeBoundary = useCallback((event: MouseEvent<HTMLButtonElement>) => {
    const index = Number(event.currentTarget.dataset.index);
    onChange({ ...value, boundaries: value.boundaries.filter((_, i) => i !== index) });
  }, [onChange, value]);
  return (
    <fieldset className="plan-guidance__fields" disabled={disabled}>
      <legend>Guidance</legend>
      <label>
        Outcome
        <textarea
          value={value.outcome}
          onChange={outcome}
          rows={2}
          maxLength={1_000}
          placeholder="What should improve for the user, and what would demonstrate success?"
        />
      </label>
      <fieldset className="plan-guidance__fields">
        <legend>Boundaries</legend>
        <span className="plan-modal__hint">
          Excluded scope, rejected approaches, and reasons to reassess. Preserve the source;
          an agent proposal is not a user requirement.
        </span>
        {value.boundaries.map((boundary, index) => (
          <div key={index} className="plan-guidance__fields">
            <label>
              Boundary {index + 1} source
              <select value={boundary.source} data-index={index} onChange={boundarySource}>
                {Object.entries(BOUNDARY_SOURCES).map(([source, label]) => (
                  <option key={source} value={source}>{label}</option>
                ))}
              </select>
            </label>
            <label>
              Boundary {index + 1}
              <textarea value={boundary.text} data-index={index} onChange={boundaryText} rows={2} maxLength={500} required />
            </label>
            <button type="button" data-index={index} onClick={removeBoundary} aria-label={`Remove boundary ${index + 1}`}>Remove boundary</button>
          </div>
        ))}
        <button type="button" onClick={addBoundary} disabled={value.boundaries.length >= 10}>Add boundary</button>
      </fieldset>
      <label>
        Principles
        <textarea
          value={value.principles.join("\n")}
          onChange={principles}
          rows={3}
          placeholder="Concrete decision rules and constraints, with reasons. One per line."
        />
      </label>
      <label>
        Assumptions
        <textarea
          value={value.assumptions.join("\n")}
          onChange={assumptions}
          rows={3}
          placeholder="Beliefs that new evidence might invalidate. One per line."
        />
      </label>
      <span className="plan-modal__hint">
        Up to 10 entries per list, 500 characters each. Record user corrections here
        and revise affected phases before continuing.
      </span>
    </fieldset>
  );
}

export function GuidanceContent({ value, repo }: { value: PlanGuidance; repo: string }) {
  const boundaries = value.boundaries ?? [];
  if (!value.outcome && !value.principles.length && !value.assumptions.length && !boundaries.length) {
    return <p className="plan-modal__muted">No guidance set.</p>;
  }
  return (
    <dl className="plan-guidance__content">
      {value.outcome ? (
        <>
          <dt>Outcome</dt>
          <dd><FileRefText text={value.outcome} repo={repo} /></dd>
        </>
      ) : null}
      {boundaries.length ? (
        <>
          <dt>Boundaries</dt>
          <dd>
            <ul>
              {boundaries.map((item, index) => (
                <li key={index}>
                  <strong>{BOUNDARY_SOURCES[item.source]}: </strong>
                  <FileRefText text={item.text} repo={repo} />
                </li>
              ))}
            </ul>
          </dd>
        </>
      ) : null}
      {value.principles.length ? (
        <>
          <dt>Principles</dt>
          <dd>
            <ul>
              {value.principles.map((item, index) => (
                <li key={index}><FileRefText text={item} repo={repo} /></li>
              ))}
            </ul>
          </dd>
        </>
      ) : null}
      {value.assumptions.length ? (
        <>
          <dt>Assumptions</dt>
          <dd>
            <ul>
              {value.assumptions.map((item, index) => (
                <li key={index}><FileRefText text={item} repo={repo} /></li>
              ))}
            </ul>
          </dd>
        </>
      ) : null}
    </dl>
  );
}
