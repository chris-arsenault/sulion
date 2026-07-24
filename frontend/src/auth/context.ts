import { createContext, useContext } from "react";

import type {
  SessionSnapshot,
  SignInChallenge,
} from "./cognito";

export type AuthState =
  | { kind: "loading" }
  | { kind: "disabled" }
  | { kind: "anonymous"; error: string | null }
  | {
      kind: "mfa";
      challenge: SignInChallenge;
      error: string | null;
      pending: boolean;
    }
  | { kind: "authenticated"; session: SessionSnapshot };

export interface AuthContextValue {
  state: AuthState;
  login: (username: string, password: string) => Promise<void>;
  submitMfaCode: (code: string) => Promise<void>;
  cancelMfa: () => void;
  logout: () => void;
}

export const AuthContext = createContext<AuthContextValue | null>(null);

export function useAuth(): AuthContextValue {
  const value = useContext(AuthContext);
  if (!value) throw new Error("useAuth must be used inside AuthProvider");
  return value;
}
