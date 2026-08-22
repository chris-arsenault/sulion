// Singleton timeline controls: turn-navigation mode, text scale, and
// content filters. Previously each `TimelinePane` kept its own copy of
// this state (seeded once from localStorage on mount), so multiple
// mounted panes could disagree until remounted. This store makes the
// controls genuinely global — one settings surface, reflected live by
// every open timeline.

import { create } from "zustand";

import type { OperationCategory, SpeakerFacet } from "../api/types";

/** How the timeline presents its turn navigation: the full list pane, a
 * compact per-turn hover grid, or nothing at all. */
export type TurnNavMode = "list" | "grid" | "hidden";

export const TURN_NAV_MODES: readonly TurnNavMode[] = ["list", "grid", "hidden"];

export const TIMELINE_FONT_SCALE_DEFAULT = 1;
export const TIMELINE_FONT_SCALE_MIN = 0.9;
export const TIMELINE_FONT_SCALE_MAX = 1.5;
export const TIMELINE_FONT_SCALE_STEP = 0.1;

export function clampTimelineFontScale(value: number): number {
  const clamped = Math.max(TIMELINE_FONT_SCALE_MIN, Math.min(TIMELINE_FONT_SCALE_MAX, value));
  return Math.round(clamped * 10) / 10;
}

const TURN_NAV_MODE_KEY = "sulion.timeline.turn-nav.v1";
const TIMELINE_FONT_SCALE_KEY = "sulion.timeline.font-scale.v1";
// v3 because v2 stored raw tool-name hide state. The app now stores
// app-facing operation categories instead, so old values should not be
// replayed into the new semantics.
const FILTERS_STORAGE_KEY = "sulion.timeline.filters.v3";

export interface TimelineFilters {
  /** Speakers to HIDE from the detail view. Empty = nothing hidden. */
  hiddenSpeakers: Set<SpeakerFacet>;
  /** Operation categories to HIDE from the detail view (tool pair rows
   * with these categories are skipped). Empty = nothing hidden. */
  hiddenOperationCategories: Set<OperationCategory>;
  /** Include-only filter: when true, drop turns that have no errors. */
  errorsOnly: boolean;
  /** Render flag: false hides thinking content. */
  showThinking: boolean;
  /** Prefilter flag: false drops bookkeeping-kind events before grouping. */
  showBookkeeping: boolean;
  /** Prefilter flag: false drops sidechain events (subagent modal only). */
  showSidechain: boolean;
  /** Include-only filter: substring match against tool_use input paths.
   * Empty = no constraint. */
  filePath: string;
  /** When true, the timeline auto-selects the latest turn as new polls
   * arrive — tail-follow behaviour. Clicking a turn in the list turns
   * this off so the user's selection isn't clobbered. */
  followLatest: boolean;
}

export const DEFAULT_FILTERS: TimelineFilters = {
  hiddenSpeakers: new Set(),
  hiddenOperationCategories: new Set(),
  errorsOnly: false,
  showThinking: true,
  showBookkeeping: false,
  showSidechain: false,
  filePath: "",
  followLatest: false,
};

export const KNOWN_OPERATION_CATEGORIES = [
  "create_content",
  "inspect",
  "utility",
  "research",
  "delegate",
  "workflow",
  "other",
] as const;

export const OPERATION_CATEGORY_LABELS: Record<OperationCategory, string> = {
  create_content: "create content",
  inspect: "inspect",
  utility: "utility",
  research: "research",
  delegate: "delegate",
  workflow: "workflow",
  other: "other",
};

function isOperationCategory(value: unknown): value is OperationCategory {
  return (KNOWN_OPERATION_CATEGORIES as readonly string[]).includes(value as string);
}

function cloneDefaultFilters(): TimelineFilters {
  return {
    ...DEFAULT_FILTERS,
    hiddenSpeakers: new Set(),
    hiddenOperationCategories: new Set(),
  };
}

