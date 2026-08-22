// Compact turn-navigation grid, relocated out of the pane body into a
// flyout above the steer/send button — keeps the grid available without
// permanently reserving vertical space in the timeline.

import { createPortal } from "react-dom";

import type { TurnSummary } from "./grouping";
import { TurnGrid } from "./TurnGrid";
import { useFlyoutPosition } from "./useFlyoutPosition";
import { Icon } from "../../icons";
import "./Flyout.css";
import "./TurnGrid.css";

interface Props {
  anchor: HTMLElement | null;
  turns: TurnSummary[];
  selectedTurnKey: string | null;
  onSelect: (key: string) => void;
  onClose: () => void;
}

export function TurnGridFlyout({ anchor, turns, selectedTurnKey, onSelect, onClose }: Props) {
  const { cardRef, isNarrow, cardStyle } = useFlyoutPosition(anchor, onClose, turns.length);

  const body = (
    <>
      <div className="flyout__header">
        <span className="flyout__title">
          <Icon name="layers" size={12} />
          <span>turn grid</span>
        </span>
        <button
          type="button"
          className="flyout__close"
          onClick={onClose}
          aria-label="Close turn grid"
        >
          <Icon name="x" size={12} />
        </button>
      </div>
      <TurnGrid turns={turns} selectedTurnKey={selectedTurnKey} onSelect={onSelect} />
    </>
  );

  if (isNarrow) {
    return createPortal(
      <div className="flyout__sheet-backdrop" data-testid="turn-grid-flyout">
        <button
          type="button"
          className="flyout__sheet-dismiss"
          aria-label="Dismiss turn grid"
          onClick={onClose}
        />
        <div
          ref={cardRef}
          className="flyout__sheet"
          role="dialog"
          aria-modal="true"
          aria-label="turn grid"
        >
          {body}
        </div>
      </div>,
      document.body,
    );
  }

  return createPortal(
    <div
      ref={cardRef}
      className="flyout__card"
      // eslint-disable-next-line local/no-inline-styles -- popover position is anchor-relative, computed at render time
      style={cardStyle}
      role="dialog"
      aria-label="turn grid"
      data-testid="turn-grid-flyout"
    >
      {body}
    </div>,
    document.body,
  );
}
