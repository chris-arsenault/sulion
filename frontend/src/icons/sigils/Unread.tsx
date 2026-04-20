import type { SVGProps } from "react";

export function Unread(props: SVGProps<SVGSVGElement>) {
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
      <circle cx="8" cy="8" r="3.5" fill="currentColor" stroke="none" />
      <circle cx="8" cy="8" r="6.25" opacity="0.35" />
    </svg>
  );
}
