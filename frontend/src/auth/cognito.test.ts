import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CognitoUserPool } from "amazon-cognito-identity-js";
import { getAccessToken } from "./cognito";
import { advanceAuthGeneration, AuthenticationRequiredError } from "./session";

const getSession = vi.fn();
const getCurrentUser = vi.fn();

describe("Cognito session recovery", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    advanceAuthGeneration();
    window.__APP_CONFIG__ = { cognitoUserPoolId: "us-east-1_fixture", cognitoClientId: "fixture" };
    vi.spyOn(CognitoUserPool.prototype, "getCurrentUser").mockImplementation(getCurrentUser);
    getCurrentUser.mockReturnValue({ getSession });
  });

  afterEach(() => { vi.restoreAllMocks(); });

  it.each([
    { code: "NotAuthorizedException", message: "Refresh Token has expired" },
    { name: "UserNotFoundException", message: "User no longer exists" },
    new Error("Cannot retrieve a new session. Please authenticate."),
    new Error("Local storage is missing an ID Token, Please authenticate"),
  ])("identifies unusable credentials: %s", async (error) => {
    getSession.mockImplementation((callback) => callback(error, null));
    await expect(getAccessToken()).rejects.toBeInstanceOf(AuthenticationRequiredError);
  });

  it("preserves refresh network errors for retry", async () => {
    const error = new TypeError("Network error");
    getSession.mockImplementation((callback) => callback(error, null));
    await expect(getAccessToken()).rejects.toBe(error);
  });

  it.each([null, { isValid: () => false }])("retains credentials when the SDK returns an unusable session: %s", async (session) => {
    getSession.mockImplementation((callback) => callback(null, session));
    await expect(getAccessToken()).rejects.toThrow("Token refresh returned no usable session");
  });

  it("returns a refreshed access token", async () => {
    getSession.mockImplementation((callback) => callback(null, {
      isValid: () => true,
      getClockDrift: () => 0,
      getAccessToken: () => ({ getJwtToken: () => "refreshed", getExpiration: () => Date.now() / 1000 + 300 }),
    }));
    await expect(getAccessToken()).resolves.toBe("refreshed");
  });

  it("returns no token when this browser has no user", async () => {
    getCurrentUser.mockReturnValue(null);
    await expect(getAccessToken()).resolves.toBeNull();
  });
});
