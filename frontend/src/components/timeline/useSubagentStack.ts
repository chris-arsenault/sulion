// The subagent modal's navigation. A spawning pair carries a reference to
// its transcript (a child session, or sidechain turns of this one); the
// modal reads those turns on open and again whenever the pane's timeline
// revision moves, which a child's growth does.

import { useCallback, useEffect, useState } from "react";

import { getTranscriptSummaries } from "../../api/turnStream";
import type { TimelineQuery, TimelineSubagent, TimelineTurnSummary } from "../../api/types";
import type { ToolPair } from "./grouping";

export interface SubagentStack {
  /** The open transcript, or null when the modal is closed. */
  subagent: TimelineSubagent | null;
  /** Its turns; null while the first read is in flight. */
  turns: TimelineTurnSummary[] | null;
  error: string | null;
  depth: number;
  open: (pair: ToolPair) => void;
  back: () => void;
  close: () => void;
}

function referenceKey(subagent: TimelineSubagent): string {
  return `${subagent.session_uuid ?? ""}:${(subagent.turn_ids ?? []).join(",")}`;
}

export function useSubagentStack(
  _query: TimelineQuery,
  revision: number,
  active: boolean,
): SubagentStack {
  const [stack, setStack] = useState<TimelineSubagent[]>([]);
  const [loaded, setLoaded] = useState<{ key: string; turns: TimelineTurnSummary[] } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const subagent = stack[stack.length - 1] ?? null;
  const key = subagent ? referenceKey(subagent) : null;

  useEffect(() => {
    if (!active || !subagent?.session_uuid) return;
    const readKey = referenceKey(subagent);
    const controller = new AbortController();
    setError(null);
    void getTranscriptSummaries(subagent.session_uuid, controller.signal)
      .then((resp) => {
        if (!controller.signal.aborted) setLoaded({ key: readKey, turns: resp.turns.filter(
          (turn) => !subagent.turn_ids?.length || subagent.turn_ids.includes(turn.id),
        ) });
      })
      .catch((err: unknown) => {
        if (!controller.signal.aborted) setError(err instanceof Error ? err.message : "Subagent loading failed");
      });
    return () => {
      controller.abort();
    };
  }, [active, subagent, revision]);

  const open = useCallback((pair: ToolPair) => {
    const next = pair.subagent;
    if (next) setStack((prev) => [...prev, next]);
  }, []);
  const back = useCallback(() => setStack((prev) => prev.slice(0, -1)), []);
  const close = useCallback(() => setStack([]), []);

  return {
    subagent,
    error,
    turns: loaded && loaded.key === key ? loaded.turns : null,
    depth: stack.length,
    open,
    back,
    close,
  };
}
