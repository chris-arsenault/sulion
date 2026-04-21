// Facet chips for the timeline. Simple semantic:
//   - A chip is BRIGHT (default) when that category is visible
//   - Clicking it DIMS it + draws a strikethrough → category hidden
//   - Click again → bright → visible
// The left-bar "errors only" and the file-path textbox are include-only
// filters (they're labeled as such and behave intuitively).

import { useCallback } from "react";

import {
  KNOWN_OPERATION_CATEGORIES,
  OPERATION_CATEGORY_LABELS,
  type TimelineFilters,
} from "./filters";
import type { OperationCategory } from "../../api/types";
import { Icon } from "../../icons";
import { Tooltip } from "../ui";
import "./FilterChips.css";

const THINKING_LABEL = (
  <>
    <Icon name="sparkles" size={12} /> thinking
  </>
);

const FOLLOW_LATEST_LABEL = (
  <>
    <Icon name="activity" size={12} /> follow latest
  </>
);

interface Props {
  filters: TimelineFilters;
  toggleSpeaker: (s: "user" | "assistant" | "tool_result") => void;
  toggleOperationCategory: (category: OperationCategory) => void;
  setErrorsOnly: (v: boolean) => void;
  setShowThinking: (v: boolean) => void;
  setShowBookkeeping: (v: boolean) => void;
  setShowSidechain: (v: boolean) => void;
  setFilePath: (v: string) => void;
  setFollowLatest: (v: boolean) => void;
  reset: () => void;
}

export function FilterChips({
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
}: Props) {
  const hasAnythingHidden =
    filters.hiddenSpeakers.size > 0 ||
    filters.hiddenOperationCategories.size > 0 ||
    filters.errorsOnly ||
    filters.filePath.length > 0 ||
    !filters.showThinking ||
    filters.showBookkeeping ||
    filters.showSidechain;

  const toggleErrorsOnly = useCallback(
    () => setErrorsOnly(!filters.errorsOnly),
    [filters.errorsOnly, setErrorsOnly],
  );
  const toggleThinking = useCallback(
    () => setShowThinking(!filters.showThinking),
    [filters.showThinking, setShowThinking],
  );
  const toggleBookkeeping = useCallback(
    () => setShowBookkeeping(!filters.showBookkeeping),
    [filters.showBookkeeping, setShowBookkeeping],
  );
  const toggleSidechain = useCallback(
    () => setShowSidechain(!filters.showSidechain),
    [filters.showSidechain, setShowSidechain],
  );
  const toggleFollowLatest = useCallback(
    () => setFollowLatest(!filters.followLatest),
    [filters.followLatest, setFollowLatest],
  );
  const onFilePathChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => setFilePath(e.target.value),
    [setFilePath],
  );

  return (
    <div className="fc" data-testid="filter-chips">
      <div className="fc__group">
        <span className="fc__label">Show</span>
        <SpeakerChip
          speaker="user"
          hidden={filters.hiddenSpeakers.has("user")}
          toggle={toggleSpeaker}
          label="user"
        />
        <SpeakerChip
          speaker="assistant"
          hidden={filters.hiddenSpeakers.has("assistant")}
          toggle={toggleSpeaker}
          label="assistant"
        />
        <SpeakerChip
          speaker="tool_result"
          hidden={filters.hiddenSpeakers.has("tool_result")}
          toggle={toggleSpeaker}
          label="tool result"
        />
      </div>

      <div className="fc__group">
        <span className="fc__label">Operations</span>
        {KNOWN_OPERATION_CATEGORIES.map((category) => (
          <CategoryChip
            key={category}
            category={category}
            hidden={filters.hiddenOperationCategories.has(category)}
            toggle={toggleOperationCategory}
          />
        ))}
      </div>

      <div className="fc__group">
        <IncludeChip
          active={filters.followLatest}
          onClick={toggleFollowLatest}
          label={FOLLOW_LATEST_LABEL}
          ariaLabel="follow latest turn"
        />
        <IncludeChip
          active={filters.errorsOnly}
          onClick={toggleErrorsOnly}
          label="errors only"
        />
        <HideChip
          hidden={!filters.showThinking}
          onClick={toggleThinking}
          label={THINKING_LABEL}
          ariaLabel="thinking"
        />
        <HideChip
          hidden={!filters.showBookkeeping}
          onClick={toggleBookkeeping}
          label="bookkeeping"
        />
        <HideChip
          hidden={!filters.showSidechain}
          onClick={toggleSidechain}
          label="sidechain"
        />
      </div>

      <div className="fc__group fc__group--grow">
        <span className="fc__label">File</span>
        <input
          type="text"
          className="fc__input"
          placeholder="show only turns touching this path…"
          value={filters.filePath}
          onChange={onFilePathChange}
          aria-label="Filter to turns referencing file path"
        />
      </div>

      {hasAnythingHidden && (
        <Tooltip label="Reset all filters">
          <button type="button" className="fc__clear" onClick={reset}>
            Show all
          </button>
        </Tooltip>
      )}
    </div>
  );
}

