import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { Icon } from "../icons";
import { Rail } from "./Rail";
import { Sidebar } from "./Sidebar";
import { WorkArea } from "./WorkArea";
import { useMediaQuery } from "../hooks/useMediaQuery";
import { appCommands, useAppCommand } from "../state/AppCommands";
import { useTabs } from "../state/TabStore";
import { useSessions } from "../state/SessionStore";
import { CommandPalette, type PaletteCommand } from "./ui";
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

  useAppCommand("open-file", ({ repo, path }) => {
    openTabRef.current({ kind: "file", repo, path });
  });
  useAppCommand("open-diff", ({ repo, path }) => {
    openTabRef.current({ kind: "diff", repo, path });
  });
  useAppCommand("close-drawer", () => {
    setDrawerOpen(false);
  });

  // ⌘K / Ctrl-K opens the command palette. Esc is handled by the Overlay.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPaletteOpen(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const commands = usePaletteCommands({
    setPinned,
    onOpenPalette: () => setPaletteOpen(true),
  });

  if (isMobile) {
    return (
      <div className="layout layout--mobile">
        <MobileTopBar onOpenDrawer={() => setDrawerOpen(true)} />
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
          <WorkArea />
        </main>
        <CommandPalette
          open={paletteOpen}
          onClose={() => setPaletteOpen(false)}
          commands={commands}
        />
      </div>
    );
  }

  return (
    <div
      className={`layout ${pinned ? "layout--pinned" : "layout--collapsed"}`}
      style={{ "--sulion-sidebar-width": `${sidebarWidth}px` } as React.CSSProperties}
    >
      <Rail
        pinned={pinned}
        onTogglePinned={() => setPinned((v) => !v)}
        onOpenPalette={() => setPaletteOpen(true)}
      />
      {pinned ? (
        <>
          <aside className="layout__sidebar" aria-label="Workspace">
            <Sidebar />
          </aside>
          <SidebarResizer
            width={sidebarWidth}
            onChange={setSidebarWidth}
          />
        </>
      ) : null}
      <main className="layout__main">
        <WorkArea />
      </main>
      <CommandPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        commands={commands}
      />
    </div>
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

  const start = () => {
    dragging.current = true;
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowLeft") onChange(Math.max(MIN_WIDTH, width - 12));
    if (e.key === "ArrowRight") onChange(Math.min(MAX_WIDTH, width + 12));
  };

  return (
    <div
      className="layout__resizer"
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize sidebar"
      aria-valuenow={width}
      aria-valuemin={MIN_WIDTH}
      aria-valuemax={MAX_WIDTH}
      // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- resize handle needs keyboard arrow support
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

  return useMemo<PaletteCommand[]>(() => {
    const out: PaletteCommand[] = [];
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
        run: () => appCommands.revealRepo({ repo: r.name }),
      });
    }
    for (const s of sessions) {
      const label = s.label && s.label.length > 0 ? s.label : s.id.slice(0, 8);
      out.push({
        id: `session.${s.id}`,
        label: `Open session · ${s.repo} / ${label}`,
        icon: "terminal",
        group: "session",
        run: () => openTerminalFor(s.id),
      });
    }
    return out;
  }, [repos, sessions, setPinned, openTerminalFor]);
}
