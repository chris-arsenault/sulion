import type { HTMLAttributes, ReactNode } from "react";

interface TabProps extends HTMLAttributes<HTMLButtonElement> {
  active?: boolean;
  leading?: ReactNode;
  trailing?: ReactNode;
  binding?: ReactNode;
}

export function Tab({
  active,
  leading,
  trailing,
  binding,
  className,
  children,
  ...rest
}: TabProps) {
  const classes = [
    "ui-tab",
    active ? "ui-tab--active" : null,
    className,
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <button type="button" className={classes} {...rest}>
      {binding !== undefined ? (
        <span className="ui-tab__binding">{binding}</span>
      ) : null}
      {leading !== undefined ? (
        <span className="ui-tab__leading">{leading}</span>
      ) : null}
      <span className="ui-tab__label">{children}</span>
      {trailing !== undefined ? (
        <span className="ui-tab__trailing">{trailing}</span>
      ) : null}
    </button>
  );
}
