import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { PlanSummaryView } from "../api/types";
import { Icon } from "../icons";
import { Rail } from "./Rail";
import { Sidebar } from "./Sidebar";
import { WorkArea } from "./WorkArea";
import { FuturePromptsModal } from "./FuturePromptsModal";
import { PlanModal } from "./PlanModal";
import { MetricsPane } from "./MetricsPane";
import { MonitorPane } from "./MonitorPane";
import { DisplaySettings } from "./DisplaySettings";
import { useMediaQuery } from "../hooks/useMediaQuery";
import {
  useIsMobileLayout,
  useSessionNavigation,
} from "../hooks/useSessionNavigation";
import { appCommands, useAppCommand } from "../state/AppCommands";
import { useTabs } from "../state/TabStore";
import { useSessions } from "../state/SessionStore";
import {
  DISPLAY_MODE_LABELS,
  DISPLAY_MODES,
  useDisplay,
} from "../state/DisplayStore";
import { MOBILE_LAYOUT_QUERY } from "../state/displayPolicy";
import { TabHost, TabSlot } from "./tabhost/TabHost";
import { peekSessionIdFrom, usePeekTabId } from "./tabhost/shown";
import { CommandPalette, Overlay, type PaletteCommand } from "./ui";
import "./Layout.css";

/** Monotonic counter making each file focus request unique so repeated
 * jumps to the same line re-trigger the scroll. */
let fileFocusSeq = 0;

const WIDTH_STORAGE_KEY = "sulion.sidebar.width.v1";
const DEFAULT_WIDTH = 280;
const MIN_WIDTH = 220;
const MAX_WIDTH = 420;

function readInt(key: string, fallback: number): number {
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return fallback;
    const n = Number.parseInt(raw, 10);
    return Number.isFinite(n) ? n : fallback;
  } catch {
    return fallback;
  }
}

/** Root layout: rail + sidebar + work area. On mobile the rail disappears
 * and the sidebar becomes a drawer. The split / tab system lives inside
 * WorkArea. */
