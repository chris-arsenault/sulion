// Singleton timeline settings: turn-navigation mode, text size, and the
// filter chips — all backed by the shared TimelineControlsStore, so this
// flyout reflects (and edits) the same state no matter which open
// timeline's trigger button opened it.

import { useCallback } from "react";
import { createPortal } from "react-dom";

import {
  clampTimelineFontScale,
  TIMELINE_FONT_SCALE_DEFAULT,
  TIMELINE_FONT_SCALE_MAX,
  TIMELINE_FONT_SCALE_MIN,
  TIMELINE_FONT_SCALE_STEP,
  TURN_NAV_MODES,
  useTimelineFontScale,
  useTurnNavMode,
  type TurnNavMode,
} from "../../state/paneTextScale";
import { useTimelineFilters } from "./filters";
import { FilterChips } from "./FilterChips";
import { useFlyoutPosition } from "./useFlyoutPosition";
import { Icon } from "../../icons";
import { Tooltip } from "../ui";
import "./Flyout.css";
import "./TimelineControlsFlyout.css";

interface Props {
  anchor: HTMLElement | null;
  onClose: () => void;
}

const TURN_NAV_ICONS = {
  list: "list",
  grid: "layers",
  hidden: "panel-left-close",
} as const satisfies Record<TurnNavMode, string>;

const TURN_NAV_LABELS: Record<TurnNavMode, string> = {
  list: "List",
  grid: "Grid",
  hidden: "Hidden",
};

export function TimelineControlsFlyout({ anchor, onClose }: Props) {
  const [turnNavMode, setTurnNavMode] = useTurnNavMode();
  const [timelineFontScale, setTimelineFontScale] = useTimelineFontScale();
  const filterHook = useTimelineFilters();
  const { cardRef, isNarrow, cardStyle } = useFlyoutPosition(anchor, onClose);

  const decreaseText = useCallback(
    () => setTimelineFontScale((value) => clampTimelineFontScale(value - TIMELINE_FONT_SCALE_STEP)),
    [setTimelineFontScale],
  );
  const increaseText = useCallback(
    () => setTimelineFontScale((value) => clampTimelineFontScale(value + TIMELINE_FONT_SCALE_STEP)),
    [setTimelineFontScale],
  );
  const resetText = useCallback(
    () => setTimelineFontScale(TIMELINE_FONT_SCALE_DEFAULT),
    [setTimelineFontScale],
  );

  const body = (
    <>
      <div className="flyout__header">
        <span className="flyout__title">
          <Icon name="settings" size={12} />
          <span>timeline settings</span>
        </span>
        <button
          type="button"
          className="flyout__close"
          onClick={onClose}
          aria-label="Close timeline settings"
        >
          <Icon name="x" size={12} />
        </button>
      </div>
      <div className="tcf__body">
        <div className="tcf__section">
          <span className="tcf__label">Turn navigation</span>
          <div className="tcf__nav-group" role="radiogroup" aria-label="Turn navigation mode">
            {TURN_NAV_MODES.map((mode) => (
              <NavModeButton
                key={mode}
                mode={mode}
                active={mode === turnNavMode}
                onSelect={setTurnNavMode}
              />
            ))}
          </div>
        </div>
        <div className="tcf__section">
          <span className="tcf__label">Text size</span>
          <div className="tcf__text-controls" aria-label="Timeline text size controls">
            <button
              type="button"
              className="tcf__text-button"
              onClick={decreaseText}
              disabled={timelineFontScale <= TIMELINE_FONT_SCALE_MIN}
              aria-label="Decrease timeline text size"
            >
              A-
            </button>
            <button
              type="button"
              className="tcf__text-button tcf__text-button--value"
              onClick={resetText}
              aria-label="Reset timeline text size"
            >
              {Math.round(timelineFontScale * 100)}%
            </button>
            <button
              type="button"
              className="tcf__text-button"
              onClick={increaseText}
              disabled={timelineFontScale >= TIMELINE_FONT_SCALE_MAX}
              aria-label="Increase timeline text size"
            >
              A+
            </button>
          </div>
        </div>
        <div className="tcf__section">
          <span className="tcf__label">Filters</span>
          <FilterChips {...filterHook} />
        </div>
      </div>
    </>
  );

  if (isNarrow) {
    return createPortal(
      <div className="flyout__sheet-backdrop" data-testid="timeline-controls-flyout">
        <button
          type="button"
          className="flyout__sheet-dismiss"
          aria-label="Dismiss timeline settings"
          onClick={onClose}
        />
        <div
          ref={cardRef}
          className="flyout__sheet"
          role="dialog"
          aria-modal="true"
          aria-label="timeline settings"
        >
          {body}
        </div>
      </div>,
      document.body,
    );
  }

  return createPortal(
    <div
      ref={cardRef}
      className="flyout__card tcf__card"
      // eslint-disable-next-line local/no-inline-styles -- popover position is anchor-relative, computed at render time
      style={cardStyle}
      role="dialog"
      aria-label="timeline settings"
      data-testid="timeline-controls-flyout"
    >
      {body}
    </div>,
    document.body,
  );
}

function NavModeButton({
  mode,
  active,
  onSelect,
}: {
  mode: TurnNavMode;
  active: boolean;
  onSelect: (mode: TurnNavMode) => void;
}) {
  const onClick = useCallback(() => onSelect(mode), [onSelect, mode]);
  return (
    <Tooltip label={`Turn navigation: ${TURN_NAV_LABELS[mode]}`}>
      <button
        type="button"
        className={active ? "tcf__nav-button tcf__nav-button--active" : "tcf__nav-button"}
        role="radio"
        aria-checked={active}
        onClick={onClick}
      >
        <Icon name={TURN_NAV_ICONS[mode]} size={12} />
        <span>{TURN_NAV_LABELS[mode]}</span>
      </button>
    </Tooltip>
  );
}