interface SerializedFilters {
  hiddenSpeakers: SpeakerFacet[];
  hiddenOperationCategories: OperationCategory[];
  errorsOnly: boolean;
  showThinking: boolean;
  showBookkeeping: boolean;
  showSidechain: boolean;
  filePath: string;
  followLatest: boolean;
}

function serializeFilters(f: TimelineFilters): SerializedFilters {
  return {
    hiddenSpeakers: Array.from(f.hiddenSpeakers),
    hiddenOperationCategories: Array.from(f.hiddenOperationCategories),
    errorsOnly: f.errorsOnly,
    showThinking: f.showThinking,
    showBookkeeping: f.showBookkeeping,
    showSidechain: f.showSidechain,
    filePath: f.filePath,
    followLatest: f.followLatest,
  };
}

function deserializeFilters(raw: unknown): TimelineFilters {
  const out = cloneDefaultFilters();
  if (!raw || typeof raw !== "object") return out;
  const r = raw as Partial<SerializedFilters>;
  if (Array.isArray(r.hiddenSpeakers)) {
    for (const s of r.hiddenSpeakers) {
      if (s === "user" || s === "assistant" || s === "tool_result") {
        out.hiddenSpeakers.add(s);
      }
    }
  }
  if (Array.isArray(r.hiddenOperationCategories)) {
    for (const category of r.hiddenOperationCategories) {
      if (isOperationCategory(category)) {
        out.hiddenOperationCategories.add(category);
      }
    }
  }
  if (typeof r.errorsOnly === "boolean") out.errorsOnly = r.errorsOnly;
  if (typeof r.showThinking === "boolean") out.showThinking = r.showThinking;
  if (typeof r.showBookkeeping === "boolean") out.showBookkeeping = r.showBookkeeping;
  if (typeof r.showSidechain === "boolean") out.showSidechain = r.showSidechain;
  if (typeof r.filePath === "string") out.filePath = r.filePath;
  if (typeof r.followLatest === "boolean") out.followLatest = r.followLatest;
  return out;
}

function readTurnNavMode(): TurnNavMode {
  try {
    const raw = localStorage.getItem(TURN_NAV_MODE_KEY);
    return raw === "grid" || raw === "hidden" ? raw : "list";
  } catch {
    return "list";
  }
}

function readTimelineFontScale(): number {
  try {
    const raw = localStorage.getItem(TIMELINE_FONT_SCALE_KEY);
    const value = raw ? Number(raw) : NaN;
    return Number.isFinite(value) ? clampTimelineFontScale(value) : TIMELINE_FONT_SCALE_DEFAULT;
  } catch {
    return TIMELINE_FONT_SCALE_DEFAULT;
  }
}

function readFilters(): TimelineFilters {
  try {
    const raw = localStorage.getItem(FILTERS_STORAGE_KEY);
    if (raw) return deserializeFilters(JSON.parse(raw));
  } catch {
    // fall through
  }
  return cloneDefaultFilters();
}

interface TimelineControlsStore {
  turnNavMode: TurnNavMode;
  timelineFontScale: number;
  filters: TimelineFilters;

  setTurnNavMode: (mode: TurnNavMode | ((prev: TurnNavMode) => TurnNavMode)) => void;
  cycleTurnNavMode: () => void;
  setTimelineFontScale: (value: number | ((prev: number) => number)) => void;

  toggleSpeaker: (s: SpeakerFacet) => void;
  toggleOperationCategory: (category: OperationCategory) => void;
  setErrorsOnly: (v: boolean) => void;
  setShowThinking: (v: boolean) => void;
  setShowBookkeeping: (v: boolean) => void;
  setShowSidechain: (v: boolean) => void;
  setFilePath: (v: string) => void;
  setFollowLatest: (v: boolean) => void;
  resetFilters: () => void;
}

