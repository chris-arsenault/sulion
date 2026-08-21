import { useEffect, useState, type Dispatch, type SetStateAction } from "react";

const TERMINAL_FONT_SIZE_KEY = "sulion.terminal.font-size.v1";
const TIMELINE_FONT_SCALE_KEY = "sulion.timeline.font-scale.v1";

export const TERMINAL_FONT_SIZE_DEFAULT = 13;
export const TERMINAL_FONT_SIZE_MIN = 11;
export const TERMINAL_FONT_SIZE_MAX = 20;

export const TIMELINE_FONT_SCALE_DEFAULT = 1;
export const TIMELINE_FONT_SCALE_MIN = 0.9;
export const TIMELINE_FONT_SCALE_MAX = 1.5;
export const TIMELINE_FONT_SCALE_STEP = 0.1;

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

export function useTimelineFontScale() {
  return useStoredNumber(
    TIMELINE_FONT_SCALE_KEY,
    TIMELINE_FONT_SCALE_DEFAULT,
    TIMELINE_FONT_SCALE_MIN,
    TIMELINE_FONT_SCALE_MAX,
  );
}

export function clampTerminalFontSize(value: number): number {
  return Math.max(TERMINAL_FONT_SIZE_MIN, Math.min(TERMINAL_FONT_SIZE_MAX, value));
}

// ─── turn navigation mode ─────────────────────────────────────────────

/** How the timeline presents its turn navigation: the full list pane, a
 * compact per-turn hover grid under the header, or nothing at all. */
export type TurnNavMode = "list" | "grid" | "hidden";

export const TURN_NAV_MODES: readonly TurnNavMode[] = ["list", "grid", "hidden"];

const TURN_NAV_MODE_KEY = "sulion.timeline.turn-nav.v1";

export function useTurnNavMode(): [TurnNavMode, Dispatch<SetStateAction<TurnNavMode>>] {
  const [value, setValue] = useState<TurnNavMode>(() => {
    if (typeof window === "undefined") return "list";
    const raw = window.localStorage.getItem(TURN_NAV_MODE_KEY);
    return raw === "grid" || raw === "hidden" ? raw : "list";
  });

  useEffect(() => {
    if (typeof window === "undefined") return;
    window.localStorage.setItem(TURN_NAV_MODE_KEY, value);
  }, [value]);

  return [value, setValue];
}

export function clampTimelineFontScale(value: number): number {
  const clamped = Math.max(TIMELINE_FONT_SCALE_MIN, Math.min(TIMELINE_FONT_SCALE_MAX, value));
  return Math.round(clamped * 10) / 10;
}
