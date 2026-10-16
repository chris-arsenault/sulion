import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties, KeyboardEvent, MouseEvent, PointerEvent, RefObject } from "react";
import { useTimelineControlsStore } from "../../state/TimelineControlsStore";

function isOutput(target: EventTarget) {
  return target instanceof Element &&
    target.closest(".td__text, .td__archived-markdown") != null &&
    target.closest("a, button, input, textarea, select, summary, [contenteditable=true]") == null;
}

export function useTurnReader(mobile: boolean, identity: string, body: RefObject<HTMLDivElement | null>) {
  const [open, setOpen] = useState(false);
  const fontScale = useTimelineControlsStore((store) => store.timelineFontScale);
  const style = useMemo(() => ({
    "--timeline-t-meta": `calc(var(--t-meta) * ${fontScale})`,
    "--timeline-t-ui": `calc(var(--t-ui) * ${fontScale})`,
    "--timeline-t-body": `calc(var(--t-body) * ${fontScale})`,
  }) as CSSProperties, [fontScale]);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const scrollTop = useRef(0);
  const wasOpen = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pointer = useRef<{ id: number; x: number; y: number } | null>(null);
  const dragged = useRef(false);
  const cancelHold = useCallback(() => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
  }, []);
  const show = useCallback(() => {
    cancelHold();
    scrollTop.current = body.current?.scrollTop ?? 0;
    setOpen(true);
  }, [body, cancelHold]);
  const close = useCallback(() => {
    scrollTop.current = body.current?.scrollTop ?? 0;
    setOpen(false);
  }, [body]);

  useEffect(() => {
    scrollTop.current = 0;
    setOpen(false);
    cancelHold();
    return cancelHold;
  }, [identity, cancelHold]);
  useLayoutEffect(() => {
    if (body.current) body.current.scrollTop = scrollTop.current;
    if (!open && wasOpen.current) buttonRef.current?.focus({ preventScroll: true });
    wasOpen.current = open;
  }, [open, body]);

  const onClick = useCallback((event: MouseEvent<HTMLDivElement>) => {
    if (mobile && !open && !dragged.current && isOutput(event.target) && !window.getSelection()?.toString()) show();
  }, [mobile, open, show]);
  const onContextMenuCapture = useCallback((event: MouseEvent<HTMLDivElement>) => {
    if (!mobile || open || !isOutput(event.target)) return;
    event.preventDefault();
    event.stopPropagation();
    show();
  }, [mobile, open, show]);
  const onPointerDown = useCallback((event: PointerEvent<HTMLDivElement>) => {
    cancelHold();
    dragged.current = false;
    pointer.current = null;
    if (!mobile || open || event.button !== 0 || !isOutput(event.target)) return;
    pointer.current = { id: event.pointerId, x: event.clientX, y: event.clientY };
    if (event.pointerType === "mouse" || window.getSelection()?.toString()) return;
    timer.current = setTimeout(show, 500);
  }, [mobile, open, show, cancelHold]);
  const onPointerMove = useCallback((event: PointerEvent<HTMLDivElement>) => {
    const start = pointer.current;
    if (!start || start.id !== event.pointerId) return;
    if (Math.hypot(event.clientX - start.x, event.clientY - start.y) > 10) {
      dragged.current = true;
      cancelHold();
    }
  }, [cancelHold]);
  const onKeyDown = useCallback((event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "Tab") return;
    const dialog = event.currentTarget;
    const controls = Array.from(dialog.querySelectorAll<HTMLElement>(
      'button:not(:disabled), a[href], input:not(:disabled), textarea:not(:disabled), select:not(:disabled), summary, [tabindex="0"]',
    )).filter((element) => element.getClientRects().length > 0);
    const first = controls[0];
    const last = controls[controls.length - 1];
    const focused = document.activeElement;
    if (event.shiftKey ? focused === first || focused === dialog : focused === last || focused === dialog) {
      event.preventDefault();
      (event.shiftKey ? last : first)?.focus();
    }
  }, []);

  return { open, show, close, buttonRef, style, onKeyDown, cancelHold, onClick, onContextMenuCapture, onPointerDown, onPointerMove };
}