export function Layout() {
  const openTab = useTabs((store) => store.openTab);
  const isMobile = useMediaQuery(MOBILE_LAYOUT_QUERY);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [monitorOverlayOpen, setMonitorOverlayOpen] = useState(false);
  const [monitorView, setMonitorView] = useState<MonitorOverlayView>("overview");
  const [futurePromptsSessionId, setFuturePromptsSessionId] = useState<string | null>(null);
  const [planTarget, setPlanTarget] = useState<{
    repo: string;
    planId?: string;
  } | null>(null);

  const pinned = useDisplay((store) => store.sidebarPinned);
  const toggleSidebar = useDisplay((store) => store.toggleSidebar);
  const displayMode = useDisplay((store) => store.mode);
  const peekOpen = useDisplay((store) => store.peekOpen);
  const closePeek = useDisplay((store) => store.closePeek);
  const selectedSessionId = useSessions((store) => store.selectedSessionId);
  const peekSessionId = useTabs((store) =>
    peekSessionIdFrom(store, selectedSessionId),
  );
  const peekTabId = usePeekTabId();

  const [sidebarWidth, setSidebarWidth] = useState<number>(() =>
    readInt(WIDTH_STORAGE_KEY, DEFAULT_WIDTH),
  );
  useEffect(() => {
    try {
      localStorage.setItem(WIDTH_STORAGE_KEY, String(sidebarWidth));
    } catch {
      /* ignore */
    }
  }, [sidebarWidth]);

  const openTabRef = useRef(openTab);
  openTabRef.current = openTab;

  // The peek adopts the counterpart tab's live host. If the session has
  // no counterpart tab open yet, open one — hidden from the strip in the
  // current mode, but its content mounts under TabHost for adoption.
  useEffect(() => {
    if (isMobile || !peekOpen || displayMode === "split" || peekTabId) return;
    if (!peekSessionId) return;
    openTabRef.current({
      kind: displayMode === "terminal" ? "timeline" : "terminal",
      sessionId: peekSessionId,
    });
  }, [isMobile, peekOpen, displayMode, peekTabId, peekSessionId]);

  useEffect(() => {
    if (isMobile && peekOpen) closePeek();
  }, [closePeek, isMobile, peekOpen]);

  useAppCommand("open-file", ({ repo, path, workspaceId, line }) => {
    openTabRef.current({
      kind: "file",
      repo,
      path,
      workspaceId,
      focusLine: line,
      // A fresh key on every request so revisiting the same file at the
      // same line still re-scrolls, mirroring timeline focus.
      focusKey: line != null ? `${line}:${++fileFocusSeq}` : undefined,
    });
    setDrawerOpen(false);
    // Reveal the file behind any transient modal (e.g. the plan modal a
    // file:line reference was clicked from).
    setPlanTarget(null);
  });
  useAppCommand("open-diff", ({ repo, path, workspaceId }) => {
    openTabRef.current({ kind: "diff", repo, path, workspaceId });
    setDrawerOpen(false);
  });
  useAppCommand("open-future-prompts", ({ sessionId }) => {
    setFuturePromptsSessionId(sessionId);
    setDrawerOpen(false);
  });
  useAppCommand("open-plan", ({ repo, planId }) => {
    setPlanTarget({ repo, planId });
    setDrawerOpen(false);
  });
  useAppCommand("close-drawer", () => {
    setDrawerOpen(false);
  });
  useAppCommand("open-display-settings", () => {
    setMonitorView("display");
    setMonitorOverlayOpen(true);
  });

  // ⌘K / Ctrl-K opens the command palette. ⌘M / Ctrl-M toggles the monitor
  // overlay. ⌘⇧B toggles the sidebar, ⌘⇧D cycles the display mode, and
  // ⌘⇧E peeks at the hidden projection in the single-pane modes.
  // (TerminalPane excludes these chords from xterm so they never reach
  // the PTY.) Esc is handled by the Overlay.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.altKey) return;
      const key = e.key.toLowerCase();
      if (!e.shiftKey && key === "k") {
        e.preventDefault();
        setPaletteOpen(true);
      }
      if (!e.shiftKey && key === "m") {
        e.preventDefault();
        setMonitorOverlayOpen((open) => !open);
      }
      if (!isMobile && e.shiftKey && key === "b") {
        e.preventDefault();
        useDisplay.getState().toggleSidebar();
      }
      if (!isMobile && e.shiftKey && key === "d") {
        e.preventDefault();
        useDisplay.getState().cycleMode();
      }
      if (!isMobile && e.shiftKey && key === "e") {
        e.preventDefault();
        useDisplay.getState().togglePeek();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [isMobile]);

  const openPalette = useCallback(() => setPaletteOpen(true), []);
  const openSecrets = useCallback(() => {
    openTabRef.current({ kind: "secrets" }, "top");
    setDrawerOpen(false);
  }, []);
  const openMonitor = useCallback(() => {
    openTabRef.current({ kind: "monitor" }, "top");
    setDrawerOpen(false);
  }, []);
  const closePalette = useCallback(() => setPaletteOpen(false), []);
  const closeMonitorOverlay = useCallback(() => setMonitorOverlayOpen(false), []);
  const closeFuturePrompts = useCallback(() => setFuturePromptsSessionId(null), []);
  const closePlan = useCallback(() => setPlanTarget(null), []);
  const openDrawer = useCallback(() => setDrawerOpen(true), []);
  const closeDrawerLocal = useCallback(() => setDrawerOpen(false), []);

  const commands = usePaletteCommands({
    onOpenPalette: openPalette,
  });

  const layoutStyle = useMemo(
    () =>
      ({
        "--sulion-sidebar-width": `${sidebarWidth}px`,
      }) as React.CSSProperties,
    [sidebarWidth],
  );

  if (isMobile) {
    return (
      <div className="layout layout--mobile">
        <TabHost />
        <MobileTopBar onOpenDrawer={openDrawer} />
        {drawerOpen && (
          <>
            <div
              className="layout__scrim"
              onClick={closeDrawerLocal}
              aria-hidden
            />
            <aside className="layout__drawer" aria-label="Sessions">
              <Sidebar />
            </aside>
          </>
        )}
        <main className="layout__main">
          <WorkArea />
        </main>
        <CommandPalette
          open={paletteOpen}
          onClose={closePalette}
          commands={commands}
        />
        <MonitorOverlay
        open={monitorOverlayOpen}
        view={monitorView}
        onViewChange={setMonitorView}
        onClose={closeMonitorOverlay}
      />
        <FuturePromptsModal
          open={futurePromptsSessionId !== null}
          sessionId={futurePromptsSessionId}
          onClose={closeFuturePrompts}
        />
        <PlanModal
          open={planTarget !== null}
          repo={planTarget?.repo ?? null}
          planId={planTarget?.planId}
          onClose={closePlan}
        />
      </div>
    );
  }

  return (
    <div
      className={`layout ${pinned ? "layout--pinned" : "layout--collapsed"}`}
      // eslint-disable-next-line local/no-inline-styles -- sidebar width is drag-resized per user; pass-through CSS custom property drives the grid-template-columns
      style={layoutStyle}
    >
      <TabHost />
      <Rail
        pinned={pinned}
        onTogglePinned={toggleSidebar}
        onOpenMonitor={openMonitor}
        onOpenSecrets={openSecrets}
        onOpenPalette={openPalette}
      />
      {pinned ? (
        <>
          <aside className="layout__sidebar" aria-label="Workspace">
            <Sidebar />
          </aside>
          <SidebarResizer width={sidebarWidth} onChange={setSidebarWidth} />
        </>
      ) : null}
      <main className="layout__main">
        <WorkArea />
      </main>
      <CommandPalette
        open={paletteOpen}
        onClose={closePalette}
        commands={commands}
      />
      <MonitorOverlay
        open={monitorOverlayOpen}
        view={monitorView}
        onViewChange={setMonitorView}
        onClose={closeMonitorOverlay}
      />
      {displayMode !== "split" && (
        <PeekOverlay
          open={peekOpen}
          hidden={displayMode === "terminal" ? "timeline" : "terminal"}
          tabId={peekTabId}
          onClose={closePeek}
        />
      )}
      <FuturePromptsModal
        open={futurePromptsSessionId !== null}
        sessionId={futurePromptsSessionId}
        onClose={closeFuturePrompts}
      />
      <PlanModal
        open={planTarget !== null}
        repo={planTarget?.repo ?? null}
        planId={planTarget?.planId}
        onClose={closePlan}
      />
    </div>
  );
}

