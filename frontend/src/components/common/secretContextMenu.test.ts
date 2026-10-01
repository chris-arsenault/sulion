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
  it("builds enable leaves as secret -> ttl", () => {
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
    expect(secret.items.map((item) => item.kind === "item" ? item.label : "")).toEqual([
      "10m",
      "30m",
      "1h",
      "4h",
      "Always for this repository",
    ]);
  });

  it("passes the selected secret and ttl to enable", () => {
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
    const tenMinutes = secret.items[0];
    expect(tenMinutes?.kind).toBe("item");
    if (tenMinutes?.kind === "item") tenMinutes.onSelect();

    expect(onEnable).toHaveBeenCalledWith(SECRET_ID, 600);
  });

  it("shows active grants as immediate revoke actions", () => {
    const onRevoke = vi.fn();
    const grant: SecretGrantMetadata = {
      secret_id: SECRET_ID,
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

    expect(onRevoke).toHaveBeenCalledWith(SECRET_ID);
  });

  it("enables repository access and keeps its revoke distinct from a terminal grant", () => {
    const onEnable = vi.fn();
    const onRevoke = vi.fn();
    const menu = rootMenu(buildSecretContextMenu({
      secrets: [SECRET],
      grants: [
        { secret_id: SECRET_ID, granted_by_sub: "user", granted_by_username: null,
          expires_at: null, repo: "atlas" },
        { secret_id: SECRET_ID, granted_by_sub: "user", granted_by_username: null,
          expires_at: new Date(Date.now() + 600_000).toISOString() },
      ],
      onEnable, onRevoke, onOpenManager: vi.fn(),
    }));
    const leaves = submenu(submenu(menu.items, "Enable secret").items, SECRET_ID).items;
    const permanent = leaves.find((item) => item.kind === "item" && item.label === "Always for this repository");
    if (permanent?.kind !== "item") throw new Error("missing permanent enable");
    permanent.onSelect();
    expect(onEnable).toHaveBeenCalledWith(SECRET_ID, null);
    const active = submenu(menu.items, "Active secrets").items;
    expect(active).toHaveLength(2);
    const repositoryGrant = active[0];
    if (repositoryGrant.kind !== "item") throw new Error("missing repository grant");
    expect(repositoryGrant.label).toContain("always for atlas");
    repositoryGrant.onSelect();
    expect(onRevoke).toHaveBeenCalledWith(SECRET_ID, true);
  });

  it("disables enablement when active bundles would conflict", () => {
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
    const openai = submenu(enable.items, "openai-api · conflicts with claude-api");

    expect(openai.disabled).toBe(true);
  });

  it("lets an explicit grant supersede an every-terminal grant", () => {
    const onRevoke = vi.fn();
    const onOpenManager = vi.fn();
    const ghKeys = { scope: "global", repo: null, env_keys: ["GH_TOKEN"], updated_at: "2026-09-30T00:00:00Z" };
    const menu = rootMenu(buildSecretContextMenu({
      secrets: [
        { id: "gh-read", description: "read", all_terminal_programs: ["gh"], ...ghKeys },
        { id: "gh-write", description: "write", ...ghKeys },
      ],
      grants: [
        { secret_id: "gh-read", granted_by_sub: "user", granted_by_username: null,
          expires_at: null, repo: null, scope: "all_terminals", programs: ["gh"] },
      ],
      onEnable: vi.fn(), onRevoke, onOpenManager,
    }));

    const enable = menu.items.find((item) => item.kind === "submenu" && item.id === "enable-secret");
    if (enable?.kind !== "submenu") throw new Error("missing enable submenu");
    const write = submenu(enable.items, "gh-write");
    expect(write.disabled).toBe(false);

    const [everyTerminal] = submenu(menu.items, "Active secrets").items;
    if (everyTerminal?.kind !== "item") throw new Error("missing every-terminal grant");
    expect(everyTerminal.label).toBe("gh-read · every terminal for gh · manage");
    everyTerminal.onSelect();
    expect(onOpenManager).toHaveBeenCalled();
    expect(onRevoke).not.toHaveBeenCalled();
  });
});
