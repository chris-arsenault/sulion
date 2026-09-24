// The subagent modal's navigation. A spawning pair carries a reference to
// its transcript (a child session, or sidechain turns of this one); the
// modal reads those turns on open and again whenever the pane's timeline
// revision moves, which a child's growth does.

import { useCallback, useEffect, useState } from "react";

import { getSessionTurns } from "../../api/client";
import type { TimelineQuery, TimelineSubagent } from "../../api/types";
import type { ToolPair, Turn } from "./grouping";

export interface SubagentStack {
  /** The open transcript, or null when the modal is closed. */
  subagent: TimelineSubagent | null;
  /** Its turns; null while the first read is in flight. */
  turns: Turn[] | null;
  depth: number;
  open: (pair: ToolPair) => void;
  back: () => void;
  close: () => void;
}

function referenceKey(subagent: TimelineSubagent): string {
  return `${subagent.session_uuid ?? ""}:${(subagent.turn_ids ?? []).join(",")}`;
}

export function useSubagentStack(
  query: TimelineQuery,
  revision: number,
  active: boolean,
): SubagentStack {
  const [stack, setStack] = useState<TimelineSubagent[]>([]);
  const [loaded, setLoaded] = useState<{ key: string; turns: Turn[] } | null>(null);
  const subagent = stack[stack.length - 1] ?? null;
  const key = subagent ? referenceKey(subagent) : null;

  useEffect(() => {
    if (!active || !subagent?.session_uuid) return;
    const readKey = referenceKey(subagent);
    let cancelled = false;
    void getSessionTurns(subagent.session_uuid, subagent.turn_ids, query)
      .then((resp) => {
        if (!cancelled) setLoaded({ key: readKey, turns: resp.turns });
      })
      .catch(() => {
        if (!cancelled) setLoaded({ key: readKey, turns: [] });
      });
    return () => {
      cancelled = true;
    };
  }, [active, subagent, query, revision]);

  const open = useCallback((pair: ToolPair) => {
    const next = pair.subagent;
    if (next) setStack((prev) => [...prev, next]);
  }, []);
  const back = useCallback(() => setStack((prev) => prev.slice(0, -1)), []);
  const close = useCallback(() => setStack([]), []);

  return {
    subagent,
    turns: loaded && loaded.key === key ? loaded.turns : null,
    depth: stack.length,
    open,
    back,
    close,
  };
}
