// Context-menu primitive. Surfaces attach:
//
//   onContextMenu={(e) => {
//     if (e.shiftKey) return;          // pass to browser's native menu
//     e.preventDefault();
//     open(e, items);
//   }}
//
// A single <ContextMenuHost> at the app root renders the menu in a
// portal. Submenus open one level deep, flip horizontally/vertically
// to stay in the viewport, and are keyboard-navigable (arrows / Enter
// / Esc / ←→ for submenu).

import {
  type ReactNode,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import { create } from "zustand";

import "./ContextMenu.css";

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

interface OpenAt {
  clientX: number;
  clientY: number;
}

interface MenuState {
  anchor: OpenAt;
  items: MenuItem[];
}

interface ContextMenuHandle {
  open: (at: OpenAt, items: MenuItem[]) => void;
  close: () => void;
}

interface ContextMenuStore extends ContextMenuHandle {
  state: MenuState | null;
}

const initialState = { state: null as MenuState | null };

export const useContextMenuStore = create<ContextMenuStore>()((set) => ({
  ...initialState,
  open: (at, items) => set({ state: { anchor: at, items } }),
  close: () => set({ state: null }),
}));

export function useContextMenu<T>(selector: (state: ContextMenuStore) => T): T {
  return useContextMenuStore(selector);
}

export function ContextMenuHost() {
  const state = useContextMenu((store) => store.state);
  const close = useContextMenu((store) => store.close);
  if (!state) return null;
  return createPortal(<MenuRoot state={state} onClose={close} />, document.body);
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

export function resetContextMenuStore() {
  useContextMenuStore.setState(initialState);
}

// ─── Menu rendering ────────────────────────────────────────────────

function MenuRoot({
  state,
  onClose,
}: {
  state: MenuState;
  onClose: () => void;
}) {
  // Close on Escape, outside-click, or window blur. We catch the click
  // at the capture phase on the document so items can still fire their
  // own onSelect (they stopPropagation).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      }
    };
    const onBlur = () => onClose();
    window.addEventListener("keydown", onKey);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("blur", onBlur);
    };
  }, [onClose]);

  return (
    <div
      className="ctxm-scrim"
      onMouseDown={onClose}
      onContextMenu={(e) => {
        // Right-click on the scrim closes the menu rather than opening
        // another one or leaking to the surface below.
        e.preventDefault();
        onClose();
      }}
    >
      <Menu
        items={state.items}
        anchor={state.anchor}
        onClose={onClose}
        level={0}
      />
    </div>
  );
}

interface Anchor {
  x: number;
  y: number;
  /** When this menu is a submenu, `from` is the bounding rect of the
   * parent item so we can align vertically and flip horizontally. */
  from?: DOMRect;
}

function Menu({
  items,
  anchor,
  onClose,
  level,
}: {
  items: MenuItem[];
  anchor: OpenAt | Anchor;
  onClose: () => void;
  level: number;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number }>(() => ({
    left: "clientX" in anchor ? anchor.clientX : anchor.x,
    top: "clientY" in anchor ? anchor.clientY : anchor.y,
  }));
  const [focusIdx, setFocusIdx] = useState<number>(firstFocusable(items));
  const [openSub, setOpenSub] = useState<number | null>(null);
  const submenuTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Reflow after mount so we can clamp to the viewport using real
  // measurements. Runs once the node lands.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const vw = window.innerWidth;
    const vh = window.innerHeight;

    let left: number;
    let top: number;
    if ("from" in anchor && anchor.from) {
      left = anchor.from.right;
      top = anchor.from.top;
      if (left + rect.width > vw) {
        left = Math.max(0, anchor.from.left - rect.width);
      }
    } else {
      left = "clientX" in anchor ? anchor.clientX : anchor.x;
      top = "clientY" in anchor ? anchor.clientY : anchor.y;
    }
    if (left + rect.width > vw) left = Math.max(0, vw - rect.width - 4);
    if (top + rect.height > vh) top = Math.max(0, vh - rect.height - 4);
    setPos({ left, top });
  }, [anchor]);

  // Keyboard navigation on the top-level menu only (submenus listen
  // independently via their own Menu instance).
  const isTop = level === 0;
  useEffect(() => {
    if (!isTop) return;
    const el = ref.current;
    el?.focus();
  }, [isTop]);

  const focusableIdxs = useMemo(() => focusablePositions(items), [items]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setFocusIdx((cur) => nextFocusable(focusableIdxs, cur, +1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setFocusIdx((cur) => nextFocusable(focusableIdxs, cur, -1));
    } else if (e.key === "ArrowRight") {
      e.preventDefault();
      const item = items[focusIdx];
      if (item?.kind === "submenu") setOpenSub(focusIdx);
    } else if (e.key === "ArrowLeft") {
      e.preventDefault();
      if (level > 0) onClose();
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      const item = items[focusIdx];
      if (!item) return;
      if (item.kind === "item" && !item.disabled) {
        item.onSelect();
        bubbleTopClose(onClose);
      } else if (item.kind === "submenu") {
        setOpenSub(focusIdx);
      }
    }
  };

  // Closing the top-level menu should close everything; submenus call
  // their parent's onClose (we wire that via the prop).
  const bubbleTopClose = (close: () => void) => {
    close();
  };

  return (
    <div
      ref={ref}
      className="ctxm"
      role="menu"
      tabIndex={-1}
      // eslint-disable-next-line local/no-inline-styles -- cursor-anchored positioning, not representable as CSS classes
      style={{ left: pos.left, top: pos.top }}
      onMouseDown={(e) => e.stopPropagation()}
      onKeyDown={onKeyDown}
    >
      {items.map((item, i) => renderItem(item, i, {
        focused: i === focusIdx,
        openSub: openSub === i,
        onHover: () => {
          setFocusIdx(i);
          if (item.kind === "submenu") {
            if (submenuTimer.current) clearTimeout(submenuTimer.current);
            submenuTimer.current = setTimeout(() => setOpenSub(i), 120);
          } else {
            if (submenuTimer.current) clearTimeout(submenuTimer.current);
            setOpenSub(null);
          }
        },
        onActivate: () => {
          if (item.kind === "item" && !item.disabled) {
            item.onSelect();
            onClose();
          } else if (item.kind === "submenu" && !item.disabled) {
            setOpenSub(i);
          }
        },
        onCloseSub: () => setOpenSub(null),
        onClose,
        submenuAnchor: (rect: DOMRect) => rect,
      }))}
    </div>
  );
}

