import { useMediaQuery } from "./useMediaQuery";
import { useDisplay } from "../state/DisplayStore";
import {
  effectiveDisplayMode,
  MOBILE_LAYOUT_QUERY,
  promptInjectionTarget,
  type PromptInjectionTarget,
} from "../state/displayPolicy";

/** The surface an `inject-prompt` command should land in right now. Both
 * candidate panes subscribe to the command and compare against this, so
 * exactly one of them takes the text for a given session. */
export function usePromptInjectionTarget(): PromptInjectionTarget {
  const storedMode = useDisplay((store) => store.mode);
  const isMobile = useMediaQuery(MOBILE_LAYOUT_QUERY);
  return promptInjectionTarget(effectiveDisplayMode(storedMode, isMobile));
}
