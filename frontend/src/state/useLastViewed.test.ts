import { afterEach, describe, expect, it } from "vitest";
import { act, renderHook } from "@testing-library/react";

import { useLastViewed } from "./useLastViewed";

describe("useLastViewed", () => {
  afterEach(() => {
    window.localStorage.clear();
  });

  it("treats a session with events but no viewed-at as unread", () => {
    const { result } = renderHook(() => useLastViewed());
    expect(
      result.current.isUnread("sess-a", "2025-01-01T00:00:00Z"),
    ).toBe(true);
  });

  it("returns false when the session has never produced events", () => {
    const { result } = renderHook(() => useLastViewed());
    expect(result.current.isUnread("sess-a", null)).toBe(false);
  });

  it("markViewed makes isUnread=false for that session", () => {
    const { result } = renderHook(() => useLastViewed());
    const early = "2025-01-01T00:00:00Z";
    expect(result.current.isUnread("sess-a", early)).toBe(true);
    act(() => result.current.markViewed("sess-a"));
    expect(result.current.isUnread("sess-a", early)).toBe(false);
  });

  it("new events after markViewed flip isUnread back to true", () => {
    const { result } = renderHook(() => useLastViewed());
    act(() => result.current.markViewed("sess-a"));
    // Force a much later event timestamp than "now"
    const future = new Date(Date.now() + 60_000).toISOString();
    expect(result.current.isUnread("sess-a", future)).toBe(true);
  });

  it("persists across hook instances via localStorage", () => {
    const { result } = renderHook(() => useLastViewed());
    act(() => result.current.markViewed("sess-a"));
    const { result: r2 } = renderHook(() => useLastViewed());
    expect(r2.current.map["sess-a"]).toBeDefined();
  });

  it("ignores malformed localStorage", () => {
    window.localStorage.setItem(
      "shuttlecraft.lastViewed.v1",
      "not json",
    );
    const { result } = renderHook(() => useLastViewed());
    expect(result.current.map).toEqual({});
  });
});
