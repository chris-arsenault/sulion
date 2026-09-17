// Confirmation dialog for a model change the user did not ask for. Both
// harnesses can move a session onto another model on their own: Codex
// applies new thread settings between turns, Claude Code falls back
// mid-turn. Neither records a reason, so the dialog shows the evidence
// the transcript holds around the switch and asks the user to decide
// whether the session keeps running on the new model. Until they do, the
// control process stops any turn that runs on it.

import { useCallback, useState } from "react";

import { acknowledgeModelSwitch } from "../../api/client";
import type {
  CodexRateLimits,
  ModelSwitchView,
  SessionView,
} from "../../api/types";
import { Icon } from "../../icons";
import { Overlay } from "../ui";
import "./ModelSwitchModal.css";

/** The timeline pane's slot for a pending switch: the dialog while it is
 * shown, a banner that reopens it once the user closed it without
 * deciding. Hiding is per switch id, so a later switch opens fresh. The
 * dialog is fixed-positioned, so both render from the banner's place in
 * the pane. */
export function ModelSwitchGuard({
  sessionId,
  session,
  active,
  onRefresh,
}: {
  sessionId: string;
  session: SessionView;
  active: boolean;
  onRefresh: () => Promise<void>;
}) {
  const pending = session.pending_model_switch ?? null;
  const [hiddenId, setHiddenId] = useState<string | null>(null);
  const hide = useCallback(() => {
    if (pending) setHiddenId(pending.id);
  }, [pending]);
  const review = useCallback(() => setHiddenId(null), []);
  if (!pending) return null;
  const hidden = hiddenId === pending.id;
  return (
    <>
      {hidden && <ModelSwitchBanner modelSwitch={pending} onReview={review} />}
      <ModelSwitchModal
        open={active && !hidden}
        sessionId={sessionId}
        session={session}
        modelSwitch={pending}
        onHide={hide}
        onAcknowledged={onRefresh}
      />
    </>
  );
}

export function ModelSwitchModal({
  open,
  sessionId,
  session,
  modelSwitch,
  onHide,
  onAcknowledged,
}: {
  open: boolean;
  sessionId: string;
  session: SessionView;
  modelSwitch: ModelSwitchView;
  /** Close without deciding. The switch stays pending and the pane keeps
   * a banner that reopens the dialog. */
  onHide: () => void;
  onAcknowledged: () => Promise<void>;
}) {
  const [busy, setBusy] = useState<"adopt" | "dismiss" | null>(null);
  const [error, setError] = useState<string | null>(null);

  const acknowledge = useCallback(
    async (adopt: boolean) => {
      setBusy(adopt ? "adopt" : "dismiss");
      setError(null);
      try {
        await acknowledgeModelSwitch(sessionId, modelSwitch.id, adopt);
        await onAcknowledged();
      } catch (err) {
        setError(err instanceof Error ? err.message : "acknowledgement failed");
      } finally {
        setBusy(null);
      }
    },
    [modelSwitch.id, onAcknowledged, sessionId],
  );
  const adopt = useCallback(() => void acknowledge(true), [acknowledge]);
  const dismiss = useCallback(() => void acknowledge(false), [acknowledge]);

  const displayName = session.label?.trim() || session.id.slice(0, 8);
  const agentName = agentLabel(modelSwitch.agent);
  const from = modelSwitch.from_model ?? "unknown";

  return (
    <Overlay
      open={open}
      onClose={onHide}
      modal
      width="min(92vw, 640px)"
      maxHeight="85vh"
      title="Model changed"
      subtitle={`${agentName} · ${displayName}`}
      leading={<Icon name="alert-triangle" size={14} />}
      data-testid="model-switch-modal"
      footer={
        <div className="msw__actions">
          <button
            type="button"
            className="msw__button"
            onClick={dismiss}
            disabled={busy != null}
          >
            {busy === "dismiss" ? "Closing…" : `Dismiss, I'll switch back to ${from}`}
          </button>
          <button
            type="button"
            className="msw__button msw__button--primary"
            onClick={adopt}
            disabled={busy != null}
          >
            {busy === "adopt" ? "Confirming…" : `Continue with ${modelSwitch.to_model}`}
          </button>
        </div>
      }
    >
      <div className="msw">
        <div className="msw__headline" data-testid="model-switch-headline">
          <span className="msw__model msw__model--from">{from}</span>
          <Icon name="arrow-right" size={14} />
          <span className="msw__model msw__model--to">{modelSwitch.to_model}</span>
        </div>
        <dl className="msw__facts">
          <dt>When</dt>
          <dd className="tabular">{formatTimestamp(modelSwitch.observed_at)}</dd>
          {(modelSwitch.from_effort || modelSwitch.to_effort) && (
            <>
              <dt>Effort</dt>
              <dd>
                {modelSwitch.from_effort ?? "unknown"} → {modelSwitch.to_effort ?? "unknown"}
              </dd>
            </>
          )}
          <dt>Recorded by</dt>
          <dd>{sourceLabel(modelSwitch.source)}</dd>
        </dl>
        <p className="msw__status" role="status">
          {statusText(modelSwitch)}
        </p>
        {modelSwitch.interrupt_error && (
          <div className="msw__error">Interrupt failed: {modelSwitch.interrupt_error}</div>
        )}
        <section className="msw__reason">
          <h4 className="msw__reason-title">Why</h4>
          <ReasonBody modelSwitch={modelSwitch} />
        </section>
        <p className="msw__hint">
          <strong>Continue</strong> makes {modelSwitch.to_model} the model this session is
          expected to run on. <strong>Dismiss</strong> keeps {from} as the expected model: the
          next turn on {modelSwitch.to_model} is no longer stopped, and returning to {from}
          (for example with <code>/model</code> in the terminal) does not raise this again.
        </p>
        {error && <div className="msw__error">{error}</div>}
      </div>
    </Overlay>
  );
}

