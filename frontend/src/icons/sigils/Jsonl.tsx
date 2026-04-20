import type { SVGProps } from "react";

export function Jsonl(props: SVGProps<SVGSVGElement>) {
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
      <path d="M3 3 L3 13" />
      <path d="M6 4 H13" />
      <path d="M6 8 H13" />
      <path d="M6 12 H11" />
    </svg>
  );
}
