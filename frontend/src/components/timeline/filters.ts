// Timeline filter chips. Flat, obvious semantics:
//
//   - Speaker/operation chips: HIDE the named category when clicked.
//     Default is "nothing hidden, everything visible". Click
//     "create content" → those tool rows disappear from the timeline.
//     Click again → they come back. There is no "inclusive" mode.
//
//   - errorsOnly / filePath: genuine include-only filters. "Errors only"
//     means drop turns that don't have any error. Empty filePath means
//     no constraint.
//
//   - showThinking / showBookkeeping / showSidechain: same "show" toggles
//     from before — clearly named for their semantics.
//
// The state itself lives in the singleton `TimelineControlsStore` (state
// shared across every mounted timeline, not per-component); this module
// re-exports the type/constants and wraps the store in the same hook
// shape callers already use.

import { useTimelineControlsStore } from "../../state/TimelineControlsStore";

export {
  DEFAULT_FILTERS,
  KNOWN_OPERATION_CATEGORIES,
  OPERATION_CATEGORY_LABELS,
  type TimelineFilters,
} from "../../state/TimelineControlsStore";
import type { TimelineFilters } from "../../state/TimelineControlsStore";
import type { OperationCategory, SpeakerFacet } from "../../api/types";

export function useTimelineFilters(): {
  filters: TimelineFilters;
  toggleSpeaker: (s: SpeakerFacet) => void;
  toggleOperationCategory: (category: OperationCategory) => void;
  setErrorsOnly: (v: boolean) => void;
  setShowThinking: (v: boolean) => void;
  setShowBookkeeping: (v: boolean) => void;
  setShowSidechain: (v: boolean) => void;
  setFilePath: (v: string) => void;
  setFollowLatest: (v: boolean) => void;
  reset: () => void;
} {
  const filters = useTimelineControlsStore((s) => s.filters);
  const toggleSpeaker = useTimelineControlsStore((s) => s.toggleSpeaker);
  const toggleOperationCategory = useTimelineControlsStore((s) => s.toggleOperationCategory);
  const setErrorsOnly = useTimelineControlsStore((s) => s.setErrorsOnly);
  const setShowThinking = useTimelineControlsStore((s) => s.setShowThinking);
  const setShowBookkeeping = useTimelineControlsStore((s) => s.setShowBookkeeping);
  const setShowSidechain = useTimelineControlsStore((s) => s.setShowSidechain);
  const setFilePath = useTimelineControlsStore((s) => s.setFilePath);
  const setFollowLatest = useTimelineControlsStore((s) => s.setFollowLatest);
  const reset = useTimelineControlsStore((s) => s.resetFilters);

  return {
    filters,
    toggleSpeaker,
    toggleOperationCategory,
    setErrorsOnly,
    setShowThinking,
    setShowBookkeeping,
    setShowSidechain,
    setFilePath,
    setFollowLatest,
    reset,
  };
}
