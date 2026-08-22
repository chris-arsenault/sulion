// Shared positioning for anchor-relative popover flyouts (ThinkingFlyout,
// TimelineControlsFlyout, TurnGridFlyout): float below the anchor,
// flipping above when clipped, clamped horizontally to the viewport; on
// narrow viewports collapse to a full-width bottom sheet instead.

import { useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from "react";

import type { Maybe } from "../../lib/types";

export function useFlyoutPosition(
  anchor: HTMLElement | null,
  onClose: () => void,
  recomputeKey?: unknown,
) {
  const cardRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);
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
  }, [anchor, isNarrow, recomputeKey]);

  const cardStyle = useMemo(
    (): Maybe<CSSProperties> => (pos ? { top: pos.top, left: pos.left } : undefined),
    [pos],
  );

  return { cardRef, isNarrow, cardStyle };
}
