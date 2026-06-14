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

interface FetchCall {
  url: string;
  method: string | null;
  body?: string | null;
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
    expect(screen.getByTestId("retrieval-backfill-button")).toHaveProperty(
      "disabled",
      false,
    );
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
    const calls: FetchCall[] = [];
    installFetchMock((url, init) => {
      calls.push({ url, method: init?.method ?? null });
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

  it("typing 'refresh' marks retrieval sources dirty and shows queue stats", async () => {
    const calls: FetchCall[] = [];
    installFetchMock((url, init) => {
      const body = typeof init?.body === "string" ? init.body : null;
      calls.push({
        url,
        method: init?.method ?? null,
        body,
      });
      if (url === "/api/admin/retrieval/reindex") {
        return jsonResponse({
          generation: 9,
          backfills_started: 3,
          sources_seen: 12,
          sources_marked_pending: 10,
          sources_deleted: 1,
          pending_sources: 10,
          vector: {
            extension_installed: true,
            column_exists: true,
            ann_index_exists: true,
          },
          embedding_model: "nomic-ai/nomic-embed-text-v1.5",
          embedding_dimensions: 768,
        });
      }
      return new Response("", { status: 404 });
    });

    const user = userEvent.setup();
    render(<ReindexButton />);
    await user.click(screen.getByTestId("retrieval-backfill-button"));
    await user.type(screen.getByLabelText(/type refresh to confirm/i), "refresh");
    await user.click(screen.getByRole("button", { name: "Refresh" }));

    await waitFor(() => {
      expect(
        calls.some(
          (c) =>
            c.url === "/api/admin/retrieval/reindex" &&
            c.method === "POST" &&
            c.body === JSON.stringify({}),
        ),
      ).toBe(true);
    });
    await waitFor(() => {
      expect(screen.getByText("Retrieval refresh queued")).toBeDefined();
    });
    expect(
      screen.getByText(/Started 3 retrieval backfills for generation 9/),
    ).toBeDefined();
    expect(screen.getByText(/nomic-ai\/nomic-embed-text-v1.5 \(768d\)/)).toBeDefined();
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
      expect(screen.getByText("Admin action failed")).toBeDefined();
      expect(screen.getByText(/db unreachable/)).toBeDefined();
    });
  });
});
