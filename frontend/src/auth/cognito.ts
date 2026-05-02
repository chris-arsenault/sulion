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

export function signIn(username: string, password: string): Promise<SessionSnapshot> {
  const pool = getPool();
  if (!pool) return Promise.reject(new Error("Cognito auth is not configured"));

  const user = new CognitoUser({
    Username: username,
    Pool: pool,
  });
  const details = new AuthenticationDetails({
    Username: username,
    Password: password,
  });

  return new Promise((resolve, reject) => {
    user.authenticateUser(details, {
      onSuccess: async () => {
        try {
          const snapshot = await getSessionSnapshot();
          if (!snapshot) throw new Error("session missing after login");
          resolve(snapshot);
        } catch (err) {
          reject(err);
        }
      },
      onFailure: (err: Error) => reject(err),
      newPasswordRequired: () => reject(new Error("new password required")),
    });
  });
}

function readString(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 ? value : null;
}
