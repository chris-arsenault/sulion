// Admin action: rebuild derived transcript rows from the database copy
// of ingested events. Gated behind a typed-phrase confirm so it can't
// fire on an accidental click — the user has to type "refresh" before
// the dialog's confirm button unlocks.

import { useCallback, useState } from "react";

import {
  ApiError,
  triggerReindex,
  triggerRetrievalBackfill,
  type ReindexResponse,
  type RetrievalBackfillResponse,
} from "../api/client";
import { Icon } from "../icons";
import { ConfirmDialog } from "./common/ConfirmDialog";
import { Tooltip } from "./ui";
import "./ReindexButton.css";

type Phase =
  | { kind: "idle" }
  | { kind: "confirming"; target: "transcripts" | "retrieval" }
  | { kind: "running"; target: "transcripts" | "retrieval" }
  | { kind: "transcripts-done"; stats: ReindexResponse }
  | { kind: "retrieval-done"; stats: RetrievalBackfillResponse }
  | { kind: "error"; message: string };

export function ReindexButton() {
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });

  const confirmTranscripts = useCallback(
    () => setPhase({ kind: "confirming", target: "transcripts" }),
    [],
  );
  const confirmRetrieval = useCallback(
    () => setPhase({ kind: "confirming", target: "retrieval" }),
    [],
  );
  const cancelConfirm = useCallback(() => setPhase({ kind: "idle" }), []);
  const dismissResult = useCallback(() => setPhase({ kind: "idle" }), []);

  const runReindex = useCallback(async () => {
    setPhase({ kind: "running", target: "transcripts" });
    try {
      const stats = await triggerReindex();
      setPhase({ kind: "transcripts-done", stats });
    } catch (err) {
      setPhase({ kind: "error", message: errorMessage(err, "reindex failed") });
    }
  }, []);

  const runRetrievalBackfill = useCallback(async () => {
    setPhase({ kind: "running", target: "retrieval" });
    try {
      const stats = await triggerRetrievalBackfill({ limit: 500, max_batches: 20 });
      setPhase({ kind: "retrieval-done", stats });
    } catch (err) {
      setPhase({
        kind: "error",
        message: errorMessage(err, "retrieval backfill failed"),
      });
    }
  }, []);
  const confirmRunReindex = useCallback(() => {
    void runReindex();
  }, [runReindex]);
  const confirmRunRetrievalBackfill = useCallback(() => {
    void runRetrievalBackfill();
  }, [runRetrievalBackfill]);

  const running = phase.kind === "running";

  return (
    <div className="reindex">
      <Tooltip label="Rebuild transcript projections from stored event payloads">
        <button
          type="button"
          className="reindex__btn"
          onClick={confirmTranscripts}
          disabled={running}
          data-testid="reindex-button"
        >
          <Icon name="refresh-cw" size={12} />
          <span>
            {phase.kind === "running" && phase.target === "transcripts"
              ? "reindexing..."
              : "reindex"}
          </span>
        </button>
      </Tooltip>

      <Tooltip label="Backfill retrieval embeddings through the retrieval service">
        <button
          type="button"
          className="reindex__btn"
          onClick={confirmRetrieval}
          disabled={running}
          data-testid="retrieval-backfill-button"
        >
          <Icon name="search" size={12} />
          <span>
            {phase.kind === "running" && phase.target === "retrieval"
              ? "backfilling..."
              : "backfill"}
          </span>
        </button>
      </Tooltip>

      {phase.kind === "confirming" && phase.target === "transcripts" && (
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
          onConfirm={confirmRunReindex}
          onCancel={cancelConfirm}
        />
      )}

      {phase.kind === "confirming" && phase.target === "retrieval" && (
        <ConfirmDialog
          title="Backfill retrieval?"
          message={
            "This asks the retrieval service to embed the next batch of transcript and timeline sources. " +
            "Canonical transcript rows remain the source of truth; the retrieval index stores source keys and vectors."
          }
          requireText="backfill"
          confirmLabel="Backfill"
          onConfirm={confirmRunRetrievalBackfill}
          onCancel={cancelConfirm}
        />
      )}

      {phase.kind === "transcripts-done" && (
        <ConfirmDialog
          title="Reindex complete"
          message={formatDoneMessage(phase.stats)}
          confirmLabel="OK"
          cancelLabel="OK"
          onConfirm={dismissResult}
          onCancel={dismissResult}
        />
      )}

      {phase.kind === "retrieval-done" && (
        <ConfirmDialog
          title={phase.stats.complete ? "Backfill complete" : "Backfill paused"}
          message={formatRetrievalDoneMessage(phase.stats)}
          confirmLabel="OK"
          cancelLabel="OK"
          onConfirm={dismissResult}
          onCancel={dismissResult}
        />
      )}

      {phase.kind === "error" && (
        <ConfirmDialog
          title="Admin action failed"
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

function errorMessage(err: unknown, fallback: string): string {
  return err instanceof ApiError
    ? err.message
    : err instanceof Error
      ? err.message
      : fallback;
}

function formatDoneMessage(stats: ReindexResponse): string {
  return (
    `Rebuilt ${stats.sessions_rebuilt} transcript ${sessions(stats.sessions_rebuilt)} ` +
    `from ${stats.events_preserved} preserved event ${events(stats.events_preserved)}. ` +
    `Canonical rows rebuilt: ${stats.canonical_events_rebuilt}; timeline sessions rebuilt: ${stats.timeline_sessions_rebuilt}.`
  );
}

function formatRetrievalDoneMessage(stats: RetrievalBackfillResponse): string {
  const vector = stats.vector.ann_index_exists
    ? "pgvector ANN index ready"
    : stats.vector.column_exists
      ? "pgvector column ready"
      : "exact vector scan";
  return (
    `Embedded ${stats.embedded} retrieval ${sources(stats.embedded)}; ` +
    `skipped ${stats.skipped}; batches: ${stats.batches}. ` +
    `Model: ${stats.embedding_model} (${stats.embedding_dimensions}d); ${vector}.`
  );
}

function sessions(n: number): string {
  return n === 1 ? "session" : "sessions";
}

function events(n: number): string {
  return n === 1 ? "row" : "rows";
}

function sources(n: number): string {
  return n === 1 ? "source" : "sources";
}
