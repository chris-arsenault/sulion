// Two-pane tab system. Top pane / bottom pane, resizable divider,
// tabs drag within + between. Tab content is mounted and hidden via
// CSS visibility (not display:none) so xterm and virtuoso keep their
// state across tab switches.
//
// Mobile mode (viewport <768px) collapses to a single-pane strip
// showing every open tab mixed together, no divider.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import type { SecretGrantMetadata, SessionView } from "../api/types";
import type { PaneId, TabData } from "../state/TabStore";
import { useTabs } from "../state/TabStore";
import { useSessions } from "../state/SessionStore";
import { useSecretStore } from "../state/SecretStore";
import { useMediaQuery } from "../hooks/useMediaQuery";
import { Icon, type IconName } from "../icons";
import { Tooltip } from "./ui";
import type { MenuItem } from "./common/contextMenuStore";
import {
  contextMenuHandler,
  useContextMenu,
} from "./common/contextMenuStore";
import { buildSecretContextMenu } from "./common/secretContextMenu";
import { TerminalPane } from "./TerminalPane";
import { TimelinePane } from "./TimelinePane";
import { MonitorPane } from "./MonitorPane";
import { SessionEndedPane } from "./SessionEndedPane";
import { FileTab } from "./FileTab";
import { DiffTab } from "./DiffTab";
import { RefTab } from "./RefTab";
import { SecretsTab } from "./SecretsTab";
import "./WorkArea.css";

const TAB_DRAG_MIME = "application/x-sulion-tab";
const EMPTY_SECRET_GRANTS: SecretGrantMetadata[] = [];

export function WorkArea() {
  const { panes, activeByPane, tabs } = useTabs(
    useShallow((store) => ({
      panes: store.panes,
      activeByPane: store.activeByPane,
      tabs: store.tabs,
    })),
  );
  const isMobile = useMediaQuery("(max-width: 767px)");
  const [topFraction, setTopFraction] = useState(0.55);
  // No more automatic prune: tabs survive their session's deletion and
  // show an orphan placeholder with a manual close. That keeps tab
  // state fully user-driven and avoids the "refresh wipes everything"
  // failure mode.

  const mobileTabIds = useMemo(
    () => [...panes.top, ...panes.bottom],
    [panes.top, panes.bottom],
  );

  const splitStyle = useMemo(
    () => ({
      gridTemplateRows: `${topFraction}fr 6px ${1 - topFraction}fr`,
    }),
    [topFraction],
  );

  if (isMobile) {
    const activeId =
      activeByPane.top ?? activeByPane.bottom ?? mobileTabIds[0] ?? null;
    if (mobileTabIds.length === 0) {
      return <EmptyWorkArea mobile />;
    }
    return (
      <div className="wa wa--mobile">
        <Pane
          paneId="top"
          tabIds={mobileTabIds}
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
      style={splitStyle}
    >
      <Pane
        paneId="top"
        tabIds={panes.top}
        activeId={activeByPane.top}
        tabs={tabs}
      />
      <Divider fraction={topFraction} onDrag={setTopFraction} />
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
      <div className="wa__empty-inner">
        <div className="wa__empty-sigil" aria-hidden>
          <Icon name="terminal" size={20} />
        </div>
        <h1>sulion</h1>
        <p className="wa__empty-sub">
          {mobile
            ? "Open the drawer to pick a session."
            : "Choose a session from the sidebar, or create one to begin."}
        </p>
        <p className="wa__empty-hint">
          <span className="wa__empty-kbd">⌘K</span>
          <span>command palette · jump to repo / session</span>
        </p>
      </div>
    </div>
  );
}

function Divider({
  fraction,
  onDrag,
}: {
  fraction: number;
  onDrag: (f: number) => void;
}) {
  const onMouseDown = useCallback(
    (e: React.MouseEvent) => {
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
    },
    [onDrag],
  );
  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      const step = e.shiftKey ? 0.1 : 0.03;
      if (e.key === "ArrowUp") {
        e.preventDefault();
        onDrag(Math.max(0.15, fraction - step));
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        onDrag(Math.min(0.85, fraction + step));
      }
    },
    [fraction, onDrag],
  );
  return (
    <div
      className="wa__divider"
      role="slider"
      aria-orientation="horizontal"
      aria-label="Resize top and bottom panes"
      aria-valuemin={15}
      aria-valuemax={85}
      aria-valuenow={Math.round(fraction * 100)}
      tabIndex={0}
      onMouseDown={onMouseDown}
      onKeyDown={onKeyDown}
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
  const { activateTab, closeTab, moveTab } = useTabs(
    useShallow((store) => ({
      activateTab: store.activateTab,
      closeTab: store.closeTab,
      moveTab: store.moveTab,
    })),
  );
  const [dragTargetActive, setDragTargetActive] = useState(false);

  const onActivate = useCallback(
    (id: string) => activateTab(paneId, id),
    [activateTab, paneId],
  );
  const onDropTab = useCallback(
    (id: string, index: number) => moveTab(id, paneId, index),
    [moveTab, paneId],
  );
  const onDropZoneDragOver = useCallback((e: React.DragEvent) => {
    if (e.dataTransfer.types.includes(TAB_DRAG_MIME)) {
      e.preventDefault();
      setDragTargetActive(true);
    }
  }, []);
  const onDropZoneDragLeave = useCallback(
    () => setDragTargetActive(false),
    [],
  );
  const onDropZoneDrop = useCallback(
    (e: React.DragEvent) => {
      const id = e.dataTransfer.getData(TAB_DRAG_MIME);
      setDragTargetActive(false);
      if (!id) return;
      e.preventDefault();
      moveTab(id, paneId);
    },
    [moveTab, paneId],
  );

  return (
    <div
      className={`wa__pane${mobile ? " wa__pane--mobile" : ""}${
        dragTargetActive ? " wa__pane--drop-target" : ""
      }`}
    >
      <TabStrip
        paneId={paneId}
        tabIds={tabIds}
        activeId={activeId}
        tabs={tabs}
        onActivate={onActivate}
        onClose={closeTab}
        onDropTab={onDropTab}
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
              <TabContent tab={tab} active={visible} />
            </div>
          );
        })}
        <button
          type="button"
          className="wa__drop-zone"
          aria-label={`Drop tab into ${paneId} pane`}
          onDragOver={onDropZoneDragOver}
          onDragLeave={onDropZoneDragLeave}
          onDrop={onDropZoneDrop}
        />
      </div>
    </div>
  );
}

