// Two-pane tab system. Top pane / bottom pane, resizable divider,
// tabs drag within + between. Tab content is mounted and hidden via
// CSS visibility (not display:none) so xterm and virtuoso keep their
// state across tab switches.
//
// Mobile mode (viewport <768px) collapses to a single-pane strip
// showing every open tab mixed together, no divider.

import { useMemo, useRef, useState } from "react";

import type { PaneId, TabData } from "../state/TabStore";
import { useTabs } from "../state/TabStore";
import { useSessions } from "../state/SessionStore";
import { useMediaQuery } from "../hooks/useMediaQuery";
import type { MenuItem } from "./common/ContextMenu";
import {
  contextMenuHandler,
  useContextMenu,
} from "./common/ContextMenu";
import { TerminalPane } from "./TerminalPane";
import { TimelinePane } from "./TimelinePane";
import { SessionEndedPane } from "./SessionEndedPane";
import { FileTab } from "./FileTab";
import { DiffTab } from "./DiffTab";
import { SearchTab } from "./SearchTab";
import "./WorkArea.css";

export function WorkArea() {
  const { panes, activeByPane, tabs } = useTabs();
  const isMobile = useMediaQuery("(max-width: 767px)");
  const [topFraction, setTopFraction] = useState(0.55);
  // No more automatic prune: tabs survive their session's deletion and
  // show an orphan placeholder with a manual close. That keeps tab
  // state fully user-driven and avoids the "refresh wipes everything"
  // failure mode.

  if (isMobile) {
    // Mobile: fold all tabs into a single pane strip.
    const allTabIds = [...panes.top, ...panes.bottom];
    const activeId =
      activeByPane.top ?? activeByPane.bottom ?? allTabIds[0] ?? null;
    if (allTabIds.length === 0) {
      return <EmptyWorkArea mobile />;
    }
    return (
      <div className="wa wa--mobile">
        <Pane
          paneId="top"
          tabIds={allTabIds}
          activeId={activeId}
          tabs={tabs}
          mobile
        />
      </div>
    );
  }

  // Desktop: always render both panes so the split is stable. An empty
  // pane just shows a splash — we don't collapse the layout because
  // that made the solo pane's descendants claim zero height and the
  // tab content rendered into a 0px box.
  return (
    <div
      className="wa wa--split"
      // eslint-disable-next-line local/no-inline-styles -- resizable split fractions are per-user-drag; can't be CSS classes
      style={{ gridTemplateRows: `${topFraction}fr 6px ${1 - topFraction}fr` }}
    >
      <Pane
        paneId="top"
        tabIds={panes.top}
        activeId={activeByPane.top}
        tabs={tabs}
      />
      <Divider onDrag={setTopFraction} />
      <Pane
        paneId="bottom"
        tabIds={panes.bottom}
        activeId={activeByPane.bottom}
        tabs={tabs}
      />
    </div>
  );
}

function EmptyWorkArea({ mobile = false }: { mobile?: boolean }) {
  return (
    <div className="wa wa--empty">
      <div>
        <h1>shuttlecraft</h1>
        <p>
          {mobile
            ? "Tap ☰ to open the session list."
            : "Select a session from the sidebar or create a new one to begin."}
        </p>
      </div>
    </div>
  );
}

