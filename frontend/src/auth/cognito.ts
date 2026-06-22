import {
  AuthenticationDetails,
  CognitoUser,
  CognitoUserPool,
  type CognitoUserSession,
} from "amazon-cognito-identity-js";

import { getAuthRuntimeConfig } from "./config";

export type SessionSnapshot = {
  accessToken: string;
  email: string | null;
  username: string | null;
};

let cachedPool: CognitoUserPool | null = null;
let cachedPoolKey: string | null = null;

function getPool(): CognitoUserPool | null {
  const config = getAuthRuntimeConfig();
  if (!config) return null;
  const key = `${config.cognitoUserPoolId}:${config.cognitoClientId}`;
  if (!cachedPool || cachedPoolKey !== key) {
    cachedPool = new CognitoUserPool({
      UserPoolId: config.cognitoUserPoolId,
      ClientId: config.cognitoClientId,
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

export function getCurrentSession(): Promise<CognitoUserSession | null> {
  const user = getCurrentUser();
  if (!user) return Promise.resolve(null);
  return new Promise((resolve, reject) => {
    user.getSession((err: Error | null, session: CognitoUserSession | null) => {
      if (err) {
        reject(err);
        return;
      }
      if (!session?.isValid()) {
        resolve(null);
        return;
      }
      resolve(session);
    });
  });
}

export async function getAccessToken(): Promise<string | null> {
  if (import.meta.env.VITE_SULION_E2E === "1") {
    const token = import.meta.env.VITE_SULION_E2E_ACCESS_TOKEN;
    if (typeof token === "string" && token.length > 0) return token;
  }
  const session = await getCurrentSession();
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
  const pool = getPool();
  if (!pool) return Promise.reject(new Error("Cognito auth is not configured"));

  const user = new CognitoUser({ Username: username, Pool: pool });
  const details = new AuthenticationDetails({ Username: username, Password: password });

  return new Promise<SignInResult>((resolve, reject) => {
    user.authenticateUser(details, buildAuthCallbacks(user, resolve, reject));
  });
}

function buildAuthCallbacks(
  user: CognitoUser,
  resolve: (result: SignInResult) => void,
  reject: (err: Error) => void,
) {
  return {
    onSuccess: () => resolveAuthenticated(resolve, reject),
    onFailure: (err: Error) => reject(err),
    newPasswordRequired: () => reject(new Error("new password required")),
    // Enrolled user: prompt for the current authenticator code.
    totpRequired: () => {
      resolve({
        kind: "challenge",
        challenge: {
          kind: "softwareTokenMfa",
          submitCode: (code: string) =>
            new Promise<SignInResult>((res, rej) => {
              user.sendMFACode(
                code.trim(),
                buildAuthCallbacks(user, res, rej),
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
