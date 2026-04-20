// <ContextMenuHost> renders the single portal menu at the app root.
// Surfaces attach trigger props from `contextMenuStore` (preferred) or
// the lower-level `contextMenuHandler`. Submenus open one level deep,
// flip horizontally/vertically to stay in the viewport, and are
// keyboard-navigable (arrows / Enter / Esc / ←→ for submenu).

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";

import type { MenuItem, MenuState, OpenAt } from "./contextMenuStore";
import { useContextMenu } from "./contextMenuStore";
import "./ContextMenu.css";

export function ContextMenuHost() {
  const state = useContextMenu((store) => store.state);
  const close = useContextMenu((store) => store.close);
  if (!state) return null;
  return createPortal(<MenuRoot state={state} onClose={close} />, document.body);
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

  const onScrimContextMenu = useCallback(
    (e: React.MouseEvent) => {
      // Right-click on the scrim closes the menu rather than opening
      // another one or leaking to the surface below.
      e.preventDefault();
      onClose();
    },
    [onClose],
  );

  return (
    <div className="ctxm-scrim">
      <button
        type="button"
        className="ctxm-scrim__dismiss"
        aria-label="Dismiss context menu"
        onMouseDown={onClose}
        onContextMenu={onScrimContextMenu}
      />
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

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
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
          onClose();
        } else if (item.kind === "submenu") {
          setOpenSub(focusIdx);
        }
      }
    },
    [focusableIdxs, items, focusIdx, level, onClose],
  );

  const onHoverIndex = useCallback(
    (i: number, kind: MenuItem["kind"]) => {
      setFocusIdx(i);
      if (kind === "submenu") {
        if (submenuTimer.current) clearTimeout(submenuTimer.current);
        submenuTimer.current = setTimeout(() => setOpenSub(i), 120);
      } else {
        if (submenuTimer.current) clearTimeout(submenuTimer.current);
        setOpenSub(null);
      }
    },
    [],
  );
  const onActivateIndex = useCallback(
    (i: number) => {
      const item = items[i];
      if (!item) return;
      if (item.kind === "item" && !item.disabled) {
        item.onSelect();
        onClose();
      } else if (item.kind === "submenu" && !item.disabled) {
        setOpenSub(i);
      }
    },
    [items, onClose],
  );
  const onCloseSub = useCallback(() => setOpenSub(null), []);
  const menuStyle = useMemo(
    () => ({ left: pos.left, top: pos.top }),
    [pos.left, pos.top],
  );
  const onMenuMouseDown = useCallback(
    (e: React.MouseEvent) => e.stopPropagation(),
    [],
  );

  return (
    <div
      ref={ref}
      className="ctxm"
      role="menu"
      tabIndex={-1}
      // eslint-disable-next-line local/no-inline-styles -- cursor-anchored positioning, not representable as CSS classes
      style={menuStyle}
      onMouseDown={onMenuMouseDown}
      onKeyDown={onKeyDown}
    >
      {items.map((item, i) => (
        <MenuItemRow
          key={menuItemKey(item, i)}
          item={item}
          index={i}
          focused={i === focusIdx}
          openSub={openSub === i}
          onHover={onHoverIndex}
          onActivate={onActivateIndex}
          onCloseSub={onCloseSub}
          onClose={onClose}
        />
      ))}
    </div>
  );
}

function menuItemKey(item: MenuItem, i: number): string {
  if (item.kind === "separator") return `sep-${i}`;
  if (item.kind === "header") return `h-${i}`;
  return item.id ?? `${item.kind}-${i}`;
}

function MenuItemRow({
  item,
  index,
  focused,
  openSub,
  onHover,
  onActivate,
  onCloseSub,
  onClose,
}: {
  item: MenuItem;
  index: number;
  focused: boolean;
  openSub: boolean;
  onHover: (i: number, kind: MenuItem["kind"]) => void;
  onActivate: (i: number) => void;
  onCloseSub: () => void;
  onClose: () => void;
}) {
  const hoverThis = useCallback(
    () => onHover(index, item.kind),
    [onHover, index, item.kind],
  );
  const activateThis = useCallback(
    () => onActivate(index),
    [onActivate, index],
  );

  if (item.kind === "separator") {
    return <div className="ctxm__sep" role="separator" />;
  }
  if (item.kind === "header") {
    return <div className="ctxm__header">{item.label}</div>;
  }
  if (item.kind === "item") {
    return (
      <ItemRow
        item={item}
        focused={focused}
        onHover={hoverThis}
        onActivate={activateThis}
      />
    );
  }
  return (
    <SubmenuRow
      item={item}
      focused={focused}
      open={openSub}
      onHover={hoverThis}
      onActivate={activateThis}
      onCloseSub={onCloseSub}
      onCloseAll={onClose}
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
  const onClick = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      onActivate();
    },
    [onActivate],
  );
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
      onClick={onClick}
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

  const onClick = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      onActivate();
    },
    [onActivate],
  );
  const onSubClose = useCallback(() => {
    onCloseSub();
    onCloseAll();
  }, [onCloseSub, onCloseAll]);
  const submenuAnchor = useMemo(
    () =>
      anchorRect
        ? { x: anchorRect.right, y: anchorRect.top, from: anchorRect }
        : null,
    [anchorRect],
  );

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
        onClick={onClick}
        tabIndex={-1}
      >
        {item.icon && <span className="ctxm__icon" aria-hidden>{item.icon}</span>}
        <span className="ctxm__label">{item.label}</span>
        <span className="ctxm__chevron" aria-hidden>▸</span>
      </button>
      {open && submenuAnchor && (
        <Menu
          items={item.items}
          anchor={submenuAnchor}
          onClose={onSubClose}
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