function Divider({ onDrag }: { onDrag: (f: number) => void }) {
  const onMouseDown = (e: React.MouseEvent) => {
    e.preventDefault();
    const container = (e.target as HTMLElement).parentElement;
    if (!container) return;
    const { top, height } = container.getBoundingClientRect();
    const onMove = (ev: MouseEvent) => {
      const f = Math.max(0.15, Math.min(0.85, (ev.clientY - top) / height));
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
      className="wa__divider"
      role="separator"
      aria-orientation="horizontal"
      onMouseDown={onMouseDown}
    />
  );
}

function Pane({
  paneId,
  tabIds,
  activeId,
  tabs,
  mobile,
}: {
  paneId: PaneId;
  tabIds: string[];
  activeId: string | null;
  tabs: Record<string, TabData>;
  mobile?: boolean;
}) {
  const { activateTab, closeTab, moveTab } = useTabs();
  const [dragTargetActive, setDragTargetActive] = useState(false);

  return (
    <div
      className={`wa__pane${mobile ? " wa__pane--mobile" : ""}${
        dragTargetActive ? " wa__pane--drop-target" : ""
      }`}
      onDragOver={(e) => {
        if (e.dataTransfer.types.includes("application/x-shuttlecraft-tab")) {
          e.preventDefault();
          setDragTargetActive(true);
        }
      }}
      onDragLeave={() => setDragTargetActive(false)}
      onDrop={(e) => {
        const id = e.dataTransfer.getData("application/x-shuttlecraft-tab");
        setDragTargetActive(false);
        if (!id) return;
        e.preventDefault();
        moveTab(id, paneId);
      }}
    >
      <TabStrip
        paneId={paneId}
        tabIds={tabIds}
        activeId={activeId}
        tabs={tabs}
        onActivate={(id) => activateTab(paneId, id)}
        onClose={closeTab}
        onDropTab={(id, index) => moveTab(id, paneId, index)}
      />
      <div className="wa__content">
        {tabIds.length === 0 && <PaneSplash paneId={paneId} />}
        {tabIds.map((id) => {
          const tab = tabs[id];
          if (!tab) return null;
          const visible = id === activeId;
          return (
            <div
              key={id}
              className={
                visible
                  ? "wa__tab wa__tab--active"
                  : "wa__tab"
              }
              aria-hidden={!visible}
            >
              <TabContent tab={tab} />
            </div>
          );
        })}
      </div>
    </div>
  );
}

function PaneSplash({ paneId }: { paneId: PaneId }) {
  const hint =
    paneId === "top"
      ? "Drag a tab here, or click a session in the sidebar."
      : "Drag a tab here, or open a timeline / diff / search tab from the sidebar.";
  return (
    <div className="wa__splash">
      <div className="wa__splash-inner">
        <div className="wa__splash-icon">⊕</div>
        <div className="wa__splash-text">{hint}</div>
      </div>
    </div>
  );
}

function TabStrip({
  paneId,
  tabIds,
  activeId,
  tabs,
  onActivate,
  onClose,
  onDropTab,
}: {
  paneId: PaneId;
  tabIds: string[];
  activeId: string | null;
  tabs: Record<string, TabData>;
  onActivate: (id: string) => void;
  onClose: (id: string) => void;
  onDropTab: (id: string, index: number) => void;
}) {
  const stripRef = useRef<HTMLDivElement>(null);
  const [dragHoverIndex, setDragHoverIndex] = useState<number | null>(null);

  return (
    <div
      ref={stripRef}
      className={`wa__tabs wa__tabs--${paneId}`}
      onDragOver={(e) => {
        if (e.dataTransfer.types.includes("application/x-shuttlecraft-tab")) {
          e.preventDefault();
          e.dataTransfer.dropEffect = "move";
        }
      }}
      onDrop={(e) => {
        const id = e.dataTransfer.getData("application/x-shuttlecraft-tab");
        if (!id) return;
        e.preventDefault();
        onDropTab(id, dragHoverIndex ?? tabIds.length);
        setDragHoverIndex(null);
      }}
    >
      {tabIds.map((id, i) => {
        const tab = tabs[id];
        if (!tab) return null;
        return (
          <TabHandle
            key={id}
            tab={tab}
            paneId={paneId}
            paneTabIds={tabIds}
            index={i}
            active={id === activeId}
            onActivate={() => onActivate(id)}
            onClose={() => onClose(id)}
            onDragOverIndex={() => setDragHoverIndex(i)}
          />
        );
      })}
    </div>
  );
}

function TabHandle({
  tab,
  paneId,
  paneTabIds,
  index,
  active,
  onActivate,
  onClose,
  onDragOverIndex,
}: {
  tab: TabData;
  paneId: PaneId;
  paneTabIds: string[];
  index: number;
  active: boolean;
  onActivate: () => void;
  onClose: () => void;
  onDragOverIndex: () => void;
}) {
  // Derive the label live from session / repo state so renames reflect
  // without touching every open tab's persisted title.
  const { sessions } = useSessions();
  const { closeTab, moveTab } = useTabs();
  const { open: openCtx } = useContextMenu();
  const label = useMemo(() => liveLabel(tab, sessions), [tab, sessions]);

  const onContextMenu = contextMenuHandler(openCtx, () => {
    const otherPane: PaneId = paneId === "top" ? "bottom" : "top";
    const others = paneTabIds.filter((id) => id !== tab.id);
    const toRight = paneTabIds.slice(index + 1);
    const items: MenuItem[] = [
      { kind: "item", id: "close", label: "Close", onSelect: onClose },
      {
        kind: "item",
        id: "close-others",
        label: "Close others",
        disabled: others.length === 0,
        onSelect: () => others.forEach((id) => closeTab(id)),
      },
      {
        kind: "item",
        id: "close-right",
        label: "Close all to the right",
        disabled: toRight.length === 0,
        onSelect: () => toRight.forEach((id) => closeTab(id)),
      },
      { kind: "separator" },
      {
        kind: "item",
        id: "move-pane",
        label: `Move to ${otherPane} pane`,
        onSelect: () => moveTab(tab.id, otherPane),
      },
    ];
    return items;
  });

  return (
    <div
      className={active ? "wa__tab-handle wa__tab-handle--active" : "wa__tab-handle"}
      draggable
      onDragStart={(e) => {
        e.dataTransfer.setData("application/x-shuttlecraft-tab", tab.id);
        e.dataTransfer.effectAllowed = "move";
      }}
      onDragOver={(e) => {
        if (e.dataTransfer.types.includes("application/x-shuttlecraft-tab")) {
          e.preventDefault();
          onDragOverIndex();
        }
      }}
      onClick={onActivate}
      onContextMenu={onContextMenu}
      title={tabTitle(tab, label)}
    >
      <span className={`wa__tab-kind wa__tab-kind--${tab.kind}`} aria-hidden>
        {tabIcon(tab)}
      </span>
      <span className="wa__tab-label">{label}</span>
      <button
        type="button"
        className="wa__tab-close"
        onClick={(e) => {
          e.stopPropagation();
          onClose();
        }}
        aria-label="Close tab"
        title="Close this view (PTY keeps running; delete from the sidebar to stop the session)"
      >
        ×
      </button>
    </div>
  );
}

/** The tab label shown in the strip. Session-bound kinds pick up the
 * session's label or its short id, so renames in the sidebar flow
 * straight through to the tab without a store round-trip. */
function liveLabel(
  tab: TabData,
  sessions: ReturnType<typeof useSessions>["sessions"],
): string {
  const session =
    tab.sessionId ? sessions.find((s) => s.id === tab.sessionId) ?? null : null;
  const sessionTag = session
    ? session.label && session.label.length > 0
      ? session.label
      : session.id.slice(0, 8)
    : null;
  switch (tab.kind) {
    case "terminal":
      return sessionTag ? `${sessionTag} · term` : "terminal";
    case "timeline":
      return sessionTag ? `${sessionTag} · time` : "timeline";
    case "file":
      return tab.path ? basename(tab.path) : "file";
    case "diff":
      return tab.path ? `diff: ${basename(tab.path)}` : `diff: ${tab.repo ?? ""}`;
    case "search":
      return "search";
  }
}

function basename(p: string): string {
  const i = p.lastIndexOf("/");
  return i === -1 ? p : p.slice(i + 1);
}

function tabIcon(tab: TabData): string {
  switch (tab.kind) {
    case "terminal":
      return "▸_";
    case "timeline":
      return "≡";
    case "file":
      return "◫";
    case "diff":
      return "±";
    case "search":
      return "⌕";
  }
}

function tabTitle(tab: TabData, label: string): string {
  const bits: string[] = [label, tab.kind];
  if (tab.sessionId) bits.push(tab.sessionId.slice(0, 8));
  if (tab.repo) bits.push(tab.repo);
  if (tab.path) bits.push(tab.path);
  return bits.join(" · ");
}

function TabContent({ tab }: { tab: TabData }) {
  return useMemo(() => {
    switch (tab.kind) {
      case "terminal":
        return <TerminalOrEndedPane sessionId={tab.sessionId!} />;
      case "timeline":
        return <TimelinePane sessionId={tab.sessionId!} />;
      case "file":
        return <FileTab repo={tab.repo!} path={tab.path!} />;
      case "diff":
        return <DiffTab repo={tab.repo!} path={tab.path} />;
      case "search":
        // SearchTab owns its own query + scope state entirely. No seed
        // from the registry — a search tab is its own process with its
        // own internal state that survives tab-strip churn.
        return <SearchTab />;
    }
  }, [tab]);
}

function TerminalOrEndedPane({ sessionId }: { sessionId: string }) {
  const { sessions, sessionsLoaded } = useSessions();
  const s = sessions.find((x) => x.id === sessionId) ?? null;
  // Sessions not loaded yet → render the terminal optimistically; the
  // WS will connect once things stabilise.
  if (!sessionsLoaded) return <TerminalPane sessionId={sessionId} />;
  if (!s) {
    return (
      <div className="wa__orphan">
        <p>This tab's session (<code>{sessionId.slice(0, 8)}</code>) is no longer available.</p>
        <p>Close the tab via the × button, or open a fresh session from the sidebar.</p>
      </div>
    );
  }
  if (s.state !== "live") return <SessionEndedPane session={s} />;
  return <TerminalPane sessionId={sessionId} />;
}
