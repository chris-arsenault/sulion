import { useEffect, useMemo, useState } from "react";

import { SessionEndedPane } from "./SessionEndedPane";
import { Sidebar } from "./Sidebar";
import { TerminalPane } from "./TerminalPane";
import { TimelinePane } from "./TimelinePane";
import { useMediaQuery } from "../hooks/useMediaQuery";
import { useSessions } from "../state/SessionStore";
import "./Layout.css";

/** Root three-region layout.
 * Desktop: sidebar (left) + main area (terminal top / timeline bottom)
 * separated by a draggable divider.
 * Mobile (<768px): sidebar collapses to an off-canvas drawer behind a
 * hamburger, the terminal/timeline split becomes a tabbed view, and we
 * default to Timeline so the "phone check-in" use case is one tap away. */
export function Layout() {
  const { selectedSessionId, sessions } = useSessions();
  const [terminalFraction, setTerminalFraction] = useState(0.55);
  const isMobile = useMediaQuery("(max-width: 767px)");
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [mobilePane, setMobilePane] = useState<"timeline" | "terminal">("timeline");

  // Close the drawer whenever the user picks a session.
  useEffect(() => {
    if (selectedSessionId) setDrawerOpen(false);
  }, [selectedSessionId]);

  const selected = useMemo(
    () => sessions.find((s) => s.id === selectedSessionId) ?? null,
    [sessions, selectedSessionId],
  );

  // If the selected session exists but isn't live, we can't attach a
  // terminal — show the SessionEndedPane instead. The timeline pane
  // still works because Claude session events persist in Postgres.
  const topPane = (() => {
    if (!selectedSessionId) return null;
    if (selected && selected.state !== "live") {
      return <SessionEndedPane session={selected} />;
    }
    return <TerminalPane sessionId={selectedSessionId} />;
  })();

  if (isMobile) {
    return (
      <div className="layout layout--mobile">
        <MobileTopBar
          onOpenDrawer={() => setDrawerOpen(true)}
          pane={mobilePane}
          onPaneChange={setMobilePane}
          hasSession={!!selectedSessionId}
        />
        {drawerOpen && (
          <>
            <div
              className="layout__scrim"
              onClick={() => setDrawerOpen(false)}
              aria-hidden
            />
            <aside className="layout__drawer" aria-label="Sessions">
              <Sidebar />
            </aside>
          </>
        )}
        <main className="layout__main">
          {selectedSessionId && topPane ? (
            // Both panes are mounted — hidden pane keeps its state (xterm
            // buffer, scroll position, websocket) while the visible one
            // shows. Display is driven by mobilePane.
            <div className="mobile-panes">
              <div
                className={
                  mobilePane === "terminal"
                    ? "mobile-pane mobile-pane--active"
                    : "mobile-pane"
                }
              >
                {topPane}
              </div>
              <div
                className={
                  mobilePane === "timeline"
                    ? "mobile-pane mobile-pane--active"
                    : "mobile-pane"
                }
              >
                <TimelinePane sessionId={selectedSessionId} />
              </div>
            </div>
          ) : (
            <EmptyState mobile />
          )}
        </main>
      </div>
    );
  }

  return (
    <div className="layout">
      <aside className="layout__sidebar">
        <Sidebar />
      </aside>
      <main className="layout__main">
        {selectedSessionId && topPane ? (
          <SplitPanes
            top={topPane}
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

function MobileTopBar({
  onOpenDrawer,
  pane,
  onPaneChange,
  hasSession,
}: {
  onOpenDrawer: () => void;
  pane: "timeline" | "terminal";
  onPaneChange: (p: "timeline" | "terminal") => void;
  hasSession: boolean;
}) {
  return (
    <div className="mobile-topbar">
      <button
        type="button"
        className="mobile-topbar__hamburger"
        onClick={onOpenDrawer}
        aria-label="Open sessions drawer"
      >
        ☰
      </button>
      <span className="mobile-topbar__title">shuttlecraft</span>
      {hasSession && (
        <div className="mobile-topbar__tabs" role="tablist">
          <button
            type="button"
            role="tab"
            aria-selected={pane === "timeline"}
            className={
              pane === "timeline"
                ? "mobile-topbar__tab mobile-topbar__tab--active"
                : "mobile-topbar__tab"
            }
            onClick={() => onPaneChange("timeline")}
          >
            Timeline
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={pane === "terminal"}
            className={
              pane === "terminal"
                ? "mobile-topbar__tab mobile-topbar__tab--active"
                : "mobile-topbar__tab"
            }
            onClick={() => onPaneChange("terminal")}
          >
            Terminal
          </button>
        </div>
      )}
    </div>
  );
}

function EmptyState({ mobile = false }: { mobile?: boolean }) {
  return (
    <div className="layout__empty">
      <h1>shuttlecraft</h1>
      <p>
        {mobile
          ? "Tap ☰ to open the session list."
          : "Select a session from the sidebar or create a new one to begin."}
      </p>
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
