import type { SVGProps } from "react";

export function SessionLive(props: SVGProps<SVGSVGElement>) {
  return (
    <svg
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      {...props}
    >
      <circle cx="8" cy="8" r="3" fill="currentColor" stroke="none" />
      <circle cx="8" cy="8" r="6" opacity="0.45" />
    </svg>
  );
}
