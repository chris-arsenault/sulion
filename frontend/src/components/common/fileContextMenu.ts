import { appCommands } from "../../state/AppCommands";
import { copyToClipboard } from "../terminal/clipboard";
import type { MenuItem } from "./contextMenuStore";

export function buildWorkspaceFileMenuItems({
  repo,
  path,
  dirty,
  workspaceId,
  copyText,
}: {
  repo: string;
  path: string;
  dirty?: string | null;
  workspaceId?: string;
  copyText?: string;
}): MenuItem[] {
  const items: MenuItem[] = [
    {
      kind: "item",
      id: "reveal-file",
      label: "Reveal in file tree",
      disabled: Boolean(workspaceId),
      onSelect: () => appCommands.revealFile({ repo, path }),
    },
    {
      kind: "item",
      id: "open-file",
      label: "Open file",
      onSelect: () => appCommands.openFile({ repo, path, workspaceId }),
    },
  ];

  if (dirty) {
    items.push({
      kind: "item",
      id: "open-diff",
      label: "Open diff",
      onSelect: () => appCommands.openDiff({ repo, path, workspaceId }),
    });
  }

  items.push({ kind: "separator" });
  items.push({
    kind: "item",
    id: "copy-path",
    label: "Copy path",
    onSelect: () => {
      void copyToClipboard(copyText ?? path);
    },
  });

  return items;
}
