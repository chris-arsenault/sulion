// Tracks per-session "last viewed at" timestamps in localStorage.
// Combined with the server's last_event_at field, this lets the sidebar
// show an unread dot on sessions that have produced events since the
// user last opened them (ticket #23).

import { useCallback, useEffect, useState } from "react";

const STORAGE_KEY = "shuttlecraft.lastViewed.v1";

type LastViewedMap = Record<string, string>; // sessionId → ISO timestamp

function loadFromStorage(): LastViewedMap {
  if (typeof window === "undefined") return {};
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const out: LastViewedMap = {};
    for (const [k, v] of Object.entries(parsed)) {
      if (typeof v === "string") out[k] = v;
    }
    return out;
  } catch {
    return {};
  }
}

function saveToStorage(m: LastViewedMap) {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(m));
  } catch {
    // storage full or disabled
  }
}

/** Hook: returns `{ isUnread, markViewed }` plus the raw map for
 * diagnostics. `isUnread(id, lastEventAt)` returns true when the
 * session has new activity the user hasn't seen. */
export function useLastViewed(): {
  isUnread: (sessionId: string, lastEventAt: string | null) => boolean;
  markViewed: (sessionId: string) => void;
  map: LastViewedMap;
} {
  const [map, setMap] = useState<LastViewedMap>(() => loadFromStorage());

  useEffect(() => {
    saveToStorage(map);
  }, [map]);

  const markViewed = useCallback((sessionId: string) => {
    setMap((prev) => ({ ...prev, [sessionId]: new Date().toISOString() }));
  }, []);

  const isUnread = useCallback(
    (sessionId: string, lastEventAt: string | null): boolean => {
      if (!lastEventAt) return false;
      const viewedAt = map[sessionId];
      if (!viewedAt) return true; // never viewed, but has events → unread
      const ev = Date.parse(lastEventAt);
      const seen = Date.parse(viewedAt);
      if (Number.isNaN(ev) || Number.isNaN(seen)) return false;
      return ev > seen;
    },
    [map],
  );

  return { isUnread, markViewed, map };
}
