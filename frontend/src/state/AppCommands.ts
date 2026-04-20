import { useEffect, useRef } from "react";

import type { LibraryKind } from "../api/types";

export type AppCommand =
  | { type: "open-file"; repo: string; path: string }
  | { type: "open-diff"; repo: string; path?: string }
  | { type: "reveal-file"; repo: string; path: string }
  | { type: "reveal-repo"; repo: string }
  | { type: "close-drawer" }
  | { type: "inject-terminal"; sessionId: string; text: string }
  | { type: "library-changed"; kind: LibraryKind };

type AppCommandType = AppCommand["type"];
type AppCommandOf<T extends AppCommandType> = Extract<AppCommand, { type: T }>;
type AppCommandListener = (command: AppCommand) => void;

const listeners = new Set<AppCommandListener>();

function dispatchAppCommand(command: AppCommand) {
  for (const listener of Array.from(listeners)) {
    listener(command);
  }
}

export const appCommands = {
  openFile(detail: Omit<AppCommandOf<"open-file">, "type">) {
    dispatchAppCommand({ type: "open-file", ...detail });
  },

  openDiff(detail: Omit<AppCommandOf<"open-diff">, "type">) {
    dispatchAppCommand({ type: "open-diff", ...detail });
  },

  revealFile(detail: Omit<AppCommandOf<"reveal-file">, "type">) {
    dispatchAppCommand({ type: "reveal-file", ...detail });
  },

  revealRepo(detail: Omit<AppCommandOf<"reveal-repo">, "type">) {
    dispatchAppCommand({ type: "reveal-repo", ...detail });
  },

  closeDrawer() {
    dispatchAppCommand({ type: "close-drawer" });
  },

  injectTerminal(detail: Omit<AppCommandOf<"inject-terminal">, "type">) {
    dispatchAppCommand({ type: "inject-terminal", ...detail });
  },

  libraryChanged(detail: Omit<AppCommandOf<"library-changed">, "type">) {
    dispatchAppCommand({ type: "library-changed", ...detail });
  },
};

export function subscribeToAppCommands(listener: AppCommandListener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function subscribeToAppCommand<T extends AppCommandType>(
  type: T,
  listener: (command: AppCommandOf<T>) => void,
): () => void {
  return subscribeToAppCommands((command) => {
    if (command.type !== type) return;
    listener(command as AppCommandOf<T>);
  });
}

export function useAppCommand<T extends AppCommandType>(
  type: T,
  listener: (command: AppCommandOf<T>) => void,
) {
  const listenerRef = useRef(listener);
  listenerRef.current = listener;

  useEffect(() => {
    return subscribeToAppCommand(type, (command) => listenerRef.current(command));
  }, [type]);
}

export function resetAppCommands() {
  listeners.clear();
}
