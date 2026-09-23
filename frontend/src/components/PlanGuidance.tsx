import { type ChangeEvent, useCallback } from "react";
import type { PlanGuidance } from "../api/types";
import { FileRefText } from "./common/FileRefText";

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
        Up to 10 principles and 10 assumptions, 500 characters each.
      </span>
    </fieldset>
  );
}

export function GuidanceContent({ value, repo }: { value: PlanGuidance; repo: string }) {
  if (!value.outcome && !value.principles.length && !value.assumptions.length) {
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