/** The projection the current display mode hides, shown transiently over
 * the workspace (⌘⇧E / Ctrl-⇧E toggles it). The overlay adopts the
 * counterpart tab's live host from the tab registry — the same terminal
 * instance the split layout shows, not a fresh attachment — so scrollback
 * and connection state carry over. */
function PeekOverlay({
  open,
  hidden,
  tabId,
  onClose,
}: {
  open: boolean;
  hidden: "terminal" | "timeline";
  tabId: string | null;
  onClose: () => void;
}) {
  const title = hidden === "terminal" ? "Terminal" : "Timeline";
  return (
    <Overlay
      open={open}
      onClose={onClose}
      modal
      className="peek-overlay"
      title={title}
      width="min(94vw, 1400px)"
      maxWidth="94vw"
      maxHeight="92vh"
    >
      {!tabId ? (
        <div className="peek-overlay__empty">
          No session tab is active to peek at.
        </div>
      ) : (
        <div className="peek-overlay__body">
          <TabSlot tabId={tabId} priority={2} className="tab-slot tab-slot--peek" />
        </div>
      )}
    </Overlay>
  );
}

type MonitorOverlayView = "overview" | "metrics" | "display";

/** The monitor as a transient modal over the workspace (⌘M / Ctrl-M
 * toggles it). Same components as the monitor/metrics tabs; mounting on
 * open is what starts their polling, so a closed overlay costs nothing.
 * The view links swap the overlay's content in place — they never open
 * a workspace tab from the modal. The view is owned by Layout so the
 * open-display-settings command can land directly on the display tab. */
