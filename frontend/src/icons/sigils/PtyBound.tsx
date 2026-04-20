import type { SVGProps } from "react";

export function PtyBound(props: SVGProps<SVGSVGElement>) {
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
      <rect x="1.75" y="3.75" width="5" height="8.5" rx="1" />
      <rect x="9.25" y="3.75" width="5" height="8.5" rx="1" />
      <path d="M6.75 8 H9.25" />
    </svg>
  );
}
