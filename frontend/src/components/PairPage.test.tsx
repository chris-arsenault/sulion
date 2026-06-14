import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { PairPage } from "./PairPage";

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

function setUrl(search: string) {
  window.history.replaceState(null, "", `/pair${search}`);
}

describe("PairPage", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    setUrl("");
  });

  it("prefills the code from ?code= and approves, POSTing the approve endpoint", async () => {
    setUrl("?code=wxyz-1234");
    const calls: { url: string; method: string | null; body: string | null }[] =
      [];
    installFetchMock((url, init) => {
      calls.push({
        url,
        method: init?.method ?? null,
        body: typeof init?.body === "string" ? init.body : null,
      });
      if (url === "/api/devices/pair/approve") {
        return jsonResponse({
          status: "approved",
          client: "ableton-extensions",
          user_code: "WXYZ-1234",
        });
      }
      return new Response("", { status: 404 });
    });

    const user = userEvent.setup();
    render(<PairPage />);

    // Code is normalized to upper-case from the query string.
    const input = screen.getByTestId("pair-code-input") as HTMLInputElement;
    expect(input.value).toBe("WXYZ-1234");

    await user.click(screen.getByTestId("pair-approve"));

    await waitFor(() => {
      expect(
        calls.some(
          (c) =>
            c.url === "/api/devices/pair/approve" &&
            c.method === "POST" &&
            c.body === JSON.stringify({ user_code: "WXYZ-1234" }),
        ),
      ).toBe(true);
    });
    await waitFor(() => {
      expect(screen.getByTestId("pair-approved")).toBeDefined();
    });
    expect(screen.getByText(/ableton-extensions/)).toBeDefined();
    expect(screen.getByText(/can now send to Sulion/i)).toBeDefined();
  });

  it("shows the backend error message when the code is unknown (404)", async () => {
    setUrl("?code=ZZZZ-9999");
    installFetchMock(() =>
      jsonResponse({ error: "not found" }, 404),
    );

    const user = userEvent.setup();
    render(<PairPage />);
    await user.click(screen.getByTestId("pair-approve"));

    await waitFor(() => {
      expect(screen.getByRole("alert").textContent).toMatch(/not found/i);
    });
    // Still on the form, not the approved state.
    expect(screen.queryByTestId("pair-approved")).toBeNull();
  });

  it("disables Approve until a code is present", async () => {
    setUrl(""); // no code in the URL
    installFetchMock(() => jsonResponse({}));
    render(<PairPage />);

    const approve = screen.getByTestId("pair-approve");
    expect(approve).toHaveProperty("disabled", true);

    const user = userEvent.setup();
    await user.type(screen.getByTestId("pair-code-input"), "abcd-1234");
    expect(approve).toHaveProperty("disabled", false);
  });
});