/** Compact reminder shown in the timeline while a pending switch's dialog
 * is hidden. */
export function ModelSwitchBanner({
  modelSwitch,
  onReview,
}: {
  modelSwitch: ModelSwitchView;
  onReview: () => void;
}) {
  return (
    <div
      className="timeline-pane__attention"
      role="alert"
      data-testid="model-switch-banner"
    >
      <Icon name="alert-triangle" size={14} />
      <span className="timeline-pane__attention-label">
        Model changed to {modelSwitch.to_model}
      </span>
      <span className="timeline-pane__attention-summary">
        {modelSwitch.from_model ? `was ${modelSwitch.from_model} · ` : ""}
        turns on it are stopped until you confirm
      </span>
      <button
        type="button"
        className="timeline-pane__attention-button"
        onClick={onReview}
      >
        Review
      </button>
    </div>
  );
}

function ReasonBody({ modelSwitch }: { modelSwitch: ModelSwitchView }) {
  const context = modelSwitch.context ?? {};
  if (modelSwitch.agent === "codex") {
    return (
      <div className="msw__reason-body">
        <p>
          Codex applied new thread settings and recorded no reason. The next turn
          started with a <code>&lt;model_switch&gt;</code> note telling the model the
          user &ldquo;was previously using a different model&rdquo;.
        </p>
        <RateLimitFacts rateLimits={context.rate_limits ?? null} />
      </div>
    );
  }
  const fallback = context.fallback;
  if (fallback) {
    return (
      <div className="msw__reason-body">
        <p>
          Claude Code fell back from {fallback.from ?? modelSwitch.from_model ?? "the primary model"} to{" "}
          {fallback.to ?? modelSwitch.to_model}: the request to the primary model did not
          complete and was retried on the fallback model mid-turn. The transcript records the
          fallback itself, not the error behind it.
        </p>
        {context.iterations && context.iterations.length > 0 && (
          <ul className="msw__iterations">
            {context.iterations.map((iteration, index) => (
              <li key={index}>
                <span className="msw__iteration-type">{iteration.type ?? "request"}</span>
                <span className="msw__iteration-model">{iteration.model ?? "unknown"}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    );
  }
  return (
    <div className="msw__reason-body">
      <p>
        The transcript shows the new model on the turn&rsquo;s own records with no fallback
        marker and no stated reason.
      </p>
    </div>
  );
}

function RateLimitFacts({ rateLimits }: { rateLimits: CodexRateLimits | null }) {
  if (!rateLimits) {
    return <p>No rate-limit snapshot was recorded before the switch.</p>;
  }
  const rows: Array<[string, string]> = [];
  if (rateLimits.primary) {
    rows.push(["Primary window", describeWindow(rateLimits.primary)]);
  }
  if (rateLimits.secondary) {
    rows.push(["Secondary window", describeWindow(rateLimits.secondary)]);
  }
  if (rateLimits.plan_type) rows.push(["Plan", rateLimits.plan_type]);
  if (rateLimits.rate_limit_reached_type) {
    rows.push(["Limit reached", rateLimits.rate_limit_reached_type]);
  }
  if (rateLimits.credits) {
    const credits = rateLimits.credits;
    rows.push([
      "Credits",
      credits.unlimited
        ? "unlimited"
        : credits.has_credits
          ? `balance ${credits.balance ?? "unknown"}`
          : "none",
    ]);
  }
  return (
    <>
      <p>Rate-limit state Codex last reported before the switch:</p>
      <dl className="msw__facts" data-testid="model-switch-rate-limits">
        {rows.map(([label, value]) => (
          <div key={label} className="msw__fact">
            <dt>{label}</dt>
            <dd className="tabular">{value}</dd>
          </div>
        ))}
      </dl>
    </>
  );
}

function describeWindow(window: {
  used_percent: number;
  window_minutes: number;
  resets_at: number;
}): string {
  const days = window.window_minutes / 1440;
  const span =
    days >= 1
      ? `${Number.isInteger(days) ? days : days.toFixed(1)}-day`
      : `${Math.round(window.window_minutes / 60)}-hour`;
  const resets = new Date(window.resets_at * 1000);
  const resetText = Number.isNaN(resets.getTime()) ? "" : `, resets ${resets.toLocaleString()}`;
  return `${window.used_percent}% of the ${span} window used${resetText}`;
}

function statusText(modelSwitch: ModelSwitchView): string {
  if (modelSwitch.interrupted_at) {
    return `The turn running on ${modelSwitch.to_model} was interrupted at ${formatTimestamp(modelSwitch.interrupted_at)}.`;
  }
  if (modelSwitch.turn_in_flight) {
    return `A turn was underway when the model changed. It will be stopped until you decide.`;
  }
  return `The change happened between turns. The next turn on ${modelSwitch.to_model} will be stopped until you decide.`;
}

function sourceLabel(source: string): string {
  switch (source) {
    case "codex_thread_settings":
      return "Codex thread settings";
    case "codex_turn_context":
      return "Codex turn context";
    case "claude_fallback":
      return "Claude Code fallback block";
    case "claude_message":
      return "Claude Code assistant record";
    default:
      return source;
  }
}

function agentLabel(agent: string): string {
  switch (agent) {
    case "codex":
      return "Codex";
    case "claude-code":
    case "claude":
      return "Claude Code";
    default:
      return agent;
  }
}

function formatTimestamp(raw: string): string {
  const date = new Date(raw);
  if (Number.isNaN(date.getTime())) return raw;
  return date.toLocaleString();
}
