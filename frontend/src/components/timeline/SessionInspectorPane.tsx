// Ticket #28. Shows the selected turn's full detail in a pane to the
// right of the timeline list. On narrow viewports it renders as an
// overlay modal instead (driven by a useMediaQuery hook in the parent
// TimelinePane).

import { useEffect } from "react";
import { createPortal } from "react-dom";

import type { ToolPair, Turn } from "./grouping";
import { Icon } from "../../icons";
import { TurnDetail } from "./TurnDetail";
import "./SessionInspectorPane.css";

interface Props {
  turn: Turn | null;
  showThinking: boolean;
  onOpenSubagent?: (pair: ToolPair) => void;
  /** When true, render as a full-screen overlay modal with a backdrop;
   * when false, render as an inline pane. */
  asOverlay: boolean;
  onClose?: () => void;
  /** Optional: id of the tool call to focus within the selected turn.
   * TurnDetail expands that tool, collapses siblings, and marks it. */
  focusPairId?: string | null;
  /** Changes on every focus request so rebases can fire. */
  focusKey?: string | null;
}

export function SessionInspectorPane({
  turn,
  showThinking,
  onOpenSubagent,
  asOverlay,
  onClose,
  focusPairId,
  focusKey,
}: Props) {
  useEffect(() => {
    if (!asOverlay) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && onClose) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [asOverlay, onClose]);

  const body = turn ? (
    <TurnDetail
      turn={turn}
      showThinking={showThinking}
      onOpenSubagent={onOpenSubagent}
      focusPairId={focusPairId ?? null}
      focusKey={focusKey ?? null}
    />
  ) : (
    <div className="sip__empty">
      <p>Select a turn from the timeline to see its detail here.</p>
    </div>
  );

  if (asOverlay) {
    // Only render the modal when a turn is selected. On narrow viewports
    // we use selection presence as the "is modal open" signal.
    if (!turn) return null;
    return createPortal(
      <div
        className="sip__overlay-backdrop"
        data-testid="inspector-overlay"
      >
        <button
          type="button"
          className="sip__overlay-dismiss"
          aria-label="Dismiss turn detail"
          onClick={onClose}
        />
        <div
          className="sip__overlay-content"
          role="dialog"
          aria-modal="true"
        >
          <div className="sip__overlay-header">
            <span className="sip__overlay-title">Turn detail</span>
            <button
              type="button"
              className="sip__overlay-close"
              onClick={onClose}
              aria-label="Close turn detail"
            >
              <Icon name="x" size={14} />
            </button>
          </div>
          {body}
        </div>
      </div>,
      document.body,
    );
  }

  return (
    <aside className="sip" data-testid="inspector-pane">
      {body}
    </aside>
  );
}
