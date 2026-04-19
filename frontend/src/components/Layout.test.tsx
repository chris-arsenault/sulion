import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { Layout } from "./Layout";
import { SessionProvider } from "../state/SessionStore";

// matchMedia is not implemented in jsdom — stub it so useMediaQuery can
// observe breakpoints deterministically.
function stubMatchMedia(matches: (q: string) => boolean) {
  window.matchMedia = vi.fn().mockImplementation((query: string) => ({
    matches: matches(query),
    media: query,
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  }));
}

beforeEach(() => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo) => {
      const url = typeof input === "string" ? input : (input as Request).url;
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

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("Layout", () => {
  it("desktop (>=768px): renders the sidebar inline (no drawer)", async () => {
    stubMatchMedia(() => false); // no mobile media match
    render(
      <SessionProvider>
        <Layout />
      </SessionProvider>,
    );
    // Hamburger is mobile-only; it should not be in the DOM.
    expect(screen.queryByLabelText(/open sessions drawer/i)).toBeNull();
    // Empty-state copy is the desktop variant.
    expect(
      screen.getByText(/select a session from the sidebar/i),
    ).toBeDefined();
  });

  it("mobile (<768px): shows hamburger, hides sidebar until toggled", async () => {
    stubMatchMedia((q) => q.includes("max-width: 767px"));
    render(
      <SessionProvider>
        <Layout />
      </SessionProvider>,
    );
    const hamburger = screen.getByLabelText(/open sessions drawer/i);
    expect(hamburger).toBeDefined();

    // Drawer not open — the Sidebar-internal "No repos yet." text is
    // only visible once the drawer opens. (We can't rely solely on the
    // drawer aside being absent because the Sidebar component may still
    // render somewhere; instead we assert the aria-label "Sessions" is
    // not present on the drawer yet.)
    expect(screen.queryByLabelText(/^sessions$/i)).toBeNull();

    const user = userEvent.setup();
    await user.click(hamburger);
    await waitFor(() => {
      expect(screen.getByLabelText(/^sessions$/i)).toBeDefined();
    });
  });

  it("mobile empty state shows the phone-friendly hint", () => {
    stubMatchMedia((q) => q.includes("max-width: 767px"));
    render(
      <SessionProvider>
        <Layout />
      </SessionProvider>,
    );
    expect(screen.getByText(/tap ☰ to open the session list/i)).toBeDefined();
  });
});