function MonitorOverlay({
  open,
  view,
  onViewChange,
  onClose,
}: {
  open: boolean;
  view: MonitorOverlayView;
  onViewChange: (view: MonitorOverlayView) => void;
  onClose: () => void;
}) {
  useEffect(() => {
    if (!open) onViewChange("overview");
    // eslint-disable-next-line react-hooks/exhaustive-deps -- reset only on close
  }, [open]);
  const showMetrics = useCallback(() => onViewChange("metrics"), [onViewChange]);
  const showOverview = useCallback(() => onViewChange("overview"), [onViewChange]);
  const showDisplay = useCallback(() => onViewChange("display"), [onViewChange]);
  return (
    <Overlay
      open={open}
      onClose={onClose}
      modal
      className="monitor-overlay"
      aria-label="Team overview"
      width="min(96vw, 1720px)"
      maxWidth="96vw"
      maxHeight="92vh"
    >
      {view === "overview" ? (
        <MonitorPane
          onNavigate={onClose}
          onOpenMetrics={showMetrics}
          onOpenDisplay={showDisplay}
        />
      ) : view === "metrics" ? (
        <MetricsPane onOpenOverview={showOverview} onOpenDisplay={showDisplay} />
      ) : (
        <DisplaySettings
          onOpenOverview={showOverview}
          onOpenMetrics={showMetrics}
        />
      )}
    </Overlay>
  );
}

function SidebarResizer({
  width,
  onChange,
}: {
  width: number;
  onChange: (n: number) => void;
}) {
  const dragging = useRef(false);

  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      if (!dragging.current) return;
      const next = Math.max(
        MIN_WIDTH,
        Math.min(MAX_WIDTH, e.clientX - 36 /* rail width */),
      );
      onChange(next);
    };
    const onUp = () => {
      dragging.current = false;
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [onChange]);

  const start = useCallback(() => {
    dragging.current = true;
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
  }, []);

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "ArrowLeft") onChange(Math.max(MIN_WIDTH, width - 12));
      if (e.key === "ArrowRight") onChange(Math.min(MAX_WIDTH, width + 12));
    },
    [onChange, width],
  );

  return (
    <div
      className="layout__resizer"
      role="slider"
      aria-orientation="vertical"
      aria-label="Resize sidebar"
      aria-valuenow={width}
      aria-valuemin={MIN_WIDTH}
      aria-valuemax={MAX_WIDTH}
      tabIndex={0}
      onMouseDown={start}
      onKeyDown={onKeyDown}
    />
  );
}

function MobileTopBar({ onOpenDrawer }: { onOpenDrawer: () => void }) {
  return (
    <div className="mobile-topbar">
      <button
        type="button"
        className="mobile-topbar__hamburger"
        onClick={onOpenDrawer}
        aria-label="Open sessions drawer"
      >
        <Icon name="menu" size={16} />
      </button>
      <span className="mobile-topbar__title">sulion</span>
    </div>
  );
}

/** A branch is only meaningful next to the plan it hangs off, so its palette
 * entry leads with the root title and sorts just under its own root. */
function planCommand(
  plan: PlanSummaryView,
  planTitles: Map<string, string>,
): PaletteCommand {
  const root = plan.depth > 0 ? planTitles.get(plan.root_plan_id) : undefined;
  const trail = root ? `${root} › ${plan.title}` : plan.title;
  return {
    id: `plan.${plan.id}`,
    label: `Open plan · ${plan.repo_name} / ${trail}`,
    icon: plan.depth > 0 ? "git-branch" : "list",
    group: "plan",
    searchOnly: true,
    rank: plan.depth > 0 ? 9 : 10,
    run: () => appCommands.openPlan({ repo: plan.repo_name, planId: plan.id }),
  };
}

