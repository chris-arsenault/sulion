// Admin action: rebuild derived transcript rows from the database copy
// of ingested events. Gated behind a typed-phrase confirm so it can't
// fire on an accidental click — the user has to type "refresh" before
// the dialog's confirm button unlocks.

import { useCallback, useState } from "react";

import { ApiError, triggerReindex, type ReindexResponse } from "../api/client";
import { Icon } from "../icons";
import { ConfirmDialog } from "./common/ConfirmDialog";
import { Tooltip } from "./ui";
import "./ReindexButton.css";

type Phase =
  | { kind: "idle" }
  | { kind: "confirming" }
  | { kind: "running" }
  | { kind: "done"; stats: ReindexResponse }
  | { kind: "error"; message: string };

export function ReindexButton() {
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });

  const openConfirm = useCallback(() => setPhase({ kind: "confirming" }), []);
  const cancelConfirm = useCallback(() => setPhase({ kind: "idle" }), []);
  const dismissResult = useCallback(() => setPhase({ kind: "idle" }), []);

  const runReindex = useCallback(async () => {
    setPhase({ kind: "running" });
    try {
      const stats = await triggerReindex();
      setPhase({ kind: "done", stats });
    } catch (err) {
      const message =
        err instanceof ApiError
          ? err.message
          : err instanceof Error
            ? err.message
            : "reindex failed";
      setPhase({ kind: "error", message });
    }
  }, []);

  return (
    <div className="reindex">
      <Tooltip label="Rebuild transcript projections from stored event payloads">
        <button
          type="button"
          className="reindex__btn"
          onClick={openConfirm}
          disabled={phase.kind === "running"}
          data-testid="reindex-button"
        >
          <Icon name="refresh-cw" size={12} />
          <span>{phase.kind === "running" ? "reindexing…" : "reindex"}</span>
        </button>
      </Tooltip>

      {phase.kind === "confirming" && (
        <ConfirmDialog
          title="Reindex transcripts?"
          message={
            "This rebuilds canonical blocks and timeline projections from stored event payloads. " +
            "Source transcript rows, ingest offsets, terminal associations, and saved library entries are preserved. " +
            "The timeline may be incomplete while the rebuild runs."
          }
          requireText="refresh"
          confirmLabel="Reindex"
          destructive
          onConfirm={() => void runReindex()}
          onCancel={cancelConfirm}
        />
      )}

      {phase.kind === "done" && (
        <ConfirmDialog
          title="Reindex complete"
          message={formatDoneMessage(phase.stats)}
          confirmLabel="OK"
          cancelLabel="OK"
          onConfirm={dismissResult}
          onCancel={dismissResult}
        />
      )}

      {phase.kind === "error" && (
        <ConfirmDialog
          title="Reindex failed"
          message={phase.message}
          confirmLabel="OK"
          cancelLabel="OK"
          onConfirm={dismissResult}
          onCancel={dismissResult}
        />
      )}
    </div>
  );
}

function formatDoneMessage(stats: ReindexResponse): string {
  return (
    `Rebuilt ${stats.sessions_rebuilt} transcript ${sessions(stats.sessions_rebuilt)} ` +
    `from ${stats.events_preserved} preserved event ${events(stats.events_preserved)}. ` +
    `Canonical rows rebuilt: ${stats.canonical_events_rebuilt}; timeline sessions rebuilt: ${stats.timeline_sessions_rebuilt}.`
  );
}

function sessions(n: number): string {
  return n === 1 ? "session" : "sessions";
}

function events(n: number): string {
  return n === 1 ? "row" : "rows";
}
