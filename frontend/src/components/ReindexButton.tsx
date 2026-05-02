// Destructive admin action: wipe every transcript row and per-file
// ingest offset, then kick the ingester to replay every JSONL from
// scratch. Gated behind a typed-phrase confirm so it can't fire on
// an accidental click — the user has to type "refresh" before the
// dialog's confirm button unlocks.

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
      <Tooltip label="Clear ingest state and re-read every JSONL transcript from disk">
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
            "This wipes every transcript row in the database and re-reads every JSONL " +
            "file from scratch. Terminal associations and saved library entries are preserved. " +
            "The timeline will be empty for a few seconds while the ingester replays."
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
  // The ingester runs on its own poll loop; the re-read happens in the
  // background after this returns. /api/stats is where you watch the
  // running totals if you want to see counts climb back up.
  return (
    `Cleared ${stats.sessions_cleared} transcript ${sessions(stats.sessions_cleared)} ` +
    `and ${stats.offsets_cleared} ingest ${offsets(stats.offsets_cleared)}. ` +
    `The ingester will replay every JSONL from scratch on its next tick.`
  );
}

function sessions(n: number): string {
  return n === 1 ? "session" : "sessions";
}

function offsets(n: number): string {
  return n === 1 ? "offset" : "offsets";
}
