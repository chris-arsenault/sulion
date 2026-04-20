import type { CSSProperties, HTMLAttributes, ReactNode } from "react";
import { useEffect, useRef } from "react";
import { Icon } from "../../icons";

type DivProps = Omit<HTMLAttributes<HTMLDivElement>, "title">;

interface OverlayProps extends DivProps {
  open: boolean;
  onClose: () => void;
  title?: ReactNode;
  subtitle?: ReactNode;
  leading?: ReactNode;
  modal?: boolean;
  anchorTo?: { top: number; left: number; width?: number };
  width?: number | string;
  maxWidth?: number | string;
  maxHeight?: number | string;
  footer?: ReactNode;
}

export function Overlay({
  open,
  onClose,
  title,
  subtitle,
  leading,
  modal = false,
  anchorTo,
  width,
  maxWidth = "min(90vw, 760px)",
  maxHeight = "70vh",
  footer,
  className,
  children,
  ...rest
}: OverlayProps) {
  const dialogRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, onClose]);

  useEffect(() => {
    if (open && dialogRef.current) {
      dialogRef.current.focus();
    }
  }, [open]);

  if (!open) return null;

  const surfaceStyle: CSSProperties = {
    width,
    maxWidth,
    maxHeight,
  };
  if (anchorTo) {
    surfaceStyle.position = "fixed";
    surfaceStyle.top = anchorTo.top;
    surfaceStyle.left = anchorTo.left;
    if (anchorTo.width !== undefined) surfaceStyle.width = anchorTo.width;
  }

  const classes = [
    "ui-overlay",
    modal ? "ui-overlay--modal" : "ui-overlay--flyout",
    className,
  ]
    .filter(Boolean)
    .join(" ");

  const onScrimClick = modal ? onClose : undefined;

  return (
    <div className={classes} role="presentation">
      {modal ? (
        <div
          className="ui-overlay__scrim"
          onClick={onScrimClick}
          aria-hidden="true"
        />
      ) : null}
      <div
        ref={dialogRef}
        className="ui-overlay__surface"
        style={surfaceStyle}
        role={modal ? "dialog" : "group"}
        aria-modal={modal ? true : undefined}
        tabIndex={-1}
        {...rest}
      >
        {(title !== undefined || leading !== undefined) && (
          <div className="ui-overlay__header">
            {leading !== undefined ? (
              <span className="ui-overlay__leading">{leading}</span>
            ) : null}
            <div className="ui-overlay__titlewrap">
              {title !== undefined ? (
                <div className="ui-overlay__title">{title}</div>
              ) : null}
              {subtitle !== undefined ? (
                <div className="ui-overlay__subtitle">{subtitle}</div>
              ) : null}
            </div>
            <button
              type="button"
              className="ui-overlay__close"
              onClick={onClose}
              aria-label="Close"
            >
              <Icon name="x" size={14} />
            </button>
          </div>
        )}
        <div className="ui-overlay__body">{children}</div>
        {footer !== undefined ? (
          <div className="ui-overlay__footer">{footer}</div>
        ) : null}
      </div>
    </div>
  );
}
