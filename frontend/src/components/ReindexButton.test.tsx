import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { ReindexButton } from "./ReindexButton";

function installFetchMock(
  handler: (url: string, init?: RequestInit) => Response | Promise<Response>,
) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo, init?: RequestInit) => {
      const url = typeof input === "string" ? input : (input as Request).url;
      return handler(url, init);
    }),
  );
}

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

describe("ReindexButton", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("renders and is enabled by default", () => {
    installFetchMock(() => jsonResponse({}));
    render(<ReindexButton />);
    const btn = screen.getByTestId("reindex-button");
    expect(btn.textContent).toMatch(/reindex/i);
    expect(btn).toHaveProperty("disabled", false);
  });

  it("click opens the typed-phrase confirm dialog", async () => {
    installFetchMock(() => jsonResponse({}));
    const user = userEvent.setup();
    render(<ReindexButton />);
    await user.click(screen.getByTestId("reindex-button"));
    expect(screen.getByText("Reindex transcripts?")).toBeDefined();
    expect(screen.getByLabelText(/type refresh to confirm/i)).toBeDefined();
    // Confirm is disabled until phrase matches.
    expect(
      screen.getByRole("button", { name: "Reindex" }),
    ).toHaveProperty("disabled", true);
  });

  it("typing 'refresh' unlocks confirm, which POSTs the reindex endpoint and shows stats", async () => {
    const calls: Array<{ url: string; method: string | undefined }> = [];
    installFetchMock((url, init) => {
      calls.push({ url, method: init?.method });
      if (url === "/api/admin/reindex") {
        return jsonResponse({
          sessions_rebuilt: 3,
          events_preserved: 42,
          canonical_events_rebuilt: 42,
          timeline_sessions_rebuilt: 3,
        });
      }
      return new Response("", { status: 404 });
    });

    const user = userEvent.setup();
    render(<ReindexButton />);
    await user.click(screen.getByTestId("reindex-button"));
    await user.type(screen.getByLabelText(/type refresh to confirm/i), "refresh");
    await user.click(screen.getByRole("button", { name: "Reindex" }));

    await waitFor(() => {
      expect(calls.some(
        (c) => c.url === "/api/admin/reindex" && c.method === "POST",
      )).toBe(true);
    });
    await waitFor(() => {
      expect(screen.getByText("Reindex complete")).toBeDefined();
    });
    expect(
      screen.getByText(/Rebuilt 3 transcript sessions from 42 preserved event rows/),
    ).toBeDefined();
    expect(
      screen.getByText(/Canonical rows rebuilt: 42; timeline sessions rebuilt: 3/i),
    ).toBeDefined();
  });

  it("shows an error dialog when the reindex request fails", async () => {
    installFetchMock(() =>
      jsonResponse({ error: "db unreachable" }, 500),
    );
    const user = userEvent.setup();
    render(<ReindexButton />);
    await user.click(screen.getByTestId("reindex-button"));
    await user.type(screen.getByLabelText(/type refresh to confirm/i), "refresh");
    await user.click(screen.getByRole("button", { name: "Reindex" }));
    await waitFor(() => {
      expect(screen.getByText("Reindex failed")).toBeDefined();
      expect(screen.getByText(/db unreachable/)).toBeDefined();
    });
  });
});
