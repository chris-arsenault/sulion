import type { SVGProps } from "react";

export function SessionDead(props: SVGProps<SVGSVGElement>) {
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
      <circle cx="8" cy="8" r="5.5" />
      <path d="M4.5 4.5 L11.5 11.5" />
    </svg>
  );
}