function usePaletteCommands({
  onOpenPalette,
}: {
  onOpenPalette: () => void;
}): PaletteCommand[] {
  const repos = useSessions((s) => s.repos);
  const metaRepos = useSessions((s) => s.metaRepos);
  const sessions = useSessions((s) => s.sessions);
  const plans = useSessions((s) => s.plans);
  const openTab = useTabs((s) => s.openTab);
  const openSession = useSessionNavigation();
  const isMobile = useIsMobileLayout();
  // onOpenPalette retained in signature for future surfaces that want the
  // palette re-entered after dispatching (no-op here today).
  void onOpenPalette;

  // Empty query shows only the action entries. Navigation entries are
  // searchOnly: they surface as the user types, ranked labeled sessions >
  // repos > plans > unlabeled sessions, with sessions ordered by recency.
  return useMemo<PaletteCommand[]>(() => {
    const out: PaletteCommand[] = [];
    out.push({
      id: "view.monitor",
      label: "Open overview",
      icon: "activity",
      group: "view",
      run: () => openTab({ kind: "monitor" }, "top"),
    });
    out.push({
      id: "view.metrics",
      label: "Open metrics",
      icon: "target",
      group: "view",
      run: () => openTab({ kind: "metrics" }, "top"),
    });
    out.push({
      id: "view.secrets",
      label: "Open secrets manager",
      icon: "settings",
      group: "view",
      run: () => openTab({ kind: "secrets" }, "top"),
    });
    if (!isMobile) {
      out.push({
        id: "sidebar.toggle-pin",
        label: "Toggle sidebar pin",
        icon: "panel-left",
        group: "view",
        run: () => useDisplay.getState().toggleSidebar(),
      });
      for (const mode of DISPLAY_MODES) {
        out.push({
          id: `display.${mode}`,
          label: `Display mode · ${DISPLAY_MODE_LABELS[mode]}`,
          icon: mode === "terminal" ? "terminal" : mode === "timeline" ? "list" : "layers",
          group: "view",
          run: () => useDisplay.getState().setMode(mode),
        });
      }
    }
    for (const metaRepo of metaRepos) {
      out.push({
        id: `meta-repo.${metaRepo.id}`,
        label: `Jump to meta-repository · ${metaRepo.name}`,
        icon: "layers",
        group: "repo",
        searchOnly: true,
        rank: 25,
        run: () =>
          appCommands.revealMetaRepo({ metaRepoId: metaRepo.id }),
      });
      out.push({
        id: `meta-repo.${metaRepo.id}.new-session`,
        label: `New collection session · ${metaRepo.name}`,
        icon: isMobile ? "list" : "terminal",
        group: "repo",
        searchOnly: true,
        rank: 20,
        run: () =>
          appCommands.newMetaRepoSession({ metaRepoId: metaRepo.id }),
      });
    }
    for (const r of repos) {
      out.push({
        id: `repo.${r.name}`,
        label: `Jump to repo · ${r.name}`,
        icon: "git-branch",
        group: "repo",
        searchOnly: true,
        rank: 20,
        run: () => appCommands.revealRepo({ repo: r.name }),
      });
      out.push({
        id: `repo.${r.name}.plans`,
        label: `Open plans · ${r.name}`,
        icon: "list",
        group: "repo",
        searchOnly: true,
        rank: 15,
        run: () => appCommands.openPlan({ repo: r.name }),
      });
    }
    const planTitles = new Map(plans.map((plan) => [plan.id, plan.title]));
    for (const plan of plans) {
      out.push(planCommand(plan, planTitles));
    }
    const liveSessions = sessions
      .filter((s) => s.state === "live")
      .sort((a, b) =>
        (b.last_event_at ?? b.created_at).localeCompare(
          a.last_event_at ?? a.created_at,
        ),
      );
    for (const s of liveSessions) {
      const labeled = Boolean(s.label && s.label.trim().length > 0);
      const label = labeled && s.label ? s.label : s.id.slice(0, 8);
      out.push({
        id: `session.${s.id}`,
        label: `Open session · ${s.meta_repo?.name ?? s.repo} / ${label}`,
        icon: "terminal",
        group: "session",
        searchOnly: true,
        rank: labeled ? 30 : 5,
        run: () => openSession(s.id),
      });
    }
    return out;
  }, [isMobile, metaRepos, openSession, openTab, plans, repos, sessions]);
}
