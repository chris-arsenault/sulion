import type { SecretGrantMetadata, SecretMetadata } from "../../api/types";
import type { MenuItem } from "./contextMenuStore";

const TTL_PRESETS = [
  { label: "10m", seconds: 600 },
  { label: "30m", seconds: 1800 },
  { label: "1h", seconds: 3600 },
  { label: "4h", seconds: 14_400 },
] as const;

export function buildSecretContextMenu({
  secrets,
  grants,
  onEnable,
  onRevoke,
  onOpenManager,
}: {
  secrets: SecretMetadata[];
  grants: SecretGrantMetadata[];
  onEnable: (secretId: string, ttlSeconds: number) => void;
  onRevoke: (secretId: string) => void;
  onOpenManager: () => void;
}): MenuItem {
  const activeItems = grants.map((grant) => ({
    kind: "item" as const,
    id: `revoke-${grant.secret_id}`,
    label: `${grant.secret_id} · ${relativeExpiry(grant.expires_at)}`,
    destructive: true,
    onSelect: () => onRevoke(grant.secret_id),
  }));

  return {
    kind: "submenu",
    id: "secrets",
    label: "Secrets",
    items: [
      {
        kind: "submenu",
        id: "enable-secret",
        label: "Enable secret",
        disabled: secrets.length === 0,
        items:
          secrets.length === 0
            ? [
                {
                  kind: "item",
                  label: "No secrets configured",
                  disabled: true,
                  onSelect: () => {},
                },
              ]
            : secrets.map((secret) => {
                const conflict = activeGrantConflict(secret, secrets, grants);
                return {
                  kind: "submenu" as const,
                  id: `secret-${secret.id}`,
                  label: conflict
                    ? `${secret.id} · conflicts with ${conflict}`
                    : secret.id,
                  disabled: conflict != null,
                  items: buildEnableLeaves(secret, onEnable),
                };
              }),
      },
      {
        kind: "submenu",
        id: "active-secrets",
        label: "Active secrets",
        disabled: activeItems.length === 0,
        items:
          activeItems.length > 0
            ? activeItems
            : [
                {
                  kind: "item",
                  label: "No active secrets",
                  disabled: true,
                  onSelect: () => {},
                },
              ],
      },
      { kind: "separator" },
      {
        kind: "item",
        id: "manage-secrets",
        label: "Manage secrets",
        onSelect: onOpenManager,
      },
    ],
  };
}

function buildEnableLeaves(
  secret: SecretMetadata,
  onEnable: (secretId: string, ttlSeconds: number) => void,
): MenuItem[] {
  return TTL_PRESETS.map((preset) => ({
    kind: "item" as const,
    id: `enable-${secret.id}-${preset.seconds}`,
    label: preset.label,
    onSelect: () => onEnable(secret.id, preset.seconds),
  }));
}

function activeGrantConflict(
  secret: SecretMetadata,
  secrets: SecretMetadata[],
  grants: SecretGrantMetadata[],
): string | null {
  const currentKeys = new Set(secret.env_keys);
  for (const grant of grants) {
    if (grant.secret_id === secret.id) continue;
    const grantedSecret = secrets.find((item) => item.id === grant.secret_id);
    const overlap = (grantedSecret?.env_keys ?? []).filter((key) =>
      currentKeys.has(key),
    );
    if (overlap.length > 0) return grant.secret_id;
  }
  return null;
}

function relativeExpiry(value: string) {
  const ms = new Date(value).getTime() - Date.now();
  if (ms <= 0) return "expired";
  const minutes = Math.round(ms / 60_000);
  if (minutes < 60) return `${minutes}m left`;
  const hours = Math.round(minutes / 60);
  return `${hours}h left`;
}
