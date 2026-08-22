// Ticket #29. Pinned floating card for a thinking block. Replaces the
// inline "purple box in the middle of the assistant row" rendering —
// thinking is now accessible as a 💭 chip that opens this card when
// clicked. Card stays pinned until explicitly closed (× / Esc / click
// another chip). One card at a time.

import { createPortal } from "react-dom";

import { Icon } from "../../icons";
import { useFlyoutPosition } from "./useFlyoutPosition";
import "./Flyout.css";
import "./ThinkingFlyout.css";

interface Props {
  anchor: HTMLElement | null;
  thinkingText: string;
  onClose: () => void;
}

export function ThinkingFlyout({ anchor, thinkingText, onClose }: Props) {
  const { cardRef, isNarrow, cardStyle } = useFlyoutPosition(anchor, onClose, thinkingText);

  if (isNarrow) {
    return createPortal(
      <div
        className="flyout__sheet-backdrop"
        data-testid="thinking-flyout"
      >
        <button
          type="button"
          className="flyout__sheet-dismiss"
          aria-label="Dismiss thinking"
          onClick={onClose}
        />
        <div
          ref={cardRef}
          className="flyout__sheet"
          role="dialog"
          aria-modal="true"
          aria-label="thinking"
        >
          <div className="flyout__header">
            <span className="flyout__title">
              <Icon name="sparkles" size={12} />
              <span>thinking</span>
            </span>
            <button
              type="button"
              className="flyout__close"
              onClick={onClose}
              aria-label="Close thinking"
            >
              <Icon name="x" size={12} />
            </button>
          </div>
          <pre className="tf__body">{thinkingText}</pre>
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
      aria-label="thinking"
      data-testid="thinking-flyout"
    >
      <div className="flyout__header">
        <span className="flyout__title">
          <Icon name="sparkles" size={12} />
          <span>thinking</span>
        </span>
        <button
          type="button"
          className="flyout__close"
          onClick={onClose}
          aria-label="Close thinking"
        >
          <Icon name="x" size={12} />
        </button>
      </div>
      <pre className="tf__body">{thinkingText}</pre>
    </div>,
    document.body,
  );
}
