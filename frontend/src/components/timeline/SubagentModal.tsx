// Drill-in for Task tool uses. Finds all events in the session whose
// lineage traces back to the Task's tool_use_id (via parentUuid chains
// + isSidechain=true) and renders them as a nested turn-grouped view.

import { useEffect, useMemo } from "react";
import { createPortal } from "react-dom";

import type { TimelineEvent } from "../../api/types";
import { groupIntoTurns } from "./grouping";
import { TurnDetail } from "./TurnDetail";
import { isSidechainEvent } from "./types";
import "./SubagentModal.css";

interface Props {
  /** The parent Task tool_use_id that seeds the subagent lineage. */
  toolUseId: string;
  /** The assistant event that contained the Task tool_use, used as a
   * fallback seed when parentUuid chains are thin. */
  seedUuid?: string;
  /** Label shown in the modal header. */
  title?: string;
  /** All events for the current Claude session. The modal picks the
   * subset that belongs to the subagent. */
  allEvents: TimelineEvent[];
  showThinking: boolean;
  onClose: () => void;
}

export function SubagentModal({
  toolUseId,
  seedUuid,
  title,
  allEvents,
  showThinking,
  onClose,
}: Props) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const subagentEvents = useMemo(
    () => collectSubagentEvents(allEvents, toolUseId, seedUuid),
    [allEvents, toolUseId, seedUuid],
  );
  const turns = useMemo(
    () => groupIntoTurns(subagentEvents),
    [subagentEvents],
  );

  return createPortal(
    <div
      className="sm__backdrop"
      onMouseDown={onClose}
      role="dialog"
      aria-modal="true"
      aria-labelledby="sm-title"
      data-testid="subagent-modal"
    >
      <div className="sm__content" onMouseDown={(e) => e.stopPropagation()}>
        <div className="sm__header">
          <h3 id="sm-title" className="sm__title">
            {title ?? "Agent log"}
          </h3>
          <span className="sm__meta">
            {subagentEvents.length} events · {turns.length} turn
            {turns.length === 1 ? "" : "s"}
          </span>
          <button
            type="button"
            className="sm__close"
            onClick={onClose}
            aria-label="Close subagent log"
          >
            ×
          </button>
        </div>
        <div className="sm__body">
          {turns.length === 0 && (
            <div className="sm__empty">
              No subagent events found for this Task. The subagent may not have
              emitted yet, or "Show sidechain" may have stripped them before
              they reached the modal — we already un-filtered sidechain for this
              view, so this is usually "just started."
            </div>
          )}
          {turns.map((t) => (
            <div key={t.id} className="sm__turn">
              <TurnDetail turn={t} showThinking={showThinking} />
            </div>
          ))}
        </div>
      </div>
    </div>,
    document.body,
  );
}

/** Walk the event graph and collect everything descended from the Task
 * tool_use_id. Covers two linkage mechanisms:
 *   - isSidechain: true + parentUuid chain traceable back to the tool_use
 *   - events with a top-level tool_use_id reference (observed in some
 *     subagent result-report formats)
 */
export function collectSubagentEvents(
  events: TimelineEvent[],
  toolUseId: string,
  seedUuid?: string,
): TimelineEvent[] {
  const uuidsInLineage = new Set<string>();
  if (seedUuid) uuidsInLineage.add(seedUuid);

  // Events with explicit tool_use_id reference belong without further chain
  // walks. Kick-start the lineage with their uuids too.
  for (const ev of events) {
    if (ev.related_tool_use_id === toolUseId && ev.event_uuid) {
      uuidsInLineage.add(ev.event_uuid);
    }
  }

  // Iterate until fixpoint, folding in events whose parentUuid is already
  // in the lineage AND which are sidechain. Non-sidechain events never
  // belong to a subagent by design.
  let added = true;
  while (added) {
    added = false;
    for (const ev of events) {
      if (!isSidechainEvent(ev)) continue;
      const uuid = ev.event_uuid;
      if (!uuid || uuidsInLineage.has(uuid)) continue;
      const parent = ev.parent_event_uuid;
      if (parent && uuidsInLineage.has(parent)) {
        uuidsInLineage.add(uuid);
        added = true;
      }
    }
  }

  return events.filter((ev) => {
    if (ev.event_uuid && uuidsInLineage.has(ev.event_uuid)) return true;
    return ev.related_tool_use_id === toolUseId;
  });
}
