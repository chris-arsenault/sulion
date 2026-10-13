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

  afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });

  it.each([APP_STATE_URL, "/api/ws-tickets", "/broker/v1/secrets"])(
    "returns to login on a 401 from %s without revoking other devices",
    async (url) => {
      const fetch = vi.fn().mockResolvedValue(new Response(null, { status: 401 }));
      vi.stubGlobal("fetch", fetch);
      await renderSession();
      await act(async () => { expect((await authFetch(url)).status).toBe(401); });
      expectLogin();
      expect(signOut).toHaveBeenCalledOnce();
      expect(fetch).toHaveBeenCalledTimes(1);
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
