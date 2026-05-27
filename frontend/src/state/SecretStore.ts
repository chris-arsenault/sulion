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
    ttlSeconds: number,
  ) => Promise<void>;
  revokeGrant: (sessionId: string, secretId: string) => Promise<void>;
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
      ttl_seconds: ttlSeconds,
    });
    await get().refreshGrants(sessionId);
  },

  revokeGrant: async (sessionId, secretId) => {
    await revokeSecretGrant({
      pty_session_id: sessionId,
      secret_id: secretId,
    });
    await get().refreshGrants(sessionId);
  },
}));

export function resetSecretStore() {
  useSecretStore.setState({
    secrets: [],
    grantsBySession: {},
  });
}
