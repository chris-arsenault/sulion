// Timeline filter chips. Flat, obvious semantics:
//
//   - Speaker/tool chips: HIDE the named category when clicked. Default
//     is "nothing hidden, everything visible". Click Edit → Edit tool
//     rows disappear from the timeline. Click again → they come back.
//     There is no "inclusive" mode.
//
//   - errorsOnly / filePath: genuine include-only filters. "Errors only"
//     means drop turns that don't have any error. Empty filePath means
//     no constraint.
//
//   - showThinking / showBookkeeping / showSidechain: same "show" toggles
//     from before — clearly named for their semantics.
//
// Rendering pipeline (applied in TimelinePane):
//   events → prefilter (bookkeeping/sidechain) → group → drop-by-include
//   (errorsOnly, filePath) → Virtuoso of TurnRows
// Inside each turn's detail: events from hidden speakers are skipped;
// tool pairs with hidden tool names are skipped. Turn row in the list
// is NEVER dropped by hidden-speakers or hidden-tools — so if you
// select "Edit", you still see the containing turn in the list with
// its non-Edit content.

import { useEffect, useState } from "react";

import type { TimelineEvent } from "../../api/types";
import type { ToolPair, Turn } from "./grouping";
import {
  hasToolError,
  isToolResultUser,
  payloadOf,
  toolUsesIn,
} from "./types";

export type SpeakerFacet = "user" | "assistant" | "tool_result";

export interface TimelineFilters {
  /** Speakers to HIDE from the detail view. Empty = nothing hidden. */
  hiddenSpeakers: Set<SpeakerFacet>;
  /** Tool names to HIDE from the detail view (tool pair rows with
   * these names are skipped). Empty = nothing hidden. */
  hiddenTools: Set<string>;
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
  hiddenTools: new Set(),
  errorsOnly: false,
  showThinking: true,
  showBookkeeping: false,
  showSidechain: false,
  filePath: "",
};

// v2 because v1's filter state used inverted semantics. Old values in
// localStorage shouldn't be applied verbatim — let it fall back to
// defaults on first load after upgrade.
const STORAGE_KEY = "shuttlecraft.timeline.filters.v2";

// ─── persistence ──────────────────────────────────────────────────────

interface SerializedFilters {
  hiddenSpeakers: SpeakerFacet[];
  hiddenTools: string[];
  errorsOnly: boolean;
  showThinking: boolean;
  showBookkeeping: boolean;
  showSidechain: boolean;
  filePath: string;
}

function serialize(f: TimelineFilters): SerializedFilters {
  return {
    hiddenSpeakers: Array.from(f.hiddenSpeakers),
    hiddenTools: Array.from(f.hiddenTools),
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
    hiddenTools: new Set(),
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
  if (Array.isArray(r.hiddenTools)) {
    for (const t of r.hiddenTools) {
      if (typeof t === "string") out.hiddenTools.add(t);
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
  toggleTool: (t: string) => void;
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

  const toggleTool = (t: string) =>
    setFilters((prev) => {
      const next = new Set(prev.hiddenTools);
      if (next.has(t)) next.delete(t);
      else next.add(t);
      return { ...prev, hiddenTools: next };
    });

  return {
    filters,
    toggleSpeaker,
    toggleTool,
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
    hiddenTools: new Set(),
    errorsOnly: DEFAULT_FILTERS.errorsOnly,
    showThinking: DEFAULT_FILTERS.showThinking,
    showBookkeeping: DEFAULT_FILTERS.showBookkeeping,
    showSidechain: DEFAULT_FILTERS.showSidechain,
    filePath: DEFAULT_FILTERS.filePath,
  };
}

// ─── predicates ───────────────────────────────────────────────────────

function speakerOf(ev: TimelineEvent): SpeakerFacet | null {
  if (ev.kind === "assistant") return "assistant";
  if (ev.kind === "user") return isToolResultUser(ev) ? "tool_result" : "user";
  return null;
}

function eventMatchesFilePath(ev: TimelineEvent, needle: string): boolean {
  if (!needle) return true;
  const lower = needle.toLowerCase();
  for (const use of toolUsesIn(ev)) {
    const input = (use.input ?? {}) as Record<string, unknown>;
    for (const key of ["file_path", "path", "pattern", "command"]) {
      const v = input[key];
      if (typeof v === "string" && v.toLowerCase().includes(lower)) return true;
    }
  }
  const pjson = JSON.stringify(payloadOf(ev)).toLowerCase();
  return pjson.includes(lower);
}

/** Turn-level include-only filtering. Drops turns that fail the
 * errorsOnly or filePath filters. Hidden speakers/tools do NOT drop
 * turns — they only affect what's rendered inside the turn detail. */
export function turnPassesIncludeFilters(
  turn: Turn,
  f: TimelineFilters,
): boolean {
  if (f.errorsOnly) {
    if (!turn.hasErrors && !turn.events.some((e) => hasToolError(e))) {
      return false;
    }
  }
  if (f.filePath) {
    if (!turn.events.some((e) => eventMatchesFilePath(e, f.filePath))) {
      return false;
    }
  }
  return true;
}

/** True when any include filter is active. When false, turn list passes
 * through unfiltered at the turn level. */
export function hasActiveIncludeFilters(f: TimelineFilters): boolean {
  return f.errorsOnly || f.filePath.length > 0;
}

/** True when an individual event should be rendered. Hidden speakers
 * cause their events to be skipped in the detail view. */
export function eventIsVisible(
  ev: TimelineEvent,
  f: TimelineFilters,
): boolean {
  const sp = speakerOf(ev);
  if (sp != null && f.hiddenSpeakers.has(sp)) return false;
  return true;
}

/** True when a tool pair should be rendered. Hidden tool names cause
 * their pair rows to be skipped. */
export function toolPairIsVisible(
  pair: ToolPair,
  f: TimelineFilters,
): boolean {
  return !f.hiddenTools.has(pair.name);
}

/** Canonical tool names — matches the ingester's canonical map in
 * backend/src/canonical.rs. Chips + filter state compare against these
 * lowercase/underscored forms, not the raw agent-emitted names. */
export const KNOWN_TOOLS = [
  "edit",
  "write",
  "multi_edit",
  "bash",
  "read",
  "grep",
  "glob",
  "task",
  "todo_write",
  "web_fetch",
  "web_search",
] as const;
