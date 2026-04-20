import type { HTMLAttributes } from "react";
import { Icon, type IconName, type IconSize } from "../../icons";

export type SigilTone =
  | "ok"
  | "warn"
  | "crit"
  | "info"
  | "atn"
  | "mute"
  | "accent";

export type SigilCategory =
  | "create"
  | "util"
  | "delegate"
  | "workflow"
  | "inspect"
  | "research"
  | "other";

interface SigilProps extends HTMLAttributes<HTMLSpanElement> {
  icon: IconName;
  size?: IconSize;
  tone?: SigilTone;
  category?: SigilCategory;
  ring?: boolean;
  pulse?: boolean;
}

export function Sigil({
  icon,
  size = 16,
  tone,
  category,
  ring,
  pulse,
  className,
  style,
  ...rest
}: SigilProps) {
  const classes = [
    "ui-sigil",
    tone ? `ui-sigil--tone-${tone}` : null,
    category ? `ui-sigil--cat-${category}` : null,
    ring ? "ui-sigil--ring" : null,
    pulse ? "ui-sigil--pulse" : null,
    className,
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <span className={classes} style={style} {...rest}>
      <Icon name={icon} size={size} />
    </span>
  );
}
