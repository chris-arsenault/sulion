import { create } from "zustand";

import {
  listSecretGrants,
  listSecrets,
  revokeSecretGrant,
  unlockSecretGrant,
} from "../api/client";
import type { SecretGrantMetadata, SecretMetadata, SecretTool } from "../api/types";

interface SecretStore {
  secrets: SecretMetadata[];
  grantsBySession: Record<string, SecretGrantMetadata[]>;
  refreshSecrets: () => Promise<void>;
  refreshGrants: (sessionId: string) => Promise<void>;
  enableGrant: (
    sessionId: string,
    secretId: string,
    tool: SecretTool,
    ttlSeconds: number,
  ) => Promise<void>;
  revokeGrant: (
    sessionId: string,
    secretId: string,
    tool: SecretTool,
  ) => Promise<void>;
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

  enableGrant: async (sessionId, secretId, tool, ttlSeconds) => {
    await unlockSecretGrant({
      pty_session_id: sessionId,
      secret_id: secretId,
      tool,
      ttl_seconds: ttlSeconds,
    });
    await get().refreshGrants(sessionId);
  },

  revokeGrant: async (sessionId, secretId, tool) => {
    await revokeSecretGrant({
      pty_session_id: sessionId,
      secret_id: secretId,
      tool,
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
