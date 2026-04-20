import type { HTMLAttributes, ReactNode } from "react";

interface StatProps extends HTMLAttributes<HTMLSpanElement> {
  label?: ReactNode;
  value: ReactNode;
  unit?: ReactNode;
  tone?: "ok" | "warn" | "crit" | "info" | "atn" | "mute" | "accent";
}

export function Stat({
  label,
  value,
  unit,
  tone,
  className,
  ...rest
}: StatProps) {
  const classes = [
    "ui-stat",
    tone ? `ui-stat--tone-${tone}` : null,
    className,
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <span className={classes} {...rest}>
      {label !== undefined ? (
        <span className="ui-stat__label">{label}</span>
      ) : null}
      <span className="ui-stat__value tabular">{value}</span>
      {unit !== undefined ? <span className="ui-stat__unit">{unit}</span> : null}
    </span>
  );
}
