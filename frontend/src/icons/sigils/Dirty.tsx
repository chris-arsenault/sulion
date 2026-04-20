import type { SVGProps } from "react";

export function Dirty(props: SVGProps<SVGSVGElement>) {
  return (
    <svg
      viewBox="0 0 16 16"
      fill="currentColor"
      aria-hidden="true"
      {...props}
    >
      <circle cx="8" cy="8" r="3.25" />
    </svg>
  );
}
