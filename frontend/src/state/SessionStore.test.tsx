import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { appStatePayload, jsonResponse } from "../test/appState";
import { useSessions, useSessionStore } from "./SessionStore";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

describe("SessionStore app-state polling", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("shares an in-flight refresh and schedules the next poll after it completes", async () => {
    const first = deferred<Response>();
    const fetchMock = vi
      .fn<typeof fetch>()
      .mockReturnValueOnce(first.promise)
      .mockResolvedValue(jsonResponse(appStatePayload()));
    vi.stubGlobal("fetch", fetchMock);

    const { unmount } = renderHook(() =>
      useSessions((state) => state.sessionsLoaded),
    );
    const manualRefresh = useSessionStore.getState().refresh();

    await act(async () => {
      await Promise.resolve();
    });
    expect(fetchMock).toHaveBeenCalledTimes(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(12_000);
    });
    expect(fetchMock).toHaveBeenCalledTimes(1);

    await act(async () => {
      first.resolve(jsonResponse(appStatePayload()));
      await manualRefresh;
    });

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2_999);
    });
    expect(fetchMock).toHaveBeenCalledTimes(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(fetchMock).toHaveBeenCalledTimes(2);

    unmount();
  });
});
