import { afterEach, describe, expect, it } from "vitest";

import {
  isSessionUnread,
  loadLastViewedMap,
  markLastViewed,
  saveLastViewedMap,
} from "./useLastViewed";

describe("useLastViewed helpers", () => {
  afterEach(() => {
    window.localStorage.clear();
  });

  it("treats a session with events but no viewed-at as unread", () => {
    expect(isSessionUnread({}, "sess-a", "2025-01-01T00:00:00Z")).toBe(true);
  });

  it("returns false when the session has never produced events", () => {
    expect(isSessionUnread({}, "sess-a", null)).toBe(false);
  });

  it("markLastViewed makes isUnread=false for that session", () => {
    const early = "2025-01-01T00:00:00Z";
    const map = markLastViewed({}, "sess-a", "2025-01-01T00:01:00Z");
    expect(isSessionUnread(map, "sess-a", early)).toBe(false);
  });

  it("new events after markLastViewed flip isUnread back to true", () => {
    const map = markLastViewed({}, "sess-a", "2025-01-01T00:00:00Z");
    expect(isSessionUnread(map, "sess-a", "2025-01-01T00:02:00Z")).toBe(true);
  });

  it("persists across loads via localStorage", () => {
    const map = markLastViewed({}, "sess-a", "2025-01-01T00:00:00Z");
    saveLastViewedMap(map);
    expect(loadLastViewedMap()["sess-a"]).toBe("2025-01-01T00:00:00Z");
  });

  it("ignores malformed localStorage", () => {
    window.localStorage.setItem("sulion.lastViewed.v1", "not json");
    expect(loadLastViewedMap()).toEqual({});
  });
});
