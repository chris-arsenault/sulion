import type { HTMLAttributes, ReactNode } from "react";

interface PanelProps extends HTMLAttributes<HTMLDivElement> {
  header?: ReactNode;
  footer?: ReactNode;
  tone?: "canvas-1" | "canvas-2";
  flush?: boolean;
}

export function Panel({
  header,
  footer,
  tone = "canvas-1",
  flush,
  className,
  children,
  ...rest
}: PanelProps) {
  const classes = [
    "ui-panel",
    `ui-panel--${tone}`,
    flush ? "ui-panel--flush" : null,
    className,
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <div className={classes} {...rest}>
      {header !== undefined ? (
        <div className="ui-panel__header">{header}</div>
      ) : null}
      <div className="ui-panel__body">{children}</div>
      {footer !== undefined ? (
        <div className="ui-panel__footer">{footer}</div>
      ) : null}
    </div>
  );
}