function PaneSplash({ paneId }: { paneId: PaneId }) {
  const hint =
    paneId === "top"
      ? "Drag a tab here, or click a session in the sidebar."
      : "Drag a tab here, or open a timeline / diff / file tab from the sidebar.";
  return (
    <div className="wa__splash">
      <div className="wa__splash-inner">
        <div className="wa__splash-icon">
          <Icon name="plus" size={20} />
        </div>
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

  const onStripDragOver = useCallback((e: React.DragEvent) => {
    if (e.dataTransfer.types.includes(TAB_DRAG_MIME)) {
      e.preventDefault();
      e.dataTransfer.dropEffect = "move";
    }
  }, []);
  const onStripDrop = useCallback(
    (e: React.DragEvent) => {
      const id = e.dataTransfer.getData(TAB_DRAG_MIME);
      if (!id) return;
      e.preventDefault();
      onDropTab(id, dragHoverIndex ?? tabIds.length);
      setDragHoverIndex(null);
    },
    [onDropTab, dragHoverIndex, tabIds.length],
  );
  const onDragOverIndex = useCallback(
    (i: number) => setDragHoverIndex(i),
    [],
  );

  return (
    <div
      ref={stripRef}
      role="tablist"
      aria-label={`${paneId} pane tabs`}
      tabIndex={-1}
      className={`wa__tabs wa__tabs--${paneId}`}
      onDragOver={onStripDragOver}
      onDrop={onStripDrop}
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
            onActivate={onActivate}
            onClose={onClose}
            onDragOverIndex={onDragOverIndex}
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
  onActivate: (id: string) => void;
  onClose: (id: string) => void;
  onDragOverIndex: (index: number) => void;
}) {
  const activateThis = useCallback(
    () => onActivate(tab.id),
    [onActivate, tab.id],
  );
  const closeThis = useCallback(() => onClose(tab.id), [onClose, tab.id]);
  const dragOverIndexThis = useCallback(
    () => onDragOverIndex(index),
    [onDragOverIndex, index],
  );
  // Derive the label live from session / repo state so renames reflect
  // without touching every open tab's persisted title.
  const sessions = useSessions((store) => store.sessions);
  const { closeTab, moveTab, openTab, setPaneSticky, paneSticky } = useTabs(
    useShallow((store) => ({
      closeTab: store.closeTab,
      moveTab: store.moveTab,
      openTab: store.openTab,
      setPaneSticky: store.setPaneSticky,
      paneSticky: store.sticky[paneId],
    })),
  );
  const {
    secrets,
    grants,
    refreshSecrets,
    refreshGrants,
    enableGrant,
    revokeGrant,
  } = useSecretStore(
    useShallow((store) => ({
      secrets: store.secrets,
      grants: tab.sessionId
        ? (store.grantsBySession[tab.sessionId] ?? EMPTY_SECRET_GRANTS)
        : EMPTY_SECRET_GRANTS,
      refreshSecrets: store.refreshSecrets,
      refreshGrants: store.refreshGrants,
      enableGrant: store.enableGrant,
      revokeGrant: store.revokeGrant,
    })),
  );
  const openCtx = useContextMenu((store) => store.open);
  const label = useMemo(() => liveLabel(tab, sessions), [tab, sessions]);

  const pairLinkable = tab.kind === "terminal" || tab.kind === "timeline";
  useEffect(() => {
    if (tab.kind !== "terminal" || !tab.sessionId) return;
    void refreshSecrets().catch(() => undefined);
    void refreshGrants(tab.sessionId).catch(() => undefined);
  }, [refreshGrants, refreshSecrets, tab.kind, tab.sessionId]);

  const openSecrets = useCallback(() => {
    if (!tab.sessionId) return;
    openTab({ kind: "secrets", sessionId: tab.sessionId }, paneId);
  }, [openTab, paneId, tab.sessionId]);
  const enableSecret = useCallback(
    (secretId: string, tool: "with-cred" | "aws", ttlSeconds: number) => {
      if (!tab.sessionId) return;
      void enableGrant(tab.sessionId, secretId, tool, ttlSeconds).catch(() => undefined);
    },
    [enableGrant, tab.sessionId],
  );
  const revokeSecret = useCallback(
    (secretId: string, tool: "with-cred" | "aws") => {
      if (!tab.sessionId) return;
      void revokeGrant(tab.sessionId, secretId, tool).catch(() => undefined);
    },
    [revokeGrant, tab.sessionId],
  );
  const secretMenu = useMemo(
    () =>
      buildSecretContextMenu({
        secrets,
        grants,
        onEnable: enableSecret,
        onRevoke: revokeSecret,
        onOpenManager: openSecrets,
      }),
    [enableSecret, grants, openSecrets, revokeSecret, secrets],
  );

  const buildMenuItems = useCallback((): MenuItem[] => {
    const otherPane: PaneId = paneId === "top" ? "bottom" : "top";
    const others = paneTabIds.filter((id) => id !== tab.id);
    const toRight = paneTabIds.slice(index + 1);
    const items: MenuItem[] = [
      { kind: "item", id: "close", label: "Close", onSelect: closeThis },
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
    if (pairLinkable) {
      items.push({ kind: "separator" });
      items.push({
        kind: "item",
        id: "pane-sticky",
        label: paneSticky
          ? `Release ${paneId} pane (allow paired switches)`
          : `Keep ${paneId} pane sticky (no paired switches)`,
        onSelect: () => setPaneSticky(paneId, !paneSticky),
      });
    }
    if (tab.kind === "terminal" && tab.sessionId) {
      items.push({ kind: "separator" });
      items.push(secretMenu);
    }
    return items;
  }, [
    paneId,
    paneTabIds,
    tab.id,
    index,
    closeThis,
    closeTab,
    moveTab,
    pairLinkable,
    paneSticky,
    secretMenu,
    setPaneSticky,
    tab.kind,
    tab.sessionId,
  ]);
  const onContextMenu = useMemo(
    () => contextMenuHandler(openCtx, buildMenuItems),
    [openCtx, buildMenuItems],
  );

  const boundSession = tab.sessionId
    ? sessions.find((s) => s.id === tab.sessionId) ?? null
    : null;
  const bindingTone = boundSession
    ? boundSession.state === "live"
      ? "ok"
      : boundSession.state === "orphaned"
        ? "atn"
        : "crit"
    : null;

  const onDragStart = useCallback(
    (e: React.DragEvent) => {
      e.dataTransfer.setData(TAB_DRAG_MIME, tab.id);
      e.dataTransfer.effectAllowed = "move";
      document.body.classList.add("is-dragging-tab");
    },
    [tab.id],
  );
  const onDragEnd = useCallback(() => {
    document.body.classList.remove("is-dragging-tab");
  }, []);
  const onDragOver = useCallback(
    (e: React.DragEvent) => {
      if (e.dataTransfer.types.includes(TAB_DRAG_MIME)) {
        e.preventDefault();
        dragOverIndexThis();
      }
    },
    [dragOverIndexThis],
  );
  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        activateThis();
      }
    },
    [activateThis],
  );
  const onCloseClick = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      closeThis();
    },
    [closeThis],
  );

  return (
    <Tooltip label={tabTitle(tab, label)}>
      <div
        role="tab"
        aria-selected={active}
        tabIndex={active ? 0 : -1}
        className={active ? "wa__tab-handle wa__tab-handle--active" : "wa__tab-handle"}
        data-kind={tab.kind}
        data-session-id={tab.sessionId}
        data-repo={tab.repo}
        data-workspace-id={tab.workspaceId}
        data-path={tab.path}
        data-slug={tab.slug}
        draggable
        onDragStart={onDragStart}
        onDragEnd={onDragEnd}
        onDragOver={onDragOver}
        onClick={activateThis}
        onKeyDown={onKeyDown}
        onContextMenu={onContextMenu}
      >
        {bindingTone ? (
          <span
            className={`wa__tab-bind wa__tab-bind--${bindingTone}`}
            aria-hidden
          />
        ) : null}
        <span className={`wa__tab-kind wa__tab-kind--${tab.kind}`} aria-hidden>
          <Icon name={tabIcon(tab)} size={12} />
        </span>
        <span className="wa__tab-label">{label}</span>
        {active && pairLinkable && paneSticky && (
          <span
            className="wa__tab-sticky"
            aria-label={`${paneId} pane sticky — paired switches disabled`}
          >
            <Icon name="pin" size={12} />
          </span>
        )}
        <Tooltip label="Close this view (PTY keeps running; delete from the sidebar to stop the session)">
          <button
            type="button"
            className="wa__tab-close"
            onClick={onCloseClick}
            aria-label="Close tab"
          >
            <Icon name="x" size={12} />
          </button>
        </Tooltip>
      </div>
    </Tooltip>
  );
}

