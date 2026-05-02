import { describe, expect, it, vi } from "vitest";

import type { SecretGrantMetadata, SecretMetadata } from "../../api/types";
import type { MenuItem } from "./contextMenuStore";
import { buildSecretContextMenu } from "./secretContextMenu";

const SECRET_ID = "claude-api";

const SECRET: SecretMetadata = {
  id: SECRET_ID,
  description: "Claude",
  scope: "global",
  repo: null,
  env_keys: ["ANTHROPIC_API_KEY"],
  updated_at: "2026-04-24T00:00:00Z",
};

function submenu(items: MenuItem[], label: string) {
  const item = items.find((candidate) =>
    candidate.kind === "submenu" && candidate.label === label,
  );
  if (!item || item.kind !== "submenu") {
    throw new Error(`submenu ${label} not found`);
  }
  return item as Extract<MenuItem, { kind: "submenu" }>;
}

function rootMenu(item: MenuItem) {
  if (item.kind !== "submenu") {
    throw new Error("root menu is not a submenu");
  }
  return item;
}

describe("buildSecretContextMenu", () => {
  it("builds enable leaves as secret -> tool -> ttl", () => {
    const menu = buildSecretContextMenu({
      secrets: [SECRET],
      grants: [],
      onEnable: vi.fn(),
      onRevoke: vi.fn(),
      onOpenManager: vi.fn(),
    });

    const root = rootMenu(menu);
    const enable = submenu(root.items, "Enable secret");
    const secret = submenu(enable.items, SECRET_ID);
    const withCred = submenu(secret.items, "with-cred");
    expect(withCred.items.map((item) => item.kind === "item" ? item.label : "")).toEqual([
      "10m",
      "30m",
      "1h",
      "4h",
    ]);
  });

  it("passes the selected secret, tool, and ttl to enable", () => {
    const onEnable = vi.fn();
    const menu = buildSecretContextMenu({
      secrets: [SECRET],
      grants: [],
      onEnable,
      onRevoke: vi.fn(),
      onOpenManager: vi.fn(),
    });

    const root = rootMenu(menu);
    const enable = submenu(root.items, "Enable secret");
    const secret = submenu(enable.items, SECRET_ID);
    const aws = submenu(secret.items, "aws");
    const tenMinutes = aws.items[0];
    expect(tenMinutes?.kind).toBe("item");
    if (tenMinutes?.kind === "item") tenMinutes.onSelect();

    expect(onEnable).toHaveBeenCalledWith(SECRET_ID, "aws", 600);
  });

  it("shows active grants as immediate revoke actions", () => {
    const onRevoke = vi.fn();
    const grant: SecretGrantMetadata = {
      secret_id: SECRET_ID,
      tool: "with-cred",
      granted_by_sub: "user",
      granted_by_username: null,
      expires_at: new Date(Date.now() + 30 * 60_000).toISOString(),
    };
    const menu = buildSecretContextMenu({
      secrets: [SECRET],
      grants: [grant],
      onEnable: vi.fn(),
      onRevoke,
      onOpenManager: vi.fn(),
    });

    const root = rootMenu(menu);
    const active = submenu(root.items, "Active secrets");
    const grantItem = active.items[0];
    expect(grantItem?.kind).toBe("item");
    if (grantItem?.kind === "item") grantItem.onSelect();

    expect(onRevoke).toHaveBeenCalledWith(SECRET_ID, "with-cred");
  });

  it("disables with-cred enablement when active bundles would conflict", () => {
    const menu = buildSecretContextMenu({
      secrets: [
        SECRET,
        {
          id: "openai-api",
          description: "OpenAI",
          scope: "global",
          repo: null,
          env_keys: ["ANTHROPIC_API_KEY", "OPENAI_API_KEY"],
          updated_at: "2026-04-24T00:00:00Z",
        },
      ],
      grants: [
        {
          secret_id: SECRET_ID,
          tool: "with-cred",
          granted_by_sub: "user",
          granted_by_username: null,
          expires_at: new Date(Date.now() + 30 * 60_000).toISOString(),
        },
      ],
      onEnable: vi.fn(),
      onRevoke: vi.fn(),
      onOpenManager: vi.fn(),
    });

    const root = rootMenu(menu);
    const enable = submenu(root.items, "Enable secret");
    const openai = submenu(enable.items, "openai-api");
    const withCred = submenu(openai.items, "with-cred · conflicts with claude-api");
    const aws = submenu(openai.items, "aws");

    expect(withCred.disabled).toBe(true);
    expect(aws.disabled).toBeFalsy();
  });
});
