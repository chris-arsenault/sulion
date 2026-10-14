import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AuthGate, AuthProvider } from "./AuthProvider";
import { authFetch } from "../api/client";
import { getAccessToken, isAuthConfigured, signOut } from "./cognito";
import * as cognito from "./cognito";
import { advanceAuthGeneration, AuthenticationRequiredError } from "./session";

const session = { accessToken: "fixture", username: "test", email: null };
const PRIVATE_CONTENT = "private content";
const APP_STATE_URL = "/api/app-state";

async function renderSession() {
  render(<AuthProvider><AuthGate><div>{PRIVATE_CONTENT}</div></AuthGate></AuthProvider>);
  await screen.findByText(PRIVATE_CONTENT);
}

function expectLogin() {
  expect(screen.getByRole("heading", { name: "Authenticate to continue" })).toBeTruthy();
  expect(screen.queryByText(PRIVATE_CONTENT)).toBeNull();
}

describe("authentication recovery and sign-out", () => {
  beforeEach(() => {
    vi.spyOn(cognito, "isAuthConfigured").mockReturnValue(true);
    vi.spyOn(cognito, "getSessionSnapshot").mockResolvedValue(session);
    vi.spyOn(cognito, "getAccessToken").mockResolvedValue("fixture");
    vi.spyOn(cognito, "signOut").mockImplementation(advanceAuthGeneration);
    vi.spyOn(cognito, "signIn").mockImplementation(async () => {
      advanceAuthGeneration();
      return { kind: "authenticated", session };
    });
    window.__APP_CONFIG__ = { cognitoUserPoolId: "fixture", cognitoClientId: "fixture" };
  });

  afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); vi.useRealTimers(); });

  it("retries a temporarily unavailable startup refresh without showing a login form", async () => {
    vi.useFakeTimers();
    vi.mocked(cognito.getSessionSnapshot).mockRejectedValueOnce(new TypeError("offline")).mockResolvedValue(session);
    render(<AuthProvider><AuthGate><div>{PRIVATE_CONTENT}</div></AuthGate></AuthProvider>);
    await act(async () => { await Promise.resolve(); });
    expect(screen.getByRole("status").textContent).toContain("Retrying");
    expect(screen.queryByLabelText("Password")).toBeNull();
    expect(signOut).not.toHaveBeenCalled();
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(screen.getByText(PRIVATE_CONTENT)).toBeTruthy();
  });

  it("cancels startup recovery when the user signs out", async () => {
    vi.useFakeTimers();
    vi.mocked(cognito.getSessionSnapshot).mockRejectedValue(new TypeError("offline"));
    render(<AuthProvider><AuthGate><div>{PRIVATE_CONTENT}</div></AuthGate></AuthProvider>);
    await act(async () => { await Promise.resolve(); });
    fireEvent.click(screen.getByRole("button", { name: "sign out (this device)" }));
    await act(async () => { await vi.advanceTimersByTimeAsync(30000); });
    expectLogin();
    expect(cognito.getSessionSnapshot).toHaveBeenCalledOnce();
  });

  it("recovers one rejected token and preserves a POST body and abort signal", async () => {
    const fetch = vi.fn().mockResolvedValueOnce(new Response(null, { status: 401 })).mockResolvedValue(new Response(null, { status: 200 }));
    vi.stubGlobal("fetch", fetch);
    vi.mocked(getAccessToken).mockResolvedValueOnce("expired").mockResolvedValue("fresh");
    await renderSession();
    const signal = new AbortController().signal;
    await act(async () => { expect((await authFetch(APP_STATE_URL, { method: "POST", body: "payload", signal })).status).toBe(200); });
    expect(getAccessToken).toHaveBeenCalledWith({ rejectedToken: "expired" });
    expect(fetch.mock.calls[1][1]).toMatchObject({ method: "POST", body: "payload", signal });
    expect(fetch.mock.calls[1][1].headers.get("Authorization")).toBe("Bearer fresh");
    expect(signOut).not.toHaveBeenCalled();
  });

  it.each(["refresh", "retry"])("retains login if %s fails during 401 recovery", async (stage) => {
    const error = new TypeError("offline");
    const fetch = vi.fn().mockResolvedValueOnce(new Response(null, { status: 401 })).mockRejectedValue(error);
    vi.stubGlobal("fetch", fetch);
    if (stage === "refresh") vi.mocked(getAccessToken).mockResolvedValueOnce("fixture").mockRejectedValue(error);
    await renderSession();
    await act(async () => { await expect(authFetch(APP_STATE_URL)).rejects.toBe(error); });
    expect(screen.getByText(PRIVATE_CONTENT)).toBeTruthy();
    expect(signOut).not.toHaveBeenCalled();
    expect(fetch).toHaveBeenCalledTimes(stage === "refresh" ? 1 : 2);
  });

  it("retains login when authority becomes unavailable during token recovery", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValueOnce(new Response(null, { status: 401 })).mockResolvedValue(new Response(null, { status: 503 })));
    await renderSession();
    await act(async () => { expect((await authFetch(APP_STATE_URL)).status).toBe(503); });
    expect(signOut).not.toHaveBeenCalled();
  });

  it("ignores a rejected retry after another request obtains a newer token", async () => {
    vi.stubGlobal("fetch", vi.fn().mockImplementation(async () => new Response(null, { status: 401 })));
    vi.mocked(getAccessToken).mockResolvedValueOnce("old").mockResolvedValueOnce("retry").mockResolvedValue("newer");
    await renderSession();
    await act(async () => { await authFetch(APP_STATE_URL); });
    expect(signOut).not.toHaveBeenCalled();
    expect(screen.getByText(PRIVATE_CONTENT)).toBeTruthy();
  });

  it("does not replace explicit authorization or clear the browser login for it", async () => {
    vi.mocked(getAccessToken).mockRejectedValue(new AuthenticationRequiredError());
    const fetch = vi.fn().mockResolvedValue(new Response(null, { status: 401 }));
    vi.stubGlobal("fetch", fetch);
    await renderSession();
    await act(async () => { await authFetch(APP_STATE_URL, { headers: { Authorization: "Bearer explicit" } }); });
    expect(fetch).toHaveBeenCalledOnce();
    expect(getAccessToken).not.toHaveBeenCalled();
    expect(signOut).not.toHaveBeenCalled();
  });

  it("does not recover or replay an aborted request", async () => {
    const controller = new AbortController();
    const fetch = vi.fn().mockImplementation(async () => {
      controller.abort();
      return new Response(null, { status: 401 });
    });
    vi.stubGlobal("fetch", fetch);
    await renderSession();
    await act(async () => { await expect(authFetch(APP_STATE_URL, { signal: controller.signal })).rejects.toMatchObject({ name: "AbortError" }); });
    expect(fetch).toHaveBeenCalledOnce();
    expect(getAccessToken).toHaveBeenCalledOnce();
    expect(signOut).not.toHaveBeenCalled();
  });

  it("refreshes but does not replay a consumed stream body", async () => {
    const fetch = vi.fn().mockResolvedValue(new Response(null, { status: 401 }));
    vi.stubGlobal("fetch", fetch);
    await renderSession();
    await act(async () => { await authFetch(APP_STATE_URL, { method: "POST", body: new ReadableStream() }); });
    expect(fetch).toHaveBeenCalledOnce();
    expect(getAccessToken).toHaveBeenCalledWith({ rejectedToken: "fixture" });
    expect(signOut).not.toHaveBeenCalled();
  });

  it.each([APP_STATE_URL, "/api/ws-tickets", "/broker/v1/secrets"])(
    "returns to login on a 401 from %s without revoking other devices",
    async (url) => {
      const fetch = vi.fn().mockResolvedValue(new Response(null, { status: 401 }));
      vi.stubGlobal("fetch", fetch);
      await renderSession();
      await act(async () => { expect((await authFetch(url)).status).toBe(401); });
      expectLogin();
      expect(signOut).toHaveBeenCalledOnce();
      expect(fetch).toHaveBeenCalledTimes(2);
      expect(fetch.mock.calls[0][0]).toBe(url);
    },
  );

  it.each([403, 500, 503])("keeps the session after HTTP %s", async (status) => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(null, { status })));
    await renderSession();
    await act(async () => { await authFetch(APP_STATE_URL); });
    expect(screen.getByText(PRIVATE_CONTENT)).toBeTruthy();
    expect(signOut).not.toHaveBeenCalled();
  });

  it.each([APP_STATE_URL, "/api/ws-tickets", "/broker/v1/secrets"])(
    "recovers after repeated key-service failures from %s without logging in again",
    async (url) => {
      const fetch = vi.fn()
        .mockImplementationOnce(async () => new Response(null, { status: 503 }))
        .mockImplementationOnce(async () => new Response(null, { status: 503 }))
        .mockImplementationOnce(async () => new Response(null, { status: 503 }))
        .mockImplementationOnce(async () => new Response(null, { status: 200 }))
        .mockImplementation(async () => new Response(null, { status: 401 }));
      vi.stubGlobal("fetch", fetch);
      await renderSession();
      for (const status of [503, 503, 503, 200]) {
        await act(async () => { expect((await authFetch(url)).status).toBe(status); });
        expect(screen.getByText(PRIVATE_CONTENT)).toBeTruthy();
        expect(signOut).not.toHaveBeenCalled();
      }
      for (const [, init] of fetch.mock.calls as unknown as [string, RequestInit][]) {
        expect(new Headers(init.headers).get("Authorization")).toBe("Bearer fixture");
      }
      await act(async () => { expect((await authFetch(url)).status).toBe(401); });
      expectLogin();
      expect(signOut).toHaveBeenCalledOnce();
    },
  );

  it.each([null, new AuthenticationRequiredError()])(
    "returns to login when credentials are missing or refresh is refused (%s)",
    async (result) => {
      const fetch = vi.fn();
      vi.stubGlobal("fetch", fetch);
      await renderSession();
      if (result instanceof Error) vi.mocked(getAccessToken).mockRejectedValue(result);
      else vi.mocked(getAccessToken).mockResolvedValue(result);
      await act(async () => {
        await expect(authFetch(APP_STATE_URL)).rejects.toMatchObject({ status: 401 });
      });
      expectLogin();
      expect(fetch).not.toHaveBeenCalled();
    },
  );

  it.each(["token refresh", "request"])("keeps the session after a %s network failure", async (stage) => {
    const error = new TypeError("Failed to fetch");
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(error));
    await renderSession();
    if (stage === "token refresh") vi.mocked(getAccessToken).mockRejectedValue(error);
    await act(async () => { await expect(authFetch(APP_STATE_URL)).rejects.toBe(error); });
    expect(screen.getByText(PRIVATE_CONTENT)).toBeTruthy();
    expect(signOut).not.toHaveBeenCalled();
  });

  it("signs out this device immediately without contacting the broker", async () => {
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    await renderSession();
    fireEvent.click(screen.getByRole("button", { name: "sign out (this device)" }));
    expectLogin();
    expect(signOut).toHaveBeenCalledOnce();
    expect(fetch).not.toHaveBeenCalled();
  });

  it("clears local login only after all-device revocation succeeds", async () => {
    let finish!: (response: Response) => void;
    const fetch = vi.fn().mockReturnValue(new Promise<Response>((resolve) => { finish = resolve; }));
    vi.stubGlobal("fetch", fetch);
    await renderSession();
    fireEvent.click(screen.getByRole("button", { name: "sign out (all devices)" }));
    await waitFor(() => expect(fetch).toHaveBeenCalledOnce());
    expect(fetch.mock.calls[0][0]).toBe("/broker/v1/session/revoke");
    expect(fetch.mock.calls[0][1].method).toBe("POST");
    expect(fetch.mock.calls[0][1].headers.get("Authorization")).toBe("Bearer fixture");
    expect(signOut).not.toHaveBeenCalled();
    await act(async () => { finish(new Response(null, { status: 204 })); });
    expectLogin();
    expect(signOut).toHaveBeenCalledOnce();
  });

  it("reports failed all-device revocation and retains login for retry", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response("broker unavailable", { status: 503 })));
    await renderSession();
    fireEvent.click(screen.getByRole("button", { name: "sign out (all devices)" }));
    expect((await screen.findByRole("alert")).textContent).toContain("503");
    expect(signOut).not.toHaveBeenCalled();
    expect(screen.getByText(PRIVATE_CONTENT)).toBeTruthy();
  });

  it("returns to login if all-device revocation rejects the current credentials", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(null, { status: 401 })));
    await renderSession();
    fireEvent.click(screen.getByRole("button", { name: "sign out (all devices)" }));
    await screen.findByRole("heading", { name: "Authenticate to continue" });
    expectLogin();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(signOut).toHaveBeenCalledOnce();
  });

  it("ignores a late 401 from a previous login", async () => {
    let finish!: (response: Response) => void;
    const fetch = vi.fn().mockReturnValue(new Promise<Response>((resolve) => { finish = resolve; }));
    vi.stubGlobal("fetch", fetch);
    await renderSession();
    const oldRequest = authFetch(APP_STATE_URL);
    await waitFor(() => expect(fetch).toHaveBeenCalledOnce());
    fireEvent.click(screen.getByRole("button", { name: "sign out (this device)" }));
    fireEvent.change(screen.getByLabelText("Username or email"), { target: { value: "test" } });
    fireEvent.change(screen.getByLabelText("Password"), { target: { value: "password" } });
    fireEvent.click(screen.getByRole("button", { name: "sign in" }));
    await screen.findByText(PRIVATE_CONTENT);
    await act(async () => {
      finish(new Response(null, { status: 401 }));
      await oldRequest;
    });
    expect(screen.getByText(PRIVATE_CONTENT)).toBeTruthy();
    expect(signOut).toHaveBeenCalledOnce();
  });

  it("does not send a request whose credentials resolved after sign-out", async () => {
    let finish!: (token: string) => void;
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    vi.mocked(getAccessToken).mockReturnValue(new Promise<string>((resolve) => { finish = resolve; }));
    await renderSession();
    const pending = authFetch(APP_STATE_URL);
    fireEvent.click(screen.getByRole("button", { name: "sign out (this device)" }));
    finish("fixture");
    await expect(pending).rejects.toMatchObject({ status: 401 });
    expect(fetch).not.toHaveBeenCalled();
  });

  it("preserves explicitly disabled authentication", async () => {
    vi.mocked(isAuthConfigured).mockReturnValue(false);
    vi.mocked(getAccessToken).mockResolvedValue(null);
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(null, { status: 401 })));
    await renderSession();
    await act(async () => { await authFetch(APP_STATE_URL); });
    expect(screen.getByText(PRIVATE_CONTENT)).toBeTruthy();
    expect(signOut).not.toHaveBeenCalled();
  });
});