function persistTurnNavMode(mode: TurnNavMode) {
  try {
    localStorage.setItem(TURN_NAV_MODE_KEY, mode);
  } catch {
    /* ignore */
  }
}

function persistTimelineFontScale(value: number) {
  try {
    localStorage.setItem(TIMELINE_FONT_SCALE_KEY, String(value));
  } catch {
    /* ignore */
  }
}

function persistFilters(filters: TimelineFilters) {
  try {
    localStorage.setItem(FILTERS_STORAGE_KEY, JSON.stringify(serializeFilters(filters)));
  } catch {
    /* storage full or disabled */
  }
}

export const useTimelineControlsStore = create<TimelineControlsStore>((set, get) => ({
  turnNavMode: typeof localStorage === "undefined" ? "list" : readTurnNavMode(),
  timelineFontScale:
    typeof localStorage === "undefined" ? TIMELINE_FONT_SCALE_DEFAULT : readTimelineFontScale(),
  filters: typeof localStorage === "undefined" ? cloneDefaultFilters() : readFilters(),

  setTurnNavMode(mode) {
    const next = typeof mode === "function" ? mode(get().turnNavMode) : mode;
    persistTurnNavMode(next);
    set({ turnNavMode: next });
  },

  cycleTurnNavMode() {
    const idx = TURN_NAV_MODES.indexOf(get().turnNavMode);
    const next = TURN_NAV_MODES[(idx + 1) % TURN_NAV_MODES.length]!;
    persistTurnNavMode(next);
    set({ turnNavMode: next });
  },

  setTimelineFontScale(value) {
    const next = typeof value === "function" ? value(get().timelineFontScale) : value;
    persistTimelineFontScale(next);
    set({ timelineFontScale: next });
  },

  toggleSpeaker(s) {
    set((state) => {
      const next = new Set(state.filters.hiddenSpeakers);
      if (next.has(s)) next.delete(s);
      else next.add(s);
      const filters = { ...state.filters, hiddenSpeakers: next };
      persistFilters(filters);
      return { filters };
    });
  },

  toggleOperationCategory(category) {
    set((state) => {
      const next = new Set(state.filters.hiddenOperationCategories);
      if (next.has(category)) next.delete(category);
      else next.add(category);
      const filters = { ...state.filters, hiddenOperationCategories: next };
      persistFilters(filters);
      return { filters };
    });
  },

  setErrorsOnly(v) {
    set((state) => {
      const filters = { ...state.filters, errorsOnly: v };
      persistFilters(filters);
      return { filters };
    });
  },

  setShowThinking(v) {
    set((state) => {
      const filters = { ...state.filters, showThinking: v };
      persistFilters(filters);
      return { filters };
    });
  },

  setShowBookkeeping(v) {
    set((state) => {
      const filters = { ...state.filters, showBookkeeping: v };
      persistFilters(filters);
      return { filters };
    });
  },

  setShowSidechain(v) {
    set((state) => {
      const filters = { ...state.filters, showSidechain: v };
      persistFilters(filters);
      return { filters };
    });
  },

  setFilePath(v) {
    set((state) => {
      const filters = { ...state.filters, filePath: v };
      persistFilters(filters);
      return { filters };
    });
  },

  setFollowLatest(v) {
    set((state) => {
      const filters = { ...state.filters, followLatest: v };
      persistFilters(filters);
      return { filters };
    });
  },

  resetFilters() {
    const filters = cloneDefaultFilters();
    persistFilters(filters);
    set({ filters });
  },
}));

export function resetTimelineControlsStore() {
  useTimelineControlsStore.setState({
    turnNavMode: typeof localStorage === "undefined" ? "list" : readTurnNavMode(),
    timelineFontScale:
      typeof localStorage === "undefined" ? TIMELINE_FONT_SCALE_DEFAULT : readTimelineFontScale(),
    filters: typeof localStorage === "undefined" ? cloneDefaultFilters() : readFilters(),
  });
}
