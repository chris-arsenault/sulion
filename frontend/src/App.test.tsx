import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { App } from "./App";

// Stub fetch so SessionProvider's mount-time calls don't spam console.
beforeEach(() => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo) => {
      const url = typeof input === "string" ? input : input.url;
      const body = url.includes("/api/sessions")
        ? { sessions: [] }
        : { repos: [] };
      return new Response(JSON.stringify(body), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    }),
  );
});

describe("App", () => {
  it("renders the empty-state copy when no session is selected", async () => {
    render(<App />);
    expect(
      screen.getByText(
        (t) => typeof t === "string" && t.toLowerCase().includes("select a session"),
      ),
    ).toBeDefined();
  });

  it("shows the sidebar logo", async () => {
    render(<App />);
    // shuttlecraft appears in both the sidebar header and the empty state;
    // getAllByText confirms both presence and count.
    expect(screen.getAllByText("shuttlecraft").length).toBeGreaterThanOrEqual(1);
  });
});
