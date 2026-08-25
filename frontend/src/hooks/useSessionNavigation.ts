import { useCallback } from "react";

import { useMediaQuery } from "./useMediaQuery";
import { MOBILE_LAYOUT_QUERY } from "../state/displayPolicy";
import { useSessions } from "../state/SessionStore";
import { useTabs } from "../state/TabStore";

export interface SessionNavigationOptions {
  focusTurnId?: number;
}

export function useIsMobileLayout(): boolean {
  return useMediaQuery(MOBILE_LAYOUT_QUERY);
}

/** Select a session and open the views supported by the current viewport.
 * Desktop keeps its paired terminal/timeline tabs; mobile opens only the
 * timeline and can optionally focus a specific turn. */
export function useSessionNavigation() {
  const isMobile = useIsMobileLayout();
  const selectSession = useSessions((store) => store.selectSession);
  const openTab = useTabs((store) => store.openTab);

  return useCallback(
    (sessionId: string, options: SessionNavigationOptions = {}) => {
      selectSession(sessionId);
      if (!isMobile) {
        openTab({ kind: "terminal", sessionId }, "top");
      }
      openTab(
        {
          kind: "timeline",
          sessionId,
          focusTurnId: options.focusTurnId,
          focusKey:
            options.focusTurnId == null ? undefined : crypto.randomUUID(),
        },
        "bottom",
      );
    },
    [isMobile, openTab, selectSession],
  );
}
