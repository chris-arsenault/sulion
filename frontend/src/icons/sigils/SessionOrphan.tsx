import type { SVGProps } from "react";

export function SessionOrphan(props: SVGProps<SVGSVGElement>) {
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
      <circle
        cx="8"
        cy="8"
        r="5.5"
        strokeDasharray="2.4 2"
        fill="currentColor"
        fillOpacity="0.18"
      />
      <circle cx="8" cy="8" r="1.5" fill="currentColor" stroke="none" />
    </svg>
  );
}