function renderItem(
  item: MenuItem,
  i: number,
  ctx: {
    focused: boolean;
    openSub: boolean;
    onHover: () => void;
    onActivate: () => void;
    onCloseSub: () => void;
    onClose: () => void;
    submenuAnchor: (rect: DOMRect) => DOMRect;
  },
): ReactNode {
  if (item.kind === "separator") {
    return <div key={`sep-${i}`} className="ctxm__sep" role="separator" />;
  }
  if (item.kind === "header") {
    return (
      <div key={`h-${i}`} className="ctxm__header">
        {item.label}
      </div>
    );
  }
  if (item.kind === "item") {
    return (
      <ItemRow
        key={item.id ?? `i-${i}`}
        item={item}
        focused={ctx.focused}
        onHover={ctx.onHover}
        onActivate={ctx.onActivate}
      />
    );
  }
  return (
    <SubmenuRow
      key={item.id ?? `sm-${i}`}
      item={item}
      focused={ctx.focused}
      open={ctx.openSub}
      onHover={ctx.onHover}
      onActivate={ctx.onActivate}
      onCloseSub={ctx.onCloseSub}
      onCloseAll={ctx.onClose}
    />
  );
}

function ItemRow({
  item,
  focused,
  onHover,
  onActivate,
}: {
  item: Extract<MenuItem, { kind: "item" }>;
  focused: boolean;
  onHover: () => void;
  onActivate: () => void;
}) {
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (focused) ref.current?.focus();
  }, [focused]);
  return (
    <button
      ref={ref}
      type="button"
      role="menuitem"
      disabled={item.disabled}
      className={
        "ctxm__row" +
        (item.destructive ? " ctxm__row--destructive" : "") +
        (focused ? " ctxm__row--focused" : "")
      }
      onMouseEnter={onHover}
      onMouseMove={onHover}
      onClick={(e) => {
        e.stopPropagation();
        onActivate();
      }}
      tabIndex={-1}
    >
      {item.icon && <span className="ctxm__icon" aria-hidden>{item.icon}</span>}
      <span className="ctxm__label">{item.label}</span>
      {item.keyHint && <span className="ctxm__kbd">{item.keyHint}</span>}
    </button>
  );
}

function SubmenuRow({
  item,
  focused,
  open,
  onHover,
  onActivate,
  onCloseSub,
  onCloseAll,
}: {
  item: Extract<MenuItem, { kind: "submenu" }>;
  focused: boolean;
  open: boolean;
  onHover: () => void;
  onActivate: () => void;
  onCloseSub: () => void;
  onCloseAll: () => void;
}) {
  const rowRef = useRef<HTMLButtonElement>(null);
  const [anchorRect, setAnchorRect] = useState<DOMRect | null>(null);

  useEffect(() => {
    if (!open) {
      setAnchorRect(null);
      return;
    }
    const el = rowRef.current;
    if (!el) return;
    setAnchorRect(el.getBoundingClientRect());
  }, [open]);

  useEffect(() => {
    if (focused) rowRef.current?.focus();
  }, [focused]);

  return (
    <>
      <button
        ref={rowRef}
        type="button"
        role="menuitem"
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={item.disabled}
        className={
          "ctxm__row ctxm__row--submenu" +
          (focused ? " ctxm__row--focused" : "") +
          (open ? " ctxm__row--open" : "")
        }
        onMouseEnter={onHover}
        onMouseMove={onHover}
        onClick={(e) => {
          e.stopPropagation();
          onActivate();
        }}
        tabIndex={-1}
      >
        {item.icon && <span className="ctxm__icon" aria-hidden>{item.icon}</span>}
        <span className="ctxm__label">{item.label}</span>
        <span className="ctxm__chevron" aria-hidden>▸</span>
      </button>
      {open && anchorRect && (
        <Menu
          items={item.items}
          anchor={{ x: anchorRect.right, y: anchorRect.top, from: anchorRect }}
          onClose={() => {
            onCloseSub();
            onCloseAll();
          }}
          level={1}
        />
      )}
    </>
  );
}

function firstFocusable(items: MenuItem[]): number {
  for (let i = 0; i < items.length; i++) {
    const it = items[i]!;
    if (it.kind === "item" || it.kind === "submenu") return i;
  }
  return -1;
}

function focusablePositions(items: MenuItem[]): number[] {
  const out: number[] = [];
  for (let i = 0; i < items.length; i++) {
    const it = items[i]!;
    if (it.kind === "item" || it.kind === "submenu") out.push(i);
  }
  return out;
}

function nextFocusable(positions: number[], current: number, dir: 1 | -1): number {
  if (positions.length === 0) return -1;
  const idx = positions.indexOf(current);
  if (idx === -1) return positions[0]!;
  const n = (idx + dir + positions.length) % positions.length;
  return positions[n]!;
}
