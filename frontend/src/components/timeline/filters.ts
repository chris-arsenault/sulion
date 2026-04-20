// Timeline filter chips. Flat, obvious semantics:
//
//   - Speaker/operation chips: HIDE the named category when clicked.
//     Default is "nothing hidden, everything visible". Click
//     "create content" → those tool rows disappear from the timeline.
//     Click again → they come back. There is no "inclusive" mode.
//
//   - errorsOnly / filePath: genuine include-only filters. "Errors only"
//     means drop turns that don't have any error. Empty filePath means
//     no constraint.
//
//   - showThinking / showBookkeeping / showSidechain: same "show" toggles
//     from before — clearly named for their semantics.
//
// The backend timeline endpoint owns grouping, pairing, preview
// generation, and turn-level filtering. This module now just owns the
// UI filter state and persistence layer.

import { useEffect, useState } from "react";

import type { OperationCategory, SpeakerFacet } from "../../api/types";

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
}

export const DEFAULT_FILTERS: TimelineFilters = {
  hiddenSpeakers: new Set(),
  hiddenOperationCategories: new Set(),
  errorsOnly: false,
  showThinking: true,
  showBookkeeping: false,
  showSidechain: false,
  filePath: "",
};

// v3 because v2 stored raw tool-name hide state. The app now stores
// app-facing operation categories instead, so old values should not be
// replayed into the new semantics.
const STORAGE_KEY = "sulion.timeline.filters.v3";

// ─── persistence ──────────────────────────────────────────────────────

interface SerializedFilters {
  hiddenSpeakers: SpeakerFacet[];
  hiddenOperationCategories: OperationCategory[];
  errorsOnly: boolean;
  showThinking: boolean;
  showBookkeeping: boolean;
  showSidechain: boolean;
  filePath: string;
}

function serialize(f: TimelineFilters): SerializedFilters {
  return {
    hiddenSpeakers: Array.from(f.hiddenSpeakers),
    hiddenOperationCategories: Array.from(f.hiddenOperationCategories),
    errorsOnly: f.errorsOnly,
    showThinking: f.showThinking,
    showBookkeeping: f.showBookkeeping,
    showSidechain: f.showSidechain,
    filePath: f.filePath,
  };
}

function deserialize(raw: unknown): TimelineFilters {
  const out: TimelineFilters = {
    hiddenSpeakers: new Set(),
    hiddenOperationCategories: new Set(),
    errorsOnly: DEFAULT_FILTERS.errorsOnly,
    showThinking: DEFAULT_FILTERS.showThinking,
    showBookkeeping: DEFAULT_FILTERS.showBookkeeping,
    showSidechain: DEFAULT_FILTERS.showSidechain,
    filePath: DEFAULT_FILTERS.filePath,
  };
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
  return out;
}

// ─── hook ─────────────────────────────────────────────────────────────

export function useTimelineFilters(): {
  filters: TimelineFilters;
  toggleSpeaker: (s: SpeakerFacet) => void;
  toggleOperationCategory: (category: OperationCategory) => void;
  setErrorsOnly: (v: boolean) => void;
  setShowThinking: (v: boolean) => void;
  setShowBookkeeping: (v: boolean) => void;
  setShowSidechain: (v: boolean) => void;
  setFilePath: (v: string) => void;
  reset: () => void;
} {
  const [filters, setFilters] = useState<TimelineFilters>(() => {
    if (typeof window === "undefined") return cloneDefault();
    try {
      const raw = window.localStorage.getItem(STORAGE_KEY);
      if (raw) return deserialize(JSON.parse(raw));
    } catch {
      // fall through
    }
    return cloneDefault();
  });

  useEffect(() => {
    if (typeof window === "undefined") return;
    try {
      window.localStorage.setItem(STORAGE_KEY, JSON.stringify(serialize(filters)));
    } catch {
      // storage full or disabled
    }
  }, [filters]);

  const toggleSpeaker = (s: SpeakerFacet) =>
    setFilters((prev) => {
      const next = new Set(prev.hiddenSpeakers);
      if (next.has(s)) next.delete(s);
      else next.add(s);
      return { ...prev, hiddenSpeakers: next };
    });

  const toggleOperationCategory = (category: OperationCategory) =>
    setFilters((prev) => {
      const next = new Set(prev.hiddenOperationCategories);
      if (next.has(category)) next.delete(category);
      else next.add(category);
      return { ...prev, hiddenOperationCategories: next };
    });

  return {
    filters,
    toggleSpeaker,
    toggleOperationCategory,
    setErrorsOnly: (v) => setFilters((p) => ({ ...p, errorsOnly: v })),
    setShowThinking: (v) => setFilters((p) => ({ ...p, showThinking: v })),
    setShowBookkeeping: (v) => setFilters((p) => ({ ...p, showBookkeeping: v })),
    setShowSidechain: (v) => setFilters((p) => ({ ...p, showSidechain: v })),
    setFilePath: (v) => setFilters((p) => ({ ...p, filePath: v })),
    reset: () => setFilters(cloneDefault()),
  };
}

function cloneDefault(): TimelineFilters {
  return {
    hiddenSpeakers: new Set(),
    hiddenOperationCategories: new Set(),
    errorsOnly: DEFAULT_FILTERS.errorsOnly,
    showThinking: DEFAULT_FILTERS.showThinking,
    showBookkeeping: DEFAULT_FILTERS.showBookkeeping,
    showSidechain: DEFAULT_FILTERS.showSidechain,
    filePath: DEFAULT_FILTERS.filePath,
  };
}

function isOperationCategory(value: unknown): value is OperationCategory {
  return (
    value === "create_content" ||
    value === "inspect" ||
    value === "utility" ||
    value === "research" ||
    value === "delegate" ||
    value === "workflow" ||
    value === "other"
  );
}

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
