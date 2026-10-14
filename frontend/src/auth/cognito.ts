import {
  AuthenticationDetails,
  CognitoUser,
  CognitoUserPool,
  type CognitoUserSession,
} from "amazon-cognito-identity-js";

import { getAuthRuntimeConfig } from "./config";
import { advanceAuthGeneration, AuthenticationRequiredError, getAuthGeneration } from "./session";
import { storageForGeneration } from "./storage";

export type SessionSnapshot = {
  accessToken: string;
  email: string | null;
  username: string | null;
};

let cachedPool: CognitoUserPool | null = null;
let cachedPoolKey: string | null = null;
let pending: { generation: number; rejectedToken?: string; promise: Promise<CognitoUserSession | null> } | null = null;
const REFRESH_MARGIN_SECONDS = 60;

function getPool(): CognitoUserPool | null {
  const config = getAuthRuntimeConfig();
  if (!config) return null;
  const key = `${config.cognitoUserPoolId}:${config.cognitoClientId}:${getAuthGeneration()}`;
  if (!cachedPool || cachedPoolKey !== key) {
    cachedPool = new CognitoUserPool({
      UserPoolId: config.cognitoUserPoolId,
      ClientId: config.cognitoClientId,
      Storage: storageForGeneration(getAuthGeneration()),
    });
    cachedPoolKey = key;
  }
  return cachedPool;
}

export function isAuthConfigured(): boolean {
  return getPool() != null;
}

export function getCurrentUser(): CognitoUser | null {
  return getPool()?.getCurrentUser() ?? null;
}

function sessionError(err: Error): Error {
  const code = (err as Error & { code?: string }).code ?? err.name;
  const missingCredentials = [
    "Username is null. Cannot retrieve a new session",
    "Cannot retrieve a new session. Please authenticate.",
    "Local storage is missing an ID Token, Please authenticate",
  ].includes(err.message);
  return missingCredentials || code === "NotAuthorizedException" || code === "UserNotFoundException"
    ? new AuthenticationRequiredError() : err;
}

async function loadSession(rejectedToken?: string): Promise<CognitoUserSession | null> {
  const user = getCurrentUser();
  if (!user) return null;
  const session = await new Promise<CognitoUserSession | null>((resolve, reject) => {
    user.getSession((err: Error | null, session: CognitoUserSession | null) => {
      if (err) {
        reject(sessionError(err));
        return;
      }
      if (!session?.isValid()) {
        reject(new Error("Token refresh returned no usable session"));
        return;
      }
      resolve(session);
    });
  });
  if (!session) return null;
  const token = session.getAccessToken();
  // Leave time before the SDK's drift-adjusted expiry for requests and ticket
  // handshakes that may still be in flight when the server checks the token.
  const clockDrift = (session as CognitoUserSession & { getClockDrift(): number }).getClockDrift();
  const nearExpiry = token.getExpiration() <= Date.now() / 1000 - clockDrift + REFRESH_MARGIN_SECONDS;
  if (!nearExpiry && token.getJwtToken() !== rejectedToken) return session;
  return new Promise((resolve, reject) => {
    user.refreshSession(session.getRefreshToken(), (err, refreshed) => {
      if (err) reject(sessionError(err));
      else if (!refreshed?.isValid()) reject(new Error("Token refresh returned no usable session"));
      else resolve(refreshed);
    });
  });
}

export async function getCurrentSession(rejectedToken?: string): Promise<CognitoUserSession | null> {
  const generation = getAuthGeneration();
  if (pending?.generation === generation) {
    const operation = pending;
    const session = await operation.promise;
    if (generation !== getAuthGeneration()) throw new AuthenticationRequiredError();
    if (!rejectedToken || operation.rejectedToken || session?.getAccessToken().getJwtToken() !== rejectedToken) return session;
    return getCurrentSession(rejectedToken);
  }
  const operation = { generation, rejectedToken, promise: loadSession(rejectedToken) };
  pending = operation;
  try {
    const session = await operation.promise;
    if (generation !== getAuthGeneration()) throw new AuthenticationRequiredError();
    return session;
  } finally {
    if (pending === operation) pending = null;
  }
}

