import { create } from "zustand";

import {
  listSecretGrants,
  listSecrets,
  revokeSecretGrant,
  unlockSecretGrant,
} from "../api/client";
import type { SecretGrantMetadata, SecretMetadata } from "../api/types";

interface SecretStore {
  secrets: SecretMetadata[];
  grantsBySession: Record<string, SecretGrantMetadata[]>;
  refreshSecrets: () => Promise<void>;
  refreshGrants: (sessionId: string) => Promise<void>;
  enableGrant: (
    sessionId: string,
    secretId: string,
    ttlSeconds: number | null,
  ) => Promise<void>;
  revokeGrant: (sessionId: string, secretId: string, repository?: boolean) => Promise<void>;
  setAllTerminals: (secretId: string, programs: string[] | null) => Promise<void>;
}

export const useSecretStore = create<SecretStore>()((set, get) => ({
  secrets: [],
  grantsBySession: {},

  refreshSecrets: async () => {
    const secrets = await listSecrets();
    set({ secrets });
  },

  refreshGrants: async (sessionId) => {
    const grants = await listSecretGrants(sessionId);
    set((state) => ({
      grantsBySession: { ...state.grantsBySession, [sessionId]: grants },
    }));
  },

  enableGrant: async (sessionId, secretId, ttlSeconds) => {
    await unlockSecretGrant({
      pty_session_id: sessionId,
      secret_id: secretId,
      ...(ttlSeconds == null ? { scope: "repository" as const } : { ttl_seconds: ttlSeconds }),
    });
    await Promise.all((ttlSeconds == null
      ? [...new Set([...Object.keys(get().grantsBySession), sessionId])]
      : [sessionId]).map((id) => get().refreshGrants(id)));
  },

  revokeGrant: async (sessionId, secretId, repository) => {
    await revokeSecretGrant({
      pty_session_id: sessionId,
      secret_id: secretId,
      ...(repository ? { scope: "repository" as const } : {}),
    });
    await Promise.all((repository
      ? [...new Set([...Object.keys(get().grantsBySession), sessionId])]
      : [sessionId]).map((id) => get().refreshGrants(id)));
  },

  setAllTerminals: async (secretId, programs) => {
    const scope = "all_terminals" as const;
    await (programs
      ? unlockSecretGrant({ secret_id: secretId, scope, programs })
      : revokeSecretGrant({ secret_id: secretId, scope }));
    await Promise.all([
      get().refreshSecrets(),
      ...Object.keys(get().grantsBySession).map((id) => get().refreshGrants(id)),
    ]);
  },
}));

export function resetSecretStore() {
  useSecretStore.setState({
    secrets: [],
    grantsBySession: {},
  });
}