/** The tab label shown in the strip. Session-bound kinds pick up the
 * session's label or its short id, so renames in the sidebar flow
 * straight through to the tab without a store round-trip. */
function liveLabel(
  tab: TabData,
  sessions: SessionView[],
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
      if (tab.repo) return `${tab.repo} · time`;
      return sessionTag ? `${sessionTag} · time` : "timeline";
    case "file":
      return tab.path
        ? `${tab.workspaceId ? "ws · " : ""}${basename(tab.path)}`
        : "file";
    case "diff":
      return tab.path
        ? `${tab.workspaceId ? "ws " : ""}diff: ${basename(tab.path)}`
        : `${tab.workspaceId ? "ws " : ""}diff: ${tab.repo ?? ""}`;
    case "ref":
      return tab.slug ? `ref: ${tab.slug}` : "ref";
    case "secrets":
      return tab.sessionId ? `secrets · ${tab.sessionId.slice(0, 8)}` : "secrets";
    case "monitor":
      return "monitor";
  }
}

function basename(p: string): string {
  const i = p.lastIndexOf("/");
  return i === -1 ? p : p.slice(i + 1);
}

function tabIcon(tab: TabData): IconName {
  switch (tab.kind) {
    case "terminal":
      return "terminal";
    case "timeline":
      return "list";
    case "file":
      return "file-text";
    case "diff":
      return "diff";
    case "ref":
      return "pin";
    case "secrets":
      return "settings";
    case "monitor":
      return "activity";
  }
}

function tabTitle(tab: TabData, label: string): string {
  const bits: string[] = [label, tab.kind];
  if (tab.sessionId) bits.push(tab.sessionId.slice(0, 8));
  if (tab.repo) bits.push(tab.repo);
  if (tab.workspaceId) bits.push(`workspace ${tab.workspaceId.slice(0, 8)}`);
  if (tab.path) bits.push(tab.path);
  return bits.join(" · ");
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
          <FileTab repo={tab.repo!} path={tab.path!} workspaceId={tab.workspaceId} />
        );
      case "diff":
        return <DiffTab repo={tab.repo!} path={tab.path} workspaceId={tab.workspaceId} />;
      case "ref":
        return <RefTab slug={tab.slug!} />;
      case "secrets":
        return <SecretsTab />;
      case "monitor":
        return <MonitorPane active={active} />;
    }
  }, [active, tab]);
}

function TerminalOrEndedPane({ sessionId }: { sessionId: string }) {
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
