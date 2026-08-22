import { useEffect, useState, type Dispatch, type SetStateAction } from "react";

import { useTimelineControlsStore } from "./TimelineControlsStore";

export {
  TIMELINE_FONT_SCALE_DEFAULT,
  TIMELINE_FONT_SCALE_MAX,
  TIMELINE_FONT_SCALE_MIN,
  TIMELINE_FONT_SCALE_STEP,
  TURN_NAV_MODES,
  clampTimelineFontScale,
  type TurnNavMode,
} from "./TimelineControlsStore";
import type { TurnNavMode } from "./TimelineControlsStore";

const TERMINAL_FONT_SIZE_KEY = "sulion.terminal.font-size.v1";

export const TERMINAL_FONT_SIZE_DEFAULT = 13;
export const TERMINAL_FONT_SIZE_MIN = 11;
export const TERMINAL_FONT_SIZE_MAX = 20;

function readStoredNumber(key: string, fallback: number, min: number, max: number): number {
  if (typeof window === "undefined") return fallback;
  const raw = window.localStorage.getItem(key);
  const value = raw ? Number(raw) : NaN;
  if (!Number.isFinite(value) || value < min || value > max) return fallback;
  return value;
}

function useStoredNumber(
  key: string,
  fallback: number,
  min: number,
  max: number,
): [number, Dispatch<SetStateAction<number>>] {
  const [value, setValue] = useState<number>(() => readStoredNumber(key, fallback, min, max));

  useEffect(() => {
    if (typeof window === "undefined") return;
    window.localStorage.setItem(key, String(value));
  }, [key, value]);

  return [value, setValue];
}

export function useTerminalFontSize() {
  return useStoredNumber(
    TERMINAL_FONT_SIZE_KEY,
    TERMINAL_FONT_SIZE_DEFAULT,
    TERMINAL_FONT_SIZE_MIN,
    TERMINAL_FONT_SIZE_MAX,
  );
}

export function clampTerminalFontSize(value: number): number {
  return Math.max(TERMINAL_FONT_SIZE_MIN, Math.min(TERMINAL_FONT_SIZE_MAX, value));
}

// ─── timeline controls — backed by the singleton TimelineControlsStore ─
//
// These used to be per-`TimelinePane`-instance `useState`, seeded once
// from localStorage on mount. They're now thin wrappers over one shared
// store, so every mounted timeline reflects the same setting live.

export function useTurnNavMode(): [TurnNavMode, Dispatch<SetStateAction<TurnNavMode>>] {
  const mode = useTimelineControlsStore((s) => s.turnNavMode);
  const setMode = useTimelineControlsStore((s) => s.setTurnNavMode);
  return [mode, setMode];
}

export function useTimelineFontScale(): [number, Dispatch<SetStateAction<number>>] {
  const scale = useTimelineControlsStore((s) => s.timelineFontScale);
  const setScale = useTimelineControlsStore((s) => s.setTimelineFontScale);
  return [scale, setScale];
}
