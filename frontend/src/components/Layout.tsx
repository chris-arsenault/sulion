import { useEffect, useRef, useState } from "react";

import { Sidebar } from "./Sidebar";
import { WorkArea } from "./WorkArea";
import { useMediaQuery } from "../hooks/useMediaQuery";
import { useAppCommand } from "../state/AppCommands";
import { useTabs } from "../state/TabStore";
import "./Layout.css";

const PIN_STORAGE_KEY = "shuttlecraft.sidebar.pinned.v1";

/** Root layout: sidebar + WorkArea. On mobile the sidebar becomes a
 * drawer. The split / tab system lives inside WorkArea. */
export function Layout() {
  const openTab = useTabs((store) => store.openTab);
  const isMobile = useMediaQuery("(max-width: 767px)");
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [pinned, setPinned] = useState<boolean>(() => {
    if (typeof localStorage === "undefined") return true;
    const v = localStorage.getItem(PIN_STORAGE_KEY);
    return v === null ? true : v === "1";
  });
  const [hovering, setHovering] = useState(false);
  useEffect(() => {
    try {
      localStorage.setItem(PIN_STORAGE_KEY, pinned ? "1" : "0");
    } catch {
      /* ignore */
    }
  }, [pinned]);

  // Stable ref to openTab so global-event listeners don't re-bind on
  // every tab state change — re-binding caused the "click file does
  // nothing" bug: every re-registration fired pending events against a
  // stale closure and the tab was immediately re-activated elsewhere.
  const openTabRef = useRef(openTab);
  openTabRef.current = openTab;

  // Global Cmd/Ctrl-K opens the search tab.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        openTabRef.current({ kind: "search" }, "top");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useAppCommand("open-file", ({ repo, path }) => {
    openTabRef.current({ kind: "file", repo, path });
  });

  useAppCommand("open-diff", ({ repo, path }) => {
    openTabRef.current({ kind: "diff", repo, path });
  });

  useAppCommand("close-drawer", () => {
    setDrawerOpen(false);
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
      </div>
    );
  }

  // Unpinned sidebar: show a narrow rail; hover expands the full nav
  // without moving the work area. Pin toggles persist across refresh.
  const expanded = pinned || hovering;

  return (
    <div
      className={`layout ${pinned ? "layout--pinned" : "layout--rail"}`}
    >
      <aside
        className={
          expanded
            ? "layout__sidebar layout__sidebar--expanded"
            : "layout__sidebar"
        }
        onMouseEnter={() => !pinned && setHovering(true)}
        onMouseLeave={() => !pinned && setHovering(false)}
      >
        {expanded ? (
          <div className="layout__sidebar-body">
            <button
              type="button"
              className="layout__pin"
              onClick={() => setPinned((v) => !v)}
              title={pinned ? "Unpin sidebar (auto-collapse)" : "Pin sidebar open"}
              aria-label={pinned ? "Unpin sidebar" : "Pin sidebar"}
            >
              {pinned ? "📌" : "📍"}
            </button>
            <Sidebar />
          </div>
        ) : (
          <div className="layout__rail" aria-hidden>
            <span className="layout__rail-logo">sc</span>
          </div>
        )}
      </aside>
      <main className="layout__main">
        <WorkArea />
      </main>
    </div>
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
        ☰
      </button>
      <span className="mobile-topbar__title">shuttlecraft</span>
    </div>
  );
}
