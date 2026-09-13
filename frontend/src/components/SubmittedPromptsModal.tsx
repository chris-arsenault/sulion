import { useCallback, useEffect, useState } from "react";

import {
  dismissSubmittedPrompt,
  listSubmittedPrompts,
  sendSessionPrompt,
} from "../api/client";
import type { PromptGate, SubmittedPrompt } from "../api/types";
import { Icon } from "../icons";
import { useSessions } from "../state/SessionStore";
import { copyToClipboard } from "./terminal/clipboard";
import { gateText } from "./timeline/promptGate";
import { Overlay } from "./ui";
import "./SubmittedPromptsModal.css";

/** Out-of-band view of what the timeline input actually sent, whether or
 * not the harness ever wrote it to its transcript. Deliberately not part of
 * the timeline: a prompt that never became a turn has no row there. */
export function SubmittedPromptsModal({
  open,
  sessionId,
  onClose,
}: {
  open: boolean;
  sessionId: string | null;
  onClose: () => void;
}) {
  const session = useSessions((store) =>
    sessionId ? store.sessions.find((item) => item.id === sessionId) ?? null : null,
  );
  const [prompts, setPrompts] = useState<SubmittedPrompt[]>([]);
  const [gate, setGate] = useState<PromptGate | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);

  const load = useCallback(async () => {
    if (!open || !sessionId) return;
    try {
      const resp = await listSubmittedPrompts(sessionId);
      setPrompts(resp.prompts);
      setGate(resp.gate);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "submitted prompt load failed");
    } finally {
      setLoaded(true);
    }
  }, [open, sessionId]);

  useEffect(() => {
    if (!open) {
      setPrompts([]);
      setGate(null);
      setBusyId(null);
      setError(null);
      setLoaded(false);
      return;
    }
    void load();
  }, [open, load]);

  const running =
    session?.state === "live" && session.agent_runtime?.state === "running";

  const retry = useCallback(
    async (entry: SubmittedPrompt) => {
      if (!sessionId || !running) return;
      setBusyId(entry.id);
      try {
        await sendSessionPrompt(sessionId, entry.text, { force: true });
        setError(null);
        await load();
      } catch (err) {
        setError(err instanceof Error ? err.message : "retry failed");
      } finally {
        setBusyId(null);
      }
    },
    [sessionId, running, load],
  );

  const dismiss = useCallback(
    async (entry: SubmittedPrompt) => {
      if (!sessionId) return;
      setBusyId(entry.id);
      try {
        await dismissSubmittedPrompt(sessionId, entry.id);
        setError(null);
        await load();
      } catch (err) {
        setError(err instanceof Error ? err.message : "dismiss failed");
      } finally {
        setBusyId(null);
      }
    },
    [sessionId, load],
  );

  const copy = useCallback(async (entry: SubmittedPrompt) => {
    await copyToClipboard(entry.text);
  }, []);

  const displayName = session
    ? session.label?.trim() || session.id.slice(0, 8)
    : "session";
  const unmatched = prompts.filter((entry) => entry.state === "unmatched").length;

  return (
    <Overlay
      open={open && Boolean(sessionId)}
      onClose={onClose}
      modal
      width="min(92vw, 760px)"
      maxHeight="80vh"
      title="Submitted Prompts"
      subtitle={`${displayName} · ${unmatched} unmatched`}
      leading={<Icon name="file-text" size={14} />}
    >
      <div className="sp">
        <p className="sp__lede">
          Everything sent from the timeline input, recorded before it reached the
          terminal. A prompt is <strong>matched</strong> once the transcript shows
          it as a turn; <strong>unmatched</strong> means the harness has not written
          it, usually because a startup dialog swallowed it.
        </p>
        {gate && (
          <div className="sp__gate" role="status">
            <Icon name="alert-triangle" size={12} />
            <span>{gateText(gate)}</span>
          </div>
        )}
        {error && <div className="sp__error">{error}</div>}
        {loaded && prompts.length === 0 ? (
          <div className="sp__empty">Nothing sent from the timeline yet.</div>
        ) : (
          <ul className="sp__list">
            {prompts.map((entry) => (
              <PromptRow
                key={entry.id}
                entry={entry}
                busy={busyId === entry.id}
                canRetry={running}
                onCopy={copy}
                onRetry={retry}
                onDismiss={dismiss}
              />
            ))}
          </ul>
        )}
      </div>
    </Overlay>
  );
}

function PromptRow({
  entry,
  busy,
  canRetry,
  onCopy,
  onRetry,
  onDismiss,
}: {
  entry: SubmittedPrompt;
  busy: boolean;
  canRetry: boolean;
  onCopy: (entry: SubmittedPrompt) => Promise<void>;
  onRetry: (entry: SubmittedPrompt) => Promise<void>;
  onDismiss: (entry: SubmittedPrompt) => Promise<void>;
}) {
  const copy = useCallback(() => void onCopy(entry), [entry, onCopy]);
  const retry = useCallback(() => void onRetry(entry), [entry, onRetry]);
  const dismiss = useCallback(() => void onDismiss(entry), [entry, onDismiss]);
  const settled = entry.state === "dismissed";
  return (
    <li className="sp__item" data-state={entry.state}>
      <div className="sp__item-meta">
        <span className={`sp__state sp__state--${entry.state}`}>{entry.state}</span>
        <span className="sp__timestamp">{formatTimestamp(entry.submitted_at)}</span>
        {entry.forced && <span className="sp__flag">sent past gate</span>}
        {entry.delivery_error && (
          <span className="sp__flag sp__flag--error">{entry.delivery_error}</span>
        )}
      </div>
      <pre className="sp__item-text">{entry.text}</pre>
      <div className="sp__item-actions">
        <button type="button" className="sp__button" onClick={copy} disabled={busy}>
          Copy
        </button>
        {!settled && (
          <button
            type="button"
            className="sp__button sp__button--primary"
            onClick={retry}
            disabled={!canRetry || busy}
          >
            {busy ? "Sending…" : "Retry"}
          </button>
        )}
        {!settled && entry.state !== "matched" && (
          <button type="button" className="sp__button" onClick={dismiss} disabled={busy}>
            Dismiss
          </button>
        )}
      </div>
    </li>
  );
}

function formatTimestamp(raw: string): string {
  const date = new Date(raw);
  if (Number.isNaN(date.getTime())) return raw;
  return date.toLocaleString();
}
