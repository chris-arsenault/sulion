import { afterEach, describe, expect, it } from "vitest";

import {
  isSessionUnread,
  loadLastViewedMap,
  markLastViewed,
  saveLastViewedMap,
} from "./useLastViewed";

const SESSION_A = "sess-a";
const T0 = "2025-01-01T00:00:00Z";

describe("useLastViewed helpers", () => {
  afterEach(() => {
    window.localStorage.clear();
  });

  it("treats a session with events but no viewed-at as unread", () => {
    expect(isSessionUnread({}, SESSION_A, T0)).toBe(true);
  });

  it("returns false when the session has never produced events", () => {
    expect(isSessionUnread({}, SESSION_A, null)).toBe(false);
  });

  it("markLastViewed makes isUnread=false for that session", () => {
    const early = T0;
    const map = markLastViewed({}, SESSION_A, "2025-01-01T00:01:00Z");
    expect(isSessionUnread(map, SESSION_A, early)).toBe(false);
  });

  it("new events after markLastViewed flip isUnread back to true", () => {
    const map = markLastViewed({}, SESSION_A, T0);
    expect(isSessionUnread(map, SESSION_A, "2025-01-01T00:02:00Z")).toBe(true);
  });

  it("persists across loads via localStorage", () => {
    const map = markLastViewed({}, SESSION_A, T0);
    saveLastViewedMap(map);
    expect(loadLastViewedMap()[SESSION_A]).toBe(T0);
  });

  it("ignores malformed localStorage", () => {
    window.localStorage.setItem("sulion.lastViewed.v1", "not json");
    expect(loadLastViewedMap()).toEqual({});
  });
});