export async function getAccessToken(options?: { rejectedToken?: string }): Promise<string | null> {
  if (import.meta.env.VITE_SULION_E2E === "1") {
    const token = import.meta.env.VITE_SULION_E2E_ACCESS_TOKEN;
    if (typeof token === "string" && token.length > 0) return token;
  }
  const session = await getCurrentSession(options?.rejectedToken);
  return session?.getAccessToken().getJwtToken() ?? null;
}

export async function getSessionSnapshot(): Promise<SessionSnapshot | null> {
  const session = await getCurrentSession();
  if (!session) return null;
  const accessToken = session.getAccessToken().getJwtToken();
  const idPayload = session.getIdToken().decodePayload() as Record<string, unknown>;
  return {
    accessToken,
    email: readString(idPayload.email),
    username:
      readString(idPayload["cognito:username"]) ??
      readString(idPayload.username) ??
      readString(idPayload.email),
  };
}

export function signOut(): void {
  advanceAuthGeneration();
  getCurrentUser()?.signOut();
}

// The shared Ahara Cognito pool runs with software-token (TOTP) MFA required
// (see ahara INTEGRATION.md "Required OTP/MFA handling"). Authenticator
// enrollment (the Cognito MFA_SETUP challenge) is centralized in ahara-business
// and intentionally NOT implemented here: this app only completes the
// SOFTWARE_TOKEN_MFA login challenge for already-enrolled users, and sends
// un-enrolled users to ahara-business to enroll.
const ENROLL_VIA_AHARA_BUSINESS =
  "Multi-factor authentication isn't set up for this account. Enroll an " +
  "authenticator in the Ahara account portal (ahara-business), then sign in here.";

export type SignInChallenge = {
  // Cognito SOFTWARE_TOKEN_MFA: an enrolled user enters their current 6-digit code.
  kind: "softwareTokenMfa";
  submitCode: (code: string) => Promise<SignInResult>;
};

export type SignInResult =
  | { kind: "authenticated"; session: SessionSnapshot }
  | { kind: "challenge"; challenge: SignInChallenge };

export function signIn(username: string, password: string): Promise<SignInResult> {
  advanceAuthGeneration();
  const pool = getPool();
  if (!pool) return Promise.reject(new Error("Cognito auth is not configured"));

  const user = new CognitoUser({ Username: username, Pool: pool });
  const details = new AuthenticationDetails({ Username: username, Password: password });

  return new Promise<SignInResult>((resolve, reject) => {
    user.authenticateUser(details, buildAuthCallbacks(user, resolve, reject, getAuthGeneration()));
  });
}

function buildAuthCallbacks(
  user: CognitoUser,
  resolve: (result: SignInResult) => void,
  reject: (err: Error) => void,
  generation: number,
) {
  const active = () => generation === getAuthGeneration();
  return {
    onSuccess: () => active() ? resolveAuthenticated(resolve, reject) : reject(new AuthenticationRequiredError()),
    onFailure: (err: Error) => reject(err),
    newPasswordRequired: () => reject(new Error("new password required")),
    // Enrolled user: prompt for the current authenticator code.
    totpRequired: () => {
      if (!active()) { reject(new AuthenticationRequiredError()); return; }
      resolve({
        kind: "challenge",
        challenge: {
          kind: "softwareTokenMfa",
          submitCode: (code: string) =>
            new Promise<SignInResult>((res, rej) => {
              if (!active()) { rej(new AuthenticationRequiredError()); return; }
              user.sendMFACode(
                code.trim(),
                buildAuthCallbacks(user, res, rej, generation),
                "SOFTWARE_TOKEN_MFA",
              );
            }),
        },
      });
    },
    // Not enrolled. Enrollment is centralized in ahara-business, so we do not run
    // associateSoftwareToken/verifySoftwareToken here — direct the user there.
    mfaSetup: () => reject(new Error(ENROLL_VIA_AHARA_BUSINESS)),
  };
}

function resolveAuthenticated(
  resolve: (result: SignInResult) => void,
  reject: (err: Error) => void,
): void {
  getSessionSnapshot()
    .then((snapshot) => {
      if (!snapshot) throw new Error("session missing after login");
      resolve({ kind: "authenticated", session: snapshot });
    })
    .catch((err) => reject(err instanceof Error ? err : new Error(String(err))));
}

function readString(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 ? value : null;
}
