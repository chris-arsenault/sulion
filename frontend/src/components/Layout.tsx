import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { Icon } from "../icons";
import { Rail } from "./Rail";
import { Sidebar } from "./Sidebar";
import { WorkArea } from "./WorkArea";
import { FuturePromptsModal } from "./FuturePromptsModal";
import { MonitorPane } from "./MonitorPane";
import { useMediaQuery } from "../hooks/useMediaQuery";
import { appCommands, useAppCommand } from "../state/AppCommands";
import { useTabs } from "../state/TabStore";
import { useSessions } from "../state/SessionStore";
import { CommandPalette, Overlay, type PaletteCommand } from "./ui";
import "./Layout.css";

const PIN_STORAGE_KEY = "sulion.sidebar.pinned.v1";
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
  const isMobile = useMediaQuery("(max-width: 767px)");
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [monitorOverlayOpen, setMonitorOverlayOpen] = useState(false);
  const [futurePromptsSessionId, setFuturePromptsSessionId] = useState<string | null>(null);

  const [pinned, setPinned] = useState<boolean>(() => {
    if (typeof localStorage === "undefined") return true;
    const v = localStorage.getItem(PIN_STORAGE_KEY);
    return v === null ? true : v === "1";
  });
  useEffect(() => {
    try {
      localStorage.setItem(PIN_STORAGE_KEY, pinned ? "1" : "0");
    } catch {
      /* ignore */
    }
  }, [pinned]);

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

  useAppCommand("open-file", ({ repo, path, workspaceId }) => {
    openTabRef.current({ kind: "file", repo, path, workspaceId });
    setDrawerOpen(false);
  });
  useAppCommand("open-diff", ({ repo, path, workspaceId }) => {
    openTabRef.current({ kind: "diff", repo, path, workspaceId });
    setDrawerOpen(false);
  });
  useAppCommand("open-future-prompts", ({ sessionId }) => {
    setFuturePromptsSessionId(sessionId);
    setDrawerOpen(false);
  });
  useAppCommand("close-drawer", () => {
    setDrawerOpen(false);
  });

  // ⌘K / Ctrl-K opens the command palette. ⌘M / Ctrl-M toggles the monitor
  // overlay (TerminalPane excludes the chord from xterm so it never reaches
  // the PTY). Esc is handled by the Overlay.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPaletteOpen(true);
      }
      if ((e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === "m") {
        e.preventDefault();
        setMonitorOverlayOpen((open) => !open);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

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
  const openDrawer = useCallback(() => setDrawerOpen(true), []);
  const closeDrawerLocal = useCallback(() => setDrawerOpen(false), []);
  const togglePinned = useCallback(() => setPinned((v) => !v), []);

  const commands = usePaletteCommands({
    setPinned,
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
        <MonitorOverlay open={monitorOverlayOpen} onClose={closeMonitorOverlay} />
        <FuturePromptsModal
          open={futurePromptsSessionId !== null}
          sessionId={futurePromptsSessionId}
          onClose={closeFuturePrompts}
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
      <Rail
        pinned={pinned}
        onTogglePinned={togglePinned}
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
      <MonitorOverlay open={monitorOverlayOpen} onClose={closeMonitorOverlay} />
      <FuturePromptsModal
        open={futurePromptsSessionId !== null}
        sessionId={futurePromptsSessionId}
        onClose={closeFuturePrompts}
      />
    </div>
  );
}

/** The monitor as a transient modal over the workspace (⌘M / Ctrl-M
 * toggles it). Same component as the monitor tab; mounting on open is
 * what starts its polling, so a closed overlay costs nothing. */
function MonitorOverlay({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
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
      <MonitorPane />
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

function usePaletteCommands({
  setPinned,
  onOpenPalette,
}: {
  setPinned: (fn: (v: boolean) => boolean) => void;
  onOpenPalette: () => void;
}): PaletteCommand[] {
  const repos = useSessions((s) => s.repos);
  const sessions = useSessions((s) => s.sessions);
  const plans = useSessions((s) => s.plans);
  const selectSession = useSessions((s) => s.selectSession);
  const openTab = useTabs((s) => s.openTab);
  // onOpenPalette retained in signature for future surfaces that want the
  // palette re-entered after dispatching (no-op here today).
  void onOpenPalette;

  const openTerminalFor = useCallback(
    (id: string) => {
      selectSession(id);
      openTab({ kind: "terminal", sessionId: id }, "top");
      openTab({ kind: "timeline", sessionId: id }, "bottom");
    },
    [openTab, selectSession],
  );

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
      id: "view.secrets",
      label: "Open secrets manager",
      icon: "settings",
      group: "view",
      run: () => openTab({ kind: "secrets" }, "top"),
    });
    out.push({
      id: "sidebar.toggle-pin",
      label: "Toggle sidebar pin",
      icon: "panel-left",
      group: "view",
      run: () => setPinned((v) => !v),
    });
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
        run: () => openTab({ kind: "plan", repo: r.name }),
      });
    }
    for (const plan of plans) {
      out.push({
        id: `plan.${plan.id}`,
        label: `Open plan · ${plan.repo_name} / ${plan.title}`,
        icon: "list",
        group: "plan",
        searchOnly: true,
        rank: 10,
        run: () =>
          openTab({
            kind: "plan",
            repo: plan.repo_name,
            planId: plan.id,
          }),
      });
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
        label: `Open session · ${s.repo} / ${label}`,
        icon: "terminal",
        group: "session",
        searchOnly: true,
        rank: labeled ? 30 : 5,
        run: () => openTerminalFor(s.id),
      });
    }
    return out;
  }, [openTab, plans, repos, sessions, setPinned, openTerminalFor]);
}
