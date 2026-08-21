// Global display preferences: how the work area projects a session
// (split terminal+timeline, terminal only, timeline only) and whether
// the sidebar is pinned open. Both persist to localStorage so the
// layout survives reloads.

import { create } from "zustand";

export type DisplayMode = "split" | "terminal" | "timeline";

export const DISPLAY_MODES: readonly DisplayMode[] = [
  "split",
  "terminal",
  "timeline",
];

export const DISPLAY_MODE_LABELS: Record<DisplayMode, string> = {
  split: "Split",
  terminal: "Terminal only",
  timeline: "Timeline only",
};

const MODE_STORAGE_KEY = "sulion.display.mode.v1";
// Pre-dates the store; kept so existing pin preferences carry over.
const PIN_STORAGE_KEY = "sulion.sidebar.pinned.v1";

function readMode(): DisplayMode {
  try {
    const raw = localStorage.getItem(MODE_STORAGE_KEY);
    if (raw === "split" || raw === "terminal" || raw === "timeline") return raw;
  } catch {
    /* storage disabled */
  }
  return "split";
}

function readPinned(): boolean {
  try {
    const raw = localStorage.getItem(PIN_STORAGE_KEY);
    return raw === null ? true : raw === "1";
  } catch {
    return true;
  }
}

interface DisplayStore {
  mode: DisplayMode;
  sidebarPinned: boolean;
  /** Peek overlay for the projection the current mode hides. Transient
   * (never persisted); forced closed whenever the mode returns to split. */
  peekOpen: boolean;
  setMode: (mode: DisplayMode) => void;
  /** split → terminal → timeline → split. */
  cycleMode: () => void;
  togglePeek: () => void;
  closePeek: () => void;
  toggleSidebar: () => void;
  setSidebarPinned: (pinned: boolean) => void;
}

export const useDisplay = create<DisplayStore>((set) => ({
  mode: typeof localStorage === "undefined" ? "split" : readMode(),
  sidebarPinned: typeof localStorage === "undefined" ? true : readPinned(),
  peekOpen: false,

  setMode(mode) {
    try {
      localStorage.setItem(MODE_STORAGE_KEY, mode);
    } catch {
      /* ignore */
    }
    set((state) => ({
      mode,
      peekOpen: mode === "split" ? false : state.peekOpen,
    }));
  },

  cycleMode() {
    set((state) => {
      const idx = DISPLAY_MODES.indexOf(state.mode);
      const mode = DISPLAY_MODES[(idx + 1) % DISPLAY_MODES.length]!;
      try {
        localStorage.setItem(MODE_STORAGE_KEY, mode);
      } catch {
        /* ignore */
      }
      return { mode, peekOpen: mode === "split" ? false : state.peekOpen };
    });
  },

  togglePeek() {
    set((state) =>
      state.mode === "split" ? state : { peekOpen: !state.peekOpen },
    );
  },

  closePeek() {
    set({ peekOpen: false });
  },

  toggleSidebar() {
    set((state) => {
      const sidebarPinned = !state.sidebarPinned;
      try {
        localStorage.setItem(PIN_STORAGE_KEY, sidebarPinned ? "1" : "0");
      } catch {
        /* ignore */
      }
      return { sidebarPinned };
    });
  },

  setSidebarPinned(sidebarPinned) {
    try {
      localStorage.setItem(PIN_STORAGE_KEY, sidebarPinned ? "1" : "0");
    } catch {
      /* ignore */
    }
    set({ sidebarPinned });
  },
}));

export function resetDisplayStore() {
  useDisplay.setState({
    mode: typeof localStorage === "undefined" ? "split" : readMode(),
    sidebarPinned: typeof localStorage === "undefined" ? true : readPinned(),
    peekOpen: false,
  });
}
