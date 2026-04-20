import { appCommands } from "../../state/AppCommands";
import { copyToClipboard } from "../terminal/clipboard";
import type { MenuItem } from "./contextMenuStore";

export function buildWorkspaceFileMenuItems({
  repo,
  path,
  dirty,
  copyText,
}: {
  repo: string;
  path: string;
  dirty?: string | null;
  copyText?: string;
}): MenuItem[] {
  const items: MenuItem[] = [
    {
      kind: "item",
      id: "reveal-file",
      label: "Reveal in file tree",
      onSelect: () => appCommands.revealFile({ repo, path }),
    },
    {
      kind: "item",
      id: "open-file",
      label: "Open file",
      onSelect: () => appCommands.openFile({ repo, path }),
    },
  ];

  if (dirty) {
    items.push({
      kind: "item",
      id: "open-diff",
      label: "Open diff",
      onSelect: () => appCommands.openDiff({ repo, path }),
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
