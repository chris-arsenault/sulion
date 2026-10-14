import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CognitoAccessToken, CognitoIdToken, CognitoRefreshToken, CognitoUser, CognitoUserPool, CognitoUserSession } from "amazon-cognito-identity-js";
import { getAccessToken, signOut } from "./cognito";
import { advanceAuthGeneration, AuthenticationRequiredError } from "./session";
import { authFetch } from "../api/client";

const config = { cognitoUserPoolId: "us-east-1_fixture", cognitoClientId: "fixture" };
const prefix = "CognitoIdentityServiceProvider.fixture";

function token(lifetime: number, label: string) {
  const now = Math.floor(Date.now() / 1000);
  return `${btoa("{}")}.${btoa(JSON.stringify({ iat: now, exp: now + lifetime, sub: "test", label }))}.signature`;
}

function seed(lifetime: number, label = "initial") {
  const access = token(lifetime, label);
  const user = new CognitoUser({ Username: "test", Pool: new CognitoUserPool({ UserPoolId: config.cognitoUserPoolId, ClientId: config.cognitoClientId }) });
  user.setSignInUserSession(new CognitoUserSession({
    AccessToken: new CognitoAccessToken({ AccessToken: access }),
    IdToken: new CognitoIdToken({ IdToken: token(lifetime, label) }),
    RefreshToken: new CognitoRefreshToken({ RefreshToken: "refresh-fixture" }),
  }));
  return access;
}

function refreshResponse(label = "refreshed") {
  return Response.json({ AuthenticationResult: { AccessToken: token(300, label), IdToken: token(300, label) } });
}

describe("Cognito refresh races with the real SDK and token storage", () => {
  beforeEach(() => {
    advanceAuthGeneration();
    vi.useFakeTimers({ toFake: ["Date"] });
    window.localStorage.clear();
    window.__APP_CONFIG__ = config;
  });
  afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); });

  it.each([60, 1, 0, -1])("shares one refresh across eight requests with %s seconds left", async (lifetime) => {
    const initial = seed(lifetime);
    const fetch = vi.fn().mockImplementation(async () => refreshResponse());
    vi.stubGlobal("fetch", fetch);
    const results = await Promise.all(Array.from({ length: 8 }, () => getAccessToken()));
    expect(new Set(results).size).toBe(1);
    expect(results[0]).not.toBe(initial);
    expect(fetch).toHaveBeenCalledOnce();
    expect(JSON.parse(fetch.mock.calls[0][1].body)).toMatchObject({ AuthFlow: "REFRESH_TOKEN_AUTH", AuthParameters: { REFRESH_TOKEN: "refresh-fixture" } });
    expect(window.localStorage.getItem(`${prefix}.test.accessToken`)).toBe(results[0]);
  });

  it("refreshes 60 seconds early with the SDK's cached clock drift", async () => {
    seed(360);
    window.localStorage.setItem(`${prefix}.test.clockDrift`, "-300");
    const fetch = vi.fn().mockImplementation(async () => refreshResponse());
    vi.stubGlobal("fetch", fetch);
    await getAccessToken();
    expect(fetch).toHaveBeenCalledOnce();
  });

  it("keeps a healthy token and reuses a newer token for a late rejection", async () => {
    const initial = seed(300);
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    expect(await getAccessToken()).toBe(initial);
    expect(await getAccessToken({ rejectedToken: "old-token" })).toBe(initial);
    expect(fetch).not.toHaveBeenCalled();
  });

  it("recovers eight rejected requests with one refresh and one replay each", async () => {
    const initial = seed(300);
    const refresh = vi.fn().mockImplementation(async () => refreshResponse());
    const api = vi.fn().mockImplementation(async (_url, init) => new Response(null, {
      status: init.headers.get("Authorization") === `Bearer ${initial}` ? 401 : 200,
    }));
    vi.stubGlobal("fetch", vi.fn((url, init) => String(url).startsWith("https://cognito-idp.") ? refresh(url, init) : api(url, init)));
    const responses = await Promise.all(Array.from({ length: 8 }, () => authFetch("/api/ws-tickets", { method: "POST", body: JSON.stringify({ session_id: "fixture" }) })));
    expect(responses.map((response) => response.status)).toEqual(Array(8).fill(200));
    expect(refresh).toHaveBeenCalledOnce();
    expect(api).toHaveBeenCalledTimes(16);
    for (const [, init] of api.mock.calls) expect(init.body).toBe(JSON.stringify({ session_id: "fixture" }));
  });

  it("coalesces concurrent forced recovery even if Cognito returns an identical token", async () => {
    const initial = seed(300);
    const fetch = vi.fn().mockImplementation(async () => refreshResponse("initial"));
    vi.stubGlobal("fetch", fetch);
    const tokens = await Promise.all(Array.from({ length: 8 }, () => getAccessToken({ rejectedToken: initial })));
    expect(tokens).toEqual(Array(8).fill(initial));
    expect(fetch).toHaveBeenCalledOnce();
  });

  it.each(["network", "service"])("retains credentials through a %s outage and recovers", async (failure) => {
    const initial = seed(30);
    const fetch = vi.fn().mockImplementation(async () => {
      if (failure === "network") throw new TypeError("offline");
      return Response.json({ __type: "InternalErrorException", message: "unavailable" }, { status: 500 });
    });
    vi.stubGlobal("fetch", fetch);
    const failed = expect(getAccessToken()).rejects.not.toBeInstanceOf(AuthenticationRequiredError);
    await failed;
    expect(window.localStorage.getItem(`${prefix}.test.accessToken`)).toBe(initial);
    expect(window.localStorage.getItem(`${prefix}.test.refreshToken`)).toBe("refresh-fixture");
    fetch.mockImplementation(async () => refreshResponse());
    expect(await getAccessToken()).not.toBe(initial);
  }, 15000);

  it.each(["success", "refusal"])("a late refresh %s cannot overwrite a newer login", async (result) => {
    seed(30);
    let finish!: (response: Response) => void;
    const response = new Promise<Response>((resolve) => { finish = resolve; });
    const fetch = vi.fn().mockImplementation(() => response.then((value) => value.clone()));
    vi.stubGlobal("fetch", fetch);
    const pending = expect(getAccessToken()).rejects.toBeInstanceOf(AuthenticationRequiredError);
    await vi.waitFor(() => expect(fetch).toHaveBeenCalledOnce());
    signOut();
    expect(window.localStorage.getItem(`${prefix}.LastAuthUser`)).toBeNull();
    advanceAuthGeneration();
    const newer = seed(300, "new-login");
    expect(await getAccessToken()).toBe(newer);
    finish(result === "success" ? refreshResponse("old-login") : Response.json({ __type: "NotAuthorizedException", message: "expired" }, { status: 400 }));
    await pending;
    expect(await getAccessToken()).toBe(newer);
    expect(window.localStorage.getItem(`${prefix}.test.refreshToken`)).toBe("refresh-fixture");
  }, 15000);

  it("a late successful refresh cannot restore a signed-out login", async () => {
    seed(30);
    let finish!: (response: Response) => void;
    vi.stubGlobal("fetch", vi.fn(() => new Promise<Response>((resolve) => { finish = resolve; })));
    const pending = expect(getAccessToken()).rejects.toBeInstanceOf(AuthenticationRequiredError);
    await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
    signOut();
    finish(refreshResponse());
    await pending;
    expect(window.localStorage.getItem(`${prefix}.LastAuthUser`)).toBeNull();
    expect(await getAccessToken()).toBeNull();
  });
});
