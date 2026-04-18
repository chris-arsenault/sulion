import { useState } from "react";

import { Sidebar } from "./Sidebar";
import { TerminalPane } from "./TerminalPane";
import { TimelinePane } from "./TimelinePane";
import { useSessions } from "../state/SessionStore";
import "./Layout.css";

/** Root three-region layout. Sidebar (left) + main area (terminal top /
 * timeline bottom) separated by a draggable divider. */
export function Layout() {
  const { selectedSessionId } = useSessions();
  // Terminal occupies the top portion; timeline the bottom. Divider is
  // a simple CSS-controlled flex ratio for now; real drag behavior added
  // in #9/#10 as the panes get weight.
  const [terminalFraction, setTerminalFraction] = useState(0.55);

  return (
    <div className="layout">
      <aside className="layout__sidebar">
        <Sidebar />
      </aside>
      <main className="layout__main">
        {selectedSessionId ? (
          <SplitPanes
            top={<TerminalPane sessionId={selectedSessionId} />}
            bottom={<TimelinePane sessionId={selectedSessionId} />}
            topFraction={terminalFraction}
            onDrag={setTerminalFraction}
          />
        ) : (
          <EmptyState />
        )}
      </main>
    </div>
  );
}

function EmptyState() {
  return (
    <div className="layout__empty">
      <h1>shuttlecraft</h1>
      <p>Select a session from the sidebar or create a new one to begin.</p>
    </div>
  );
}

function SplitPanes({
  top,
  bottom,
  topFraction,
  onDrag,
}: {
  top: React.ReactNode;
  bottom: React.ReactNode;
  topFraction: number;
  onDrag: (f: number) => void;
}) {
  const onMouseDown = (e: React.MouseEvent) => {
    e.preventDefault();
    const container = (e.target as HTMLElement).parentElement;
    if (!container) return;
    const { top, height } = container.getBoundingClientRect();
    const onMove = (ev: MouseEvent) => {
      const f = clamp((ev.clientY - top) / height, 0.15, 0.85);
      onDrag(f);
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  return (
    <div
      className="split"
      style={{ gridTemplateRows: `${topFraction}fr 6px ${1 - topFraction}fr` }}
    >
      <div className="split__top">{top}</div>
      <div
        className="split__divider"
        role="separator"
        aria-orientation="horizontal"
        onMouseDown={onMouseDown}
      />
      <div className="split__bottom">{bottom}</div>
    </div>
  );
}

function clamp(v: number, lo: number, hi: number) {
  return Math.max(lo, Math.min(hi, v));
}
