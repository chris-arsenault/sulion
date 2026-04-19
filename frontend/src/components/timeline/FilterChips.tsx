// Facet chips for the timeline. Simple semantic:
//   - A chip is BRIGHT (default) when that category is visible
//   - Clicking it DIMS it + draws a strikethrough → category hidden
//   - Click again → bright → visible
// The left-bar "errors only" and the file-path textbox are include-only
// filters (they're labeled as such and behave intuitively).

import { KNOWN_TOOLS, type TimelineFilters } from "./filters";
import "./FilterChips.css";

interface Props {
  filters: TimelineFilters;
  toggleSpeaker: (s: "user" | "assistant" | "tool_result") => void;
  toggleTool: (name: string) => void;
  setErrorsOnly: (v: boolean) => void;
  setShowThinking: (v: boolean) => void;
  setShowBookkeeping: (v: boolean) => void;
  setShowSidechain: (v: boolean) => void;
  setFilePath: (v: string) => void;
  reset: () => void;
}

export function FilterChips({
  filters,
  toggleSpeaker,
  toggleTool,
  setErrorsOnly,
  setShowThinking,
  setShowBookkeeping,
  setShowSidechain,
  setFilePath,
  reset,
}: Props) {
  const hasAnythingHidden =
    filters.hiddenSpeakers.size > 0 ||
    filters.hiddenTools.size > 0 ||
    filters.errorsOnly ||
    filters.filePath.length > 0 ||
    !filters.showThinking ||
    filters.showBookkeeping ||
    filters.showSidechain;

  return (
    <div className="fc" data-testid="filter-chips">
      <div className="fc__group">
        <span className="fc__label">Show</span>
        <HideChip
          hidden={filters.hiddenSpeakers.has("user")}
          onClick={() => toggleSpeaker("user")}
          label="user"
        />
        <HideChip
          hidden={filters.hiddenSpeakers.has("assistant")}
          onClick={() => toggleSpeaker("assistant")}
          label="claude"
        />
        <HideChip
          hidden={filters.hiddenSpeakers.has("tool_result")}
          onClick={() => toggleSpeaker("tool_result")}
          label="tool result"
        />
      </div>

      <div className="fc__group">
        <span className="fc__label">Tools</span>
        {KNOWN_TOOLS.map((t) => (
          <HideChip
            key={t}
            hidden={filters.hiddenTools.has(t)}
            onClick={() => toggleTool(t)}
            label={t}
            variant={t.toLowerCase()}
          />
        ))}
      </div>

      <div className="fc__group">
        <IncludeChip
          active={filters.errorsOnly}
          onClick={() => setErrorsOnly(!filters.errorsOnly)}
          label="errors only"
        />
        <HideChip
          hidden={!filters.showThinking}
          onClick={() => setShowThinking(!filters.showThinking)}
          label="💭 thinking"
        />
        <HideChip
          hidden={!filters.showBookkeeping}
          onClick={() => setShowBookkeeping(!filters.showBookkeeping)}
          label="bookkeeping"
        />
        <HideChip
          hidden={!filters.showSidechain}
          onClick={() => setShowSidechain(!filters.showSidechain)}
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
          onChange={(e) => setFilePath(e.target.value)}
          aria-label="Filter to turns referencing file path"
        />
      </div>

      {hasAnythingHidden && (
        <button
          type="button"
          className="fc__clear"
          onClick={reset}
          title="Reset all filters"
        >
          Show all
        </button>
      )}
    </div>
  );
}

/** Hide chip: bright when the category is SHOWING (default), dim +
 * strikethrough when hidden. Clicking toggles. */
function HideChip({
  hidden,
  onClick,
  label,
  variant,
}: {
  hidden: boolean;
  onClick: () => void;
  label: string;
  variant?: string;
}) {
  const cls = [
    "fc__chip",
    variant ? `fc__chip--${variant}` : "",
    hidden ? "fc__chip--hidden" : "fc__chip--visible",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <button
      type="button"
      className={cls}
      onClick={onClick}
      aria-pressed={hidden}
      title={hidden ? `${label} — hidden (click to show)` : `${label} — showing (click to hide)`}
    >
      {label}
    </button>
  );
}

/** Include-only chip: dim when inactive (no constraint), bright when
 * active (filtering). Used for errorsOnly — still the intuitive
 * "click to filter" semantic. */
function IncludeChip({
  active,
  onClick,
  label,
}: {
  active: boolean;
  onClick: () => void;
  label: string;
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
    >
      {label}
    </button>
  );
}
