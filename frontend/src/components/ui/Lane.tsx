import type { CSSProperties, HTMLAttributes, ReactNode } from "react";

export type LaneSize = "xs" | "sm" | "md" | "lg";

interface LaneProps extends HTMLAttributes<HTMLDivElement> {
  size?: LaneSize;
  leading?: ReactNode;
  trailing?: ReactNode;
  meta?: ReactNode;
  selected?: boolean;
  muted?: boolean;
  as?: "div" | "button" | "li" | "a";
  innerStyle?: CSSProperties;
}

export function Lane({
  size = "sm",
  leading,
  trailing,
  meta,
  selected,
  muted,
  className,
  children,
  as,
  ...rest
}: LaneProps) {
  const classes = [
    "ui-lane",
    `ui-lane--${size}`,
    selected ? "ui-lane--selected" : null,
    muted ? "ui-lane--muted" : null,
    className,
  ]
    .filter(Boolean)
    .join(" ");

  const Tag = (as ?? "div") as "div";
  return (
    <Tag className={classes} {...rest}>
      {leading !== undefined ? (
        <span className="ui-lane__slot ui-lane__slot--leading">{leading}</span>
      ) : null}
      <span className="ui-lane__label">{children}</span>
      {meta !== undefined ? <span className="ui-lane__meta">{meta}</span> : null}
      {trailing !== undefined ? (
        <span className="ui-lane__slot ui-lane__slot--trailing">{trailing}</span>
      ) : null}
    </Tag>
  );
}
