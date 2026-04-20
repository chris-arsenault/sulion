import type { SVGProps } from "react";

export function RepoStale(props: SVGProps<SVGSVGElement>) {
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
      <path d="M8 5 V8 L10 9.25" />
    </svg>
  );
}
