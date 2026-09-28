import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { resetSecretStore, useSecretStore } from "./SecretStore";

const writes: Array<{ method: string; body: unknown }> = [];
const reads: string[] = [];

beforeEach(() => {
  writes.length = 0;
  reads.length = 0;
  resetSecretStore();
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    if (init?.method) {
      writes.push({ method: init.method, body: JSON.parse(String(init.body)) });
      return new Response(null, { status: 204 });
    }
    reads.push(new URL(url, "http://localhost").searchParams.get("pty_session_id")!);
    return new Response("[]", { status: 200 });
  }));
});

afterEach(() => vi.unstubAllGlobals());

describe("repository secret grants", () => {
  it("sends an explicit repository scope without a TTL and refreshes other sessions", async () => {
    useSecretStore.setState({ grantsBySession: { first: [], second: [] } });
    await useSecretStore.getState().enableGrant("first", "status", null);
    expect(writes[0]).toEqual({ method: "POST", body: {
      pty_session_id: "first", secret_id: "status", scope: "repository",
    } });
    expect(reads).toEqual(["first", "second"]);
  });

  it("revokes repository access explicitly and refreshes every observed session", async () => {
    useSecretStore.setState({ grantsBySession: { first: [], second: [] } });
    await useSecretStore.getState().revokeGrant("first", "status", true);
    expect(writes[0]).toEqual({ method: "DELETE", body: {
      pty_session_id: "first", secret_id: "status", scope: "repository",
    } });
    expect(reads).toEqual(["first", "second"]);
  });

  it("preserves terminal grant requests and leaves other sessions alone", async () => {
    useSecretStore.setState({ grantsBySession: { first: [], second: [] } });
    await useSecretStore.getState().enableGrant("first", "status", 600);
    expect(writes[0]).toEqual({ method: "POST", body: {
      pty_session_id: "first", secret_id: "status", ttl_seconds: 600,
    } });
    expect(reads).toEqual(["first"]);
    await useSecretStore.getState().revokeGrant("first", "status");
    expect(writes[1]).toEqual({ method: "DELETE", body: { pty_session_id: "first", secret_id: "status" } });
  });
});
