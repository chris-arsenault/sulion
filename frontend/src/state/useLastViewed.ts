const STORAGE_KEY = "shuttlecraft.lastViewed.v1";

export type LastViewedMap = Record<string, string>;

export function loadLastViewedMap(): LastViewedMap {
  if (typeof window === "undefined") return {};
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};

    const out: LastViewedMap = {};
    for (const [key, value] of Object.entries(parsed)) {
      if (typeof value === "string") out[key] = value;
    }
    return out;
  } catch {
    return {};
  }
}

export function saveLastViewedMap(map: LastViewedMap) {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(map));
  } catch {
    // storage full or disabled
  }
}

export function markLastViewed(
  map: LastViewedMap,
  sessionId: string,
  viewedAt = new Date().toISOString(),
): LastViewedMap {
  return { ...map, [sessionId]: viewedAt };
}

export function isSessionUnread(
  map: LastViewedMap,
  sessionId: string,
  lastEventAt: string | null,
): boolean {
  if (!lastEventAt) return false;
  const viewedAt = map[sessionId];
  if (!viewedAt) return true;

  const eventMs = Date.parse(lastEventAt);
  const seenMs = Date.parse(viewedAt);
  if (Number.isNaN(eventMs) || Number.isNaN(seenMs)) return false;
  return eventMs > seenMs;
}

export function clearLastViewedStorage() {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.removeItem(STORAGE_KEY);
  } catch {
    // storage unavailable
  }
}
