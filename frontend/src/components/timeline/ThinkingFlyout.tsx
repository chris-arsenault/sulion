// Ticket #29. Pinned floating card for a thinking block. Replaces the
// inline "purple box in the middle of the assistant row" rendering —
// thinking is now accessible as a 💭 chip that opens this card when
// clicked. Card stays pinned until explicitly closed (× / Esc / click
// another chip). One card at a time.

import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";

import { Icon } from "../../icons";
import "./ThinkingFlyout.css";

interface Props {
  anchor: HTMLElement | null;
  thinkingText: string;
  onClose: () => void;
}

export function ThinkingFlyout({ anchor, thinkingText, onClose }: Props) {
  const cardRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);
  // If the viewport is narrow, render as a bottom sheet instead.
  const [isNarrow, setIsNarrow] = useState(false);

  useEffect(() => {
    const onResize = () => setIsNarrow(window.innerWidth < 720);
    onResize();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  useLayoutEffect(() => {
    if (isNarrow) return;
    if (!anchor || !cardRef.current) return;
    const a = anchor.getBoundingClientRect();
    const c = cardRef.current.getBoundingClientRect();
    const gap = 8;

    // Prefer below-anchor; fall back above if not enough room.
    let top = a.bottom + gap;
    if (top + c.height > window.innerHeight - gap) {
      top = Math.max(gap, a.top - c.height - gap);
    }
    // Horizontal align: left edge on anchor's left, clamped to viewport.
    let left = a.left;
    if (left + c.width > window.innerWidth - gap) {
      left = Math.max(gap, window.innerWidth - c.width - gap);
    }
    setPos({ top, left });
  }, [anchor, isNarrow, thinkingText]);

  const cardStyle = useMemo(
    () => (pos ? { top: pos.top, left: pos.left } : undefined),
    [pos],
  );

  if (isNarrow) {
    return createPortal(
      <div
        className="tf__sheet-backdrop"
        data-testid="thinking-flyout"
      >
        <button
          type="button"
          className="tf__sheet-dismiss"
          aria-label="Dismiss thinking"
          onClick={onClose}
        />
        <div
          ref={cardRef}
          className="tf__sheet"
          role="dialog"
          aria-modal="true"
          aria-label="thinking"
        >
          <div className="tf__header">
            <span className="tf__title">
              <Icon name="sparkles" size={12} />
              <span>thinking</span>
            </span>
            <button
              type="button"
              className="tf__close"
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
      className="tf__card"
      // eslint-disable-next-line local/no-inline-styles -- popover position is anchor-relative, computed at render time
      style={cardStyle}
      role="dialog"
      aria-label="thinking"
      data-testid="thinking-flyout"
    >
      <div className="tf__header">
        <span className="tf__title">
          <Icon name="sparkles" size={12} />
          <span>thinking</span>
        </span>
        <button
          type="button"
          className="tf__close"
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
