// Context-menu primitive store + trigger helpers. Kept out of the
// component file so fast-refresh sees only component exports there.
// Surfaces attach trigger props or handlers returned from this module
// and let the single <ContextMenuHost> render the menu in a portal.

import type { ReactNode } from "react";
import { create } from "zustand";

export type MenuItem =
  | {
      kind: "item";
      id?: string;
      label: string;
      icon?: ReactNode;
      keyHint?: string;
      disabled?: boolean;
      destructive?: boolean;
      onSelect: () => void;
    }
  | {
      kind: "submenu";
      id?: string;
      label: string;
      icon?: ReactNode;
      items: MenuItem[];
      disabled?: boolean;
    }
  | { kind: "separator" }
  | { kind: "header"; label: string };

export interface OpenAt {
  clientX: number;
  clientY: number;
}

export interface MenuState {
  anchor: OpenAt;
  items: MenuItem[];
}

export interface ContextMenuHandle {
  open: (at: OpenAt, items: MenuItem[]) => void;
  close: () => void;
}

interface ContextMenuStoreShape extends ContextMenuHandle {
  state: MenuState | null;
}

const initialState = { state: null as MenuState | null };

export const useContextMenuStore = create<ContextMenuStoreShape>()((set) => ({
  ...initialState,
  open: (at, items) => set({ state: { anchor: at, items } }),
  close: () => set({ state: null }),
}));

export function useContextMenu<T>(
  selector: (state: ContextMenuStoreShape) => T,
): T {
  return useContextMenuStore(selector);
}

export function resetContextMenuStore() {
  useContextMenuStore.setState(initialState);
}

/** Convenience: returns an onContextMenu handler that opens the menu
 * unless the user held Shift (in which case the native browser menu
 * appears). Callers that need access to the event itself (e.g. for a
 * row-local action) should use the hook directly. */
export function contextMenuHandler(
  open: ContextMenuHandle["open"],
  build: (e: React.MouseEvent) => MenuItem[] | null,
): (e: React.MouseEvent) => void {
  return (e) => {
    if (e.shiftKey) return;
    const items = build(e);
    if (!items || items.length === 0) return;
    e.preventDefault();
    e.stopPropagation();
    open({ clientX: e.clientX, clientY: e.clientY }, items);
  };
}

/** Returns a11y-complete trigger props for a region whose only purpose
 * is to open a context menu. Spread on any element:
 *
 *   <div {...contextMenuTriggerProps(openCtx, () => [...items])} />
 *
 * Provides mouse (right-click) + keyboard (ContextMenu key / Shift+F10)
 * paths to the same menu, plus role and tabIndex so the element is
 * focusable and the a11y tree reads it as an actionable button. */
export function contextMenuTriggerProps(
  open: ContextMenuHandle["open"],
  build: () => MenuItem[] | null,
): {
  onContextMenu: (e: React.MouseEvent) => void;
  onKeyDown: (e: React.KeyboardEvent) => void;
  role: "button";
  tabIndex: 0;
} {
  const openAt = (clientX: number, clientY: number) => {
    const items = build();
    if (!items || items.length === 0) return false;
    open({ clientX, clientY }, items);
    return true;
  };
  return {
    role: "button",
    tabIndex: 0,
    onContextMenu: (e) => {
      if (e.shiftKey) return;
      if (openAt(e.clientX, e.clientY)) {
        e.preventDefault();
        e.stopPropagation();
      }
    },
    onKeyDown: (e) => {
      const isContextKey =
        e.key === "ContextMenu" || (e.shiftKey && e.key === "F10");
      if (!isContextKey) return;
      const target = e.currentTarget as HTMLElement;
      const rect = target.getBoundingClientRect();
      if (openAt(rect.left + 8, rect.bottom)) {
        e.preventDefault();
        e.stopPropagation();
      }
    },
  };
}