function SpeakerChip({
  speaker,
  hidden,
  toggle,
  label,
}: {
  speaker: "user" | "assistant" | "tool_result";
  hidden: boolean;
  toggle: (s: "user" | "assistant" | "tool_result") => void;
  label: string;
}) {
  const onClick = useCallback(() => toggle(speaker), [toggle, speaker]);
  return <HideChip hidden={hidden} onClick={onClick} label={label} />;
}

function CategoryChip({
  category,
  hidden,
  toggle,
}: {
  category: OperationCategory;
  hidden: boolean;
  toggle: (c: OperationCategory) => void;
}) {
  const onClick = useCallback(() => toggle(category), [toggle, category]);
  return (
    <HideChip
      hidden={hidden}
      onClick={onClick}
      label={OPERATION_CATEGORY_LABELS[category]}
      variant={variantClass(category)}
    />
  );
}

function variantClass(value: string): string {
  return value.replace(/[^a-z0-9]/gi, "");
}

/** Hide chip: bright when the category is SHOWING (default), dim +
 * strikethrough when hidden. Clicking toggles. */
function HideChip({
  hidden,
  onClick,
  label,
  variant,
  ariaLabel,
}: {
  hidden: boolean;
  onClick: () => void;
  label: React.ReactNode;
  variant?: string;
  ariaLabel?: string;
}) {
  const cls = [
    "fc__chip",
    variant ? `fc__chip--${variant}` : "",
    hidden ? "fc__chip--hidden" : "fc__chip--visible",
  ]
    .filter(Boolean)
    .join(" ");
  const tip = hidden
    ? `${ariaLabel ?? (typeof label === "string" ? label : "")} — hidden (click to show)`
    : `${ariaLabel ?? (typeof label === "string" ? label : "")} — showing (click to hide)`;
  return (
    <Tooltip label={tip}>
      <button
        type="button"
        className={cls}
        onClick={onClick}
        aria-pressed={hidden}
        aria-label={ariaLabel}
      >
        {label}
      </button>
    </Tooltip>
  );
}

/** Include-only chip: dim when inactive (no constraint), bright when
 * active (filtering). Used for errorsOnly — still the intuitive
 * "click to filter" semantic. */
function IncludeChip({
  active,
  onClick,
  label,
  ariaLabel,
}: {
  active: boolean;
  onClick: () => void;
  label: React.ReactNode;
  ariaLabel?: string;
}) {
  const cls = [
    "fc__chip",
    "fc__chip--include",
    active ? "fc__chip--include-active" : "",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <button
      type="button"
      className={cls}
      onClick={onClick}
      aria-pressed={active}
      aria-label={ariaLabel}
    >
      {label}
    </button>
  );
}
