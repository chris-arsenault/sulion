import { describe, expect, it, beforeEach } from "vitest";
import { renderHook, act } from "@testing-library/react";

import {
  KNOWN_OPERATION_CATEGORIES,
  OPERATION_CATEGORY_LABELS,
  useTimelineFilters,
} from "./filters";

describe("timeline filters", () => {
  beforeEach(() => window.localStorage.clear());

  it("starts with the expected defaults", () => {
    const { result } = renderHook(() => useTimelineFilters());
    expect(result.current.filters.hiddenSpeakers.size).toBe(0);
    expect(result.current.filters.hiddenOperationCategories.size).toBe(0);
    expect(result.current.filters.errorsOnly).toBe(false);
    expect(result.current.filters.showThinking).toBe(true);
    expect(result.current.filters.followLatest).toBe(false);
  });

  it("persists the follow-latest toggle across reloads", () => {
    const { result, rerender } = renderHook(() => useTimelineFilters());
    act(() => result.current.setFollowLatest(true));
    rerender();
    const { result: second } = renderHook(() => useTimelineFilters());
    expect(second.current.filters.followLatest).toBe(true);
  });

  it("toggles speaker and operation-category visibility", () => {
    const { result } = renderHook(() => useTimelineFilters());
    act(() => {
      result.current.toggleSpeaker("assistant");
      result.current.toggleOperationCategory("utility");
    });
    expect(result.current.filters.hiddenSpeakers.has("assistant")).toBe(true);
    expect(result.current.filters.hiddenOperationCategories.has("utility")).toBe(true);
  });

  it("persists filter state to localStorage", () => {
    const { result, rerender } = renderHook(() => useTimelineFilters());
    act(() => {
      result.current.setErrorsOnly(true);
      result.current.setFilePath("foo.ts");
    });
    rerender();

    const { result: second } = renderHook(() => useTimelineFilters());
    expect(second.current.filters.errorsOnly).toBe(true);
    expect(second.current.filters.filePath).toBe("foo.ts");
  });

  it("exports the expected operation categories and labels", () => {
    expect(KNOWN_OPERATION_CATEGORIES).toEqual([
      "create_content",
      "inspect",
      "utility",
      "research",
      "delegate",
      "workflow",
      "other",
    ]);
    expect(OPERATION_CATEGORY_LABELS.create_content).toBe("create content");
  });
});
