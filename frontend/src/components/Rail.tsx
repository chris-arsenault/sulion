import { useCallback, useMemo } from "react";
import { useShallow } from "zustand/react/shallow";

import { appCommands } from "../state/AppCommands";
import { stalenessFor, useRepos } from "../state/RepoStore";
import { useSessions } from "../state/SessionStore";
import { Icon } from "../icons";
import { Tooltip } from "./ui";
import "./Rail.css";

interface RailProps {
  pinned: boolean;
  onTogglePinned: () => void;
  onOpenMonitor: () => void;
  onOpenSecrets: () => void;
  onOpenPalette: () => void;
}

/** Left rail — functional. Lists each repo as a sigil with staleness ring
 * + unread dot. Click scrolls the sidebar to that repo and expands it.
 * Also carries pin toggle and command palette trigger. */
export function Rail({
  pinned,
  onTogglePinned,
  onOpenMonitor,
  onOpenSecrets,
  onOpenPalette,
}: RailProps) {
  const { repos, sessions, isUnread } = useSessions(
    useShallow((store) => ({
      repos: store.repos,
      sessions: store.sessions,
      isUnread: store.isUnread,
    })),
  );
  const repoStates = useRepos((store) => store.repos);

  const items = useMemo(() => {
    const byRepo = new Map<string, { unread: boolean; latest: number | null }>();
    for (const s of sessions) {
      const entry = byRepo.get(s.repo) ?? { unread: false, latest: null };
      if (isUnread(s.id, s.last_event_at)) entry.unread = true;
      if (s.last_event_at) {
        const t = new Date(s.last_event_at).getTime();
        if (entry.latest === null || t > entry.latest) entry.latest = t;
      }
      byRepo.set(s.repo, entry);
    }
    return repos
      .slice()
      .sort((a, b) => a.name.localeCompare(b.name))
      .map((r) => {
        const st = byRepo.get(r.name) ?? { unread: false, latest: null };
        const git = repoStates[r.name]?.git ?? null;
        const staleness = stalenessFor(git, st.latest);
        return {
          name: r.name,
          unread: st.unread,
          staleness,
          branch: git?.branch ?? null,
          uncommitted: git?.uncommitted_count ?? 0,
        };
      });
  }, [repos, sessions, repoStates, isUnread]);

  return (
    <nav className="rail" aria-label="Repos">
      <Tooltip label={pinned ? "Unpin sidebar" : "Pin sidebar open"} placement="right">
        <button
          type="button"
          className="rail__icon"
          onClick={onTogglePinned}
          aria-label={pinned ? "Unpin sidebar" : "Pin sidebar"}
          aria-pressed={pinned}
        >
          <Icon name={pinned ? "panel-left-close" : "panel-left"} size={14} />
        </button>
      </Tooltip>

      <div className="rail__repos">
        {items.length === 0 ? null : (
          items.map((it) => <RailRepo key={it.name} item={it} />)
        )}
      </div>

      <div className="rail__spacer" />

      <Tooltip label="Monitor active agents" placement="right">
        <button
          type="button"
          className="rail__icon"
          onClick={onOpenMonitor}
          aria-label="Open monitor"
        >
          <Icon name="activity" size={14} />
        </button>
      </Tooltip>

      <Tooltip label="Secrets" placement="right">
        <button
          type="button"
          className="rail__icon"
          onClick={onOpenSecrets}
          aria-label="Open secrets manager"
        >
          <Icon name="settings" size={14} />
        </button>
      </Tooltip>

      <Tooltip label="Command palette  ⌘K" placement="right">
        <button
          type="button"
          className="rail__icon"
          onClick={onOpenPalette}
          aria-label="Open command palette"
        >
          <Icon name="command" size={14} />
        </button>
      </Tooltip>
    </nav>
  );
}

interface RailRepoItem {
  name: string;
  unread: boolean;
  staleness: "green" | "amber" | "red";
  branch: string | null;
  uncommitted: number;
}

function RailRepo({ item }: { item: RailRepoItem }) {
  const letter = item.name.charAt(0).toUpperCase() || "?";
  const toneClass =
    item.staleness === "red"
      ? "rail__sigil--crit"
      : item.staleness === "amber"
        ? "rail__sigil--warn"
        : "rail__sigil--ok";
  const pulse = item.staleness === "red";

  const tooltip = (
    <span className="rail__tip">
      <span className="rail__tip-name">{item.name}</span>
      {item.branch ? (
        <span className="rail__tip-meta">
          {item.branch}
          {item.uncommitted > 0 ? ` · ${item.uncommitted} uncommitted` : ""}
        </span>
      ) : null}
    </span>
  );

  const onClick = useCallback(
    () => appCommands.revealRepo({ repo: item.name }),
    [item.name],
  );

  return (
    <Tooltip label={tooltip} placement="right">
      <button
        type="button"
        className={`rail__sigil ${toneClass}${pulse ? " rail__sigil--pulse" : ""}`}
        onClick={onClick}
        aria-label={`Jump to ${item.name}`}
      >
        <span className="rail__sigil-letter">{letter}</span>
        {item.unread ? <span className="rail__sigil-unread" aria-hidden /> : null}
      </button>
    </Tooltip>
  );
}
