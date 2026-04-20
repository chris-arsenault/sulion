import { useEffect, useState } from "react";

/** Track a CSS media query in React state. Returns true when the query
 * currently matches. Used to collapse the inspector pane into an
 * overlay modal on narrow viewports. */
export function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = useState(() => {
    if (typeof window === "undefined" || !window.matchMedia) return false;
    return window.matchMedia(query).matches;
  });

  useEffect(() => {
    if (typeof window === "undefined" || !window.matchMedia) return;
    const m = window.matchMedia(query);
    const onChange = () => setMatches(m.matches);
    // `addEventListener('change', ...)` is the modern API; fall back to the
    // deprecated `addListener` for Safari versions in the wild.
    if (m.addEventListener) m.addEventListener("change", onChange);
    else m.addListener(onChange);
    if (matches !== m.matches) setMatches(m.matches);
    return () => {
      if (m.removeEventListener) m.removeEventListener("change", onChange);
      else m.removeListener(onChange);
    };
  }, [matches, query]);

  return matches;
}
