import {
  FloatingPortal,
  autoUpdate,
  flip,
  offset,
  shift,
  useDismiss,
  useFloating,
  useFocus,
  useHover,
  useInteractions,
  useRole,
} from "@floating-ui/react";
import {
  Children,
  cloneElement,
  isValidElement,
  useMemo,
  useState,
  type ReactElement,
  type ReactNode,
} from "react";

interface TooltipProps {
  label: ReactNode;
  placement?: "top" | "bottom" | "left" | "right";
  delay?: number;
  children: ReactElement;
}

export function Tooltip({
  label,
  placement = "top",
  delay = 150,
  children,
}: TooltipProps) {
  const [open, setOpen] = useState(false);

  const { x, y, strategy, refs, context } = useFloating({
    open,
    onOpenChange: setOpen,
    placement,
    whileElementsMounted: autoUpdate,
    middleware: [offset(6), flip({ padding: 8 }), shift({ padding: 8 })],
  });

  const hover = useHover(context, { move: false, delay: { open: delay, close: 0 } });
  const focus = useFocus(context);
  const dismiss = useDismiss(context);
  const role = useRole(context, { role: "tooltip" });

  const { getReferenceProps, getFloatingProps } = useInteractions([
    hover,
    focus,
    dismiss,
    role,
  ]);

  const child = useMemo(() => Children.only(children), [children]);
  if (!isValidElement(child)) return child;

  const childProps = (child.props ?? {}) as Record<string, unknown>;
  const referenceProps = getReferenceProps({
    ref: refs.setReference,
    ...childProps,
  }) as Record<string, unknown>;

  // If label is empty/null, don't wrap — acts as identity.
  if (label === null || label === undefined || label === "") {
    return cloneElement(child, referenceProps as never);
  }

  return (
    <>
      {cloneElement(child, referenceProps as never)}
      {open ? (
        <FloatingPortal>
          <div
            ref={refs.setFloating}
            className="ui-tooltip"
            style={{
              position: strategy,
              top: y ?? 0,
              left: x ?? 0,
            }}
            {...getFloatingProps()}
          >
            {label}
          </div>
        </FloatingPortal>
      ) : null}
    </>
  );
}
