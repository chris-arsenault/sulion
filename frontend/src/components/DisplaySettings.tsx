// Display settings: the work-area projection mode (split / terminal
// only / timeline only), sidebar pin, and the hotkeys behind them.
// Hosted as the third view of the overview/metrics modal.

import { useCallback } from "react";

import {
  DISPLAY_MODE_LABELS,
  DISPLAY_MODES,
  useDisplay,
  type DisplayMode,
} from "../state/DisplayStore";
import { useIsMobileLayout } from "../hooks/useSessionNavigation";
import "./DisplaySettings.css";

const MODE_DESCRIPTIONS: Record<DisplayMode, string> = {
  split: "Terminal and timeline stacked in two panes (the classic layout).",
  terminal: "One full-height pane; timeline tabs are hidden.",
  timeline: "One full-height pane; terminal tabs are hidden.",
};

const IS_MAC =
  typeof navigator !== "undefined" && /Mac|iP(hone|ad|od)/.test(navigator.platform);
const MOD = IS_MAC ? "⌘" : "Ctrl";

export function DisplaySettings({
  onOpenOverview,
  onOpenMetrics,
}: {
  /** Modal host overrides: swap the overlay's view in place. */
  onOpenOverview?: () => void;
  onOpenMetrics?: () => void;
}) {
  const mode = useDisplay((store) => store.mode);
  const isMobile = useIsMobileLayout();
  const sidebarPinned = useDisplay((store) => store.sidebarPinned);
  const setMode = useDisplay((store) => store.setMode);
  const toggleSidebar = useDisplay((store) => store.toggleSidebar);

  return (
    <div className="display-settings" data-testid="display-settings">
      <header className="display-settings__bar">
        <h2>Display</h2>
        {onOpenOverview ? (
          <button
            type="button"
            className="display-settings__link"
            onClick={onOpenOverview}
          >
            overview
          </button>
        ) : null}
        {onOpenMetrics ? (
          <button
            type="button"
            className="display-settings__link"
            onClick={onOpenMetrics}
          >
            metrics
          </button>
        ) : null}
      </header>

      <section className="display-settings__section" aria-label="Display mode">
        <header>Reading pane</header>
        {isMobile ? (
          <p className="display-settings__mobile-policy">
            Timeline only is fixed on mobile. Your desktop setting remains{" "}
            {DISPLAY_MODE_LABELS[mode].toLowerCase()}.
          </p>
        ) : (
          <div role="radiogroup" aria-label="Display mode" className="display-settings__modes">
            {DISPLAY_MODES.map((candidate) => (
              <ModeOption
                key={candidate}
                mode={candidate}
                selected={mode === candidate}
                onSelect={setMode}
              />
            ))}
          </div>
        )}
      </section>

      {!isMobile && (
        <section className="display-settings__section" aria-label="Sidebar">
          <header>Sidebar</header>
          <label className="display-settings__toggle">
            <input
              type="checkbox"
              checked={sidebarPinned}
              onChange={toggleSidebar}
            />
            <span>Pin the session sidebar open</span>
          </label>
        </section>
      )}

      <section className="display-settings__section" aria-label="Keyboard shortcuts">
        <header>Hotkeys</header>
        <dl className="display-settings__keys">
          {!isMobile && (
            <>
              <div>
                <dt>{MOD}⇧D</dt>
                <dd>cycle display mode (split → terminal → timeline)</dd>
              </div>
              <div>
                <dt>{MOD}⇧E</dt>
                <dd>peek at the hidden projection (terminal/timeline modes)</dd>
              </div>
              <div>
                <dt>{MOD}⇧B</dt>
                <dd>collapse / pin the sidebar</dd>
              </div>
            </>
          )}
          <div>
            <dt>{MOD}K</dt>
            <dd>command palette</dd>
          </div>
          <div>
            <dt>{MOD}M</dt>
            <dd>team overview modal</dd>
          </div>
        </dl>
      </section>
    </div>
  );
}

function ModeOption({
  mode,
  selected,
  onSelect,
}: {
  mode: DisplayMode;
  selected: boolean;
  onSelect: (mode: DisplayMode) => void;
}) {
  const onClick = useCallback(() => onSelect(mode), [onSelect, mode]);
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      className={`display-settings__mode${
        selected ? " display-settings__mode--selected" : ""
      }`}
      onClick={onClick}
    >
      <span className="display-settings__mode-label">
        {DISPLAY_MODE_LABELS[mode]}
      </span>
      <span className="display-settings__mode-desc">
        {MODE_DESCRIPTIONS[mode]}
      </span>
    </button>
  );
}
