// Ticket #31. Floating peek card for a tool-use row. Hovering a tool
// row in the inspector pane opens the card after a short delay; moving
// the pointer out dismisses it unless the user has clicked to pin.
// Click-to-pin makes the card persistent until closed (× / Esc /
// another peek opens).

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import type { ToolPair } from "./grouping";
import { flattenContent } from "./types";
import { ToolCallRenderer } from "./tools/renderers";
import "./ToolHoverCard.css";

interface Props {
  anchor: HTMLElement | null;
  pair: ToolPair;
  pinned: boolean;
  onPin: () => void;
  onClose: () => void;
}

export function ToolHoverCard({ anchor, pair, pinned, onPin, onClose }: Props) {
  const cardRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);

  useEffect(() => {
    if (!pinned) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [pinned, onClose]);

  useLayoutEffect(() => {
    if (!anchor || !cardRef.current) return;
    const a = anchor.getBoundingClientRect();
    const c = cardRef.current.getBoundingClientRect();
    const gap = 8;

    // Prefer right-of-anchor; overflow → left-of-anchor; overflow → below.
    let top = a.top;
    let left = a.right + gap;
    if (left + c.width > window.innerWidth - gap) {
      left = a.left - c.width - gap;
    }
    if (left < gap) {
      left = Math.max(gap, Math.min(a.left, window.innerWidth - c.width - gap));
      top = a.bottom + gap;
      if (top + c.height > window.innerHeight - gap) {
        top = Math.max(gap, a.top - c.height - gap);
      }
    }
    if (top + c.height > window.innerHeight - gap) {
      top = Math.max(gap, window.innerHeight - c.height - gap);
    }
    setPos({ top, left });
  }, [anchor, pair.id]);

  const resultText = (() => {
    if (!pair.result) return pair.isPending ? "(pending)" : "";
    const body =
      typeof pair.result.content === "string"
        ? pair.result.content
        : flattenContent(pair.result.content);
    if (!body) return "(empty result)";
    return body.length > 1200 ? `${body.slice(0, 1200)}\n… (${body.length} chars)` : body;
  })();

  return createPortal(
    <div
      ref={cardRef}
      className={`thc ${pinned ? "thc--pinned" : ""} ${
        pair.isError ? "thc--error" : ""
      }`}
      style={pos ? { top: pos.top, left: pos.left } : undefined}
      role="tooltip"
      data-testid="tool-hover-card"
      onMouseDown={(e) => {
        // Clicking inside the card is how you pin it.
        e.stopPropagation();
        if (!pinned) onPin();
      }}
    >
      <div className="thc__header">
        <span className="thc__name">{pair.name}</span>
        {pair.isPending && <span className="thc__status">pending</span>}
        {pair.isError && <span className="thc__status thc__status--error">error</span>}
        {pinned && (
          <button
            type="button"
            className="thc__close"
            onClick={(e) => {
              e.stopPropagation();
              onClose();
            }}
            aria-label="Close card"
          >
            ×
          </button>
        )}
        {!pinned && <span className="thc__hint">click to pin</span>}
      </div>
      <div className="thc__input">
        <div className="thc__label">input</div>
        <ToolCallRenderer
          tool={{ id: pair.id, name: pair.name, input: pair.input }}
        />
      </div>
      {pair.result && (
        <div className={`thc__result ${pair.isError ? "thc__result--error" : ""}`}>
          <div className="thc__label">result{pair.isError ? " (error)" : ""}</div>
          <pre>{resultText}</pre>
        </div>
      )}
    </div>,
    document.body,
  );
}
