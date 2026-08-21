// Root-level renderer for all tab content. Each tab renders exactly
// once, through a portal, into its stable host element from the
// registry. Layout surfaces (panes, the peek overlay) render TabSlot
// containers that adopt the host by reference — content instances
// survive pane drags, display-mode switches, and peeks.
//
// The `active` prop that gates polling (timeline, monitor, metrics) is
// computed here from what is actually presented somewhere on screen.

import { useEffect, useMemo, useRef } from "react";
import { createPortal } from "react-dom";
import { useShallow } from "zustand/react/shallow";

import type { TabData } from "../../state/TabStore";
import { useTabs } from "../../state/TabStore";
import { useSessions } from "../../state/SessionStore";
import { useDisplay } from "../../state/DisplayStore";
import { useMediaQuery } from "../../hooks/useMediaQuery";
import { computeShownTabIds, usePeekTabId } from "./shown";
import {
  claimHost,
  hostFor,
  registeredHostIds,
  releaseHost,
} from "./registry";
import { TerminalPane } from "../TerminalPane";
import { TimelinePane } from "../TimelinePane";
import { MetricsPane } from "../MetricsPane";
import { MonitorPane } from "../MonitorPane";
import { SessionEndedPane } from "../SessionEndedPane";
import { FileTab } from "../FileTab";
import { DiffTab } from "../DiffTab";
import { RefTab } from "../RefTab";
import { SecretsTab } from "../SecretsTab";
import "./TabHost.css";

export function TabHost() {
  const { tabs, panes, activeByPane } = useTabs(
    useShallow((store) => ({
      tabs: store.tabs,
      panes: store.panes,
      activeByPane: store.activeByPane,
    })),
  );
  const mode = useDisplay((store) => store.mode);
  const isMobile = useMediaQuery("(max-width: 767px)");
  const peekTabId = usePeekTabId();

  const shownIds = useMemo(
    () =>
      computeShownTabIds({ tabs, panes, activeByPane }, mode, isMobile, peekTabId),
    [tabs, panes, activeByPane, mode, isMobile, peekTabId],
  );

  // Closed tabs release their host here, against the live tab set —
  // never from portal cleanup, which StrictMode also runs on mounted
  // components and would strand the portal in a deleted element.
  useEffect(() => {
    for (const id of registeredHostIds()) {
      if (!tabs[id]) releaseHost(id);
    }
  }, [tabs]);

  return (
    <>
      {Object.values(tabs).map((tab) => (
        <TabPortal key={tab.id} tab={tab} active={shownIds.has(tab.id)} />
      ))}
    </>
  );
}

function TabPortal({ tab, active }: { tab: TabData; active: boolean }) {
  const host = useMemo(() => hostFor(tab.id), [tab.id]);
  return createPortal(<TabContent tab={tab} active={active} />, host);
}

/** Empty container that adopts a tab's host element while mounted. The
 * peek overlay claims at a higher priority than panes and wins the host
 * for as long as it is open. */
export function TabSlot({
  tabId,
  priority = 1,
  className = "tab-slot",
}: {
  tabId: string;
  priority?: number;
  className?: string;
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const container = ref.current;
    if (!container) return;
    return claimHost(tabId, container, priority);
  }, [tabId, priority]);
  return <div ref={ref} className={className} data-testid="tab-slot" />;
}

function TabContent({ tab, active }: { tab: TabData; active: boolean }) {
  return useMemo(() => {
    switch (tab.kind) {
      case "terminal":
        return <TerminalOrEndedPane sessionId={tab.sessionId!} />;
      case "timeline":
        return (
          <TimelinePane
            tabId={tab.id}
            sessionId={tab.sessionId}
            repo={tab.repo}
            active={active}
            focusTurnId={tab.focusTurnId}
            focusPairId={tab.focusPairId}
            focusKey={tab.focusKey}
          />
        );
      case "file":
        return (
          <FileTab
            repo={tab.repo!}
            path={tab.path!}
            workspaceId={tab.workspaceId}
            focusLine={tab.focusLine}
            focusKey={tab.focusKey}
          />
        );
      case "diff":
        return <DiffTab repo={tab.repo!} path={tab.path} workspaceId={tab.workspaceId} />;
      case "ref":
        return <RefTab slug={tab.slug!} />;
      case "secrets":
        return <SecretsTab />;
      case "monitor":
        return <MonitorPane active={active} />;
      case "metrics":
        return <MetricsPane active={active} />;
    }
  }, [active, tab]);
}

export function TerminalOrEndedPane({ sessionId }: { sessionId: string }) {
  const { sessions, sessionsLoaded } = useSessions(
    useShallow((store) => ({
      sessions: store.sessions,
      sessionsLoaded: store.sessionsLoaded,
    })),
  );
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
