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
