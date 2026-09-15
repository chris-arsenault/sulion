import type { DisplayMode } from "./DisplayStore";

export const MOBILE_LAYOUT_QUERY = "(max-width: 767px)";

/** Mobile has one supported projection. Keep the stored desktop preference
 * unchanged so resizing back to desktop restores the user's chosen mode. */
export function effectiveDisplayMode(
  storedMode: DisplayMode,
  isMobile: boolean,
): DisplayMode {
  return isMobile ? "timeline" : storedMode;
}

export type PromptInjectionTarget = "terminal" | "timeline";

/** Where injected prompt text (library snippets, queued future prompts)
 * lands. The terminal pane stays mounted but hidden in timeline-only
 * mode, so pasting there would swallow the text; the timeline prompt
 * box is the input the user can see. Split and terminal-only keep the
 * terminal. Takes the effective mode, so mobile resolves to timeline. */
export function promptInjectionTarget(
  effectiveMode: DisplayMode,
): PromptInjectionTarget {
  return effectiveMode === "timeline" ? "timeline" : "terminal";
}
