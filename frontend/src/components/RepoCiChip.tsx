import type { RepoCiState, RepoCiStatus } from "../api/types";
import type { IconName } from "../icons";
import { Sigil, Tooltip, type SigilTone } from "./ui";

const CI_PRESENTATION: Record<
  RepoCiState,
  { icon: IconName; tone: SigilTone; label: string }
> = {
  succeeded: { icon: "check", tone: "ok", label: "succeeded" },
  failed: { icon: "x", tone: "crit", label: "failed" },
  in_progress: { icon: "refresh-cw", tone: "info", label: "in progress" },
};

/** Latest GitHub Actions run state and age for a repo's checked-out branch. */
export function RepoCiChip({
  ci,
  branch,
  age,
}: {
  ci: RepoCiStatus;
  branch: string | null;
  age: (iso: string) => string;
}) {
  const view = CI_PRESENTATION[ci.state];
  const when = age(ci.updated_at);
  return (
    <Tooltip
      label={`CI ${view.label} · ${when} ago\nlatest Actions run on ${branch ?? "this branch"}`}
    >
      <span
        className={`sidebar__repo-ci sidebar__repo-ci--${ci.state}`}
        data-ci-state={ci.state}
        aria-label={`CI ${view.label}, ${when} ago`}
      >
        <Sigil
          icon={view.icon}
          size={12}
          tone={view.tone}
          pulse={ci.state === "in_progress"}
        />
        <span className="tabular">{when}</span>
      </span>
    </Tooltip>
  );
}
