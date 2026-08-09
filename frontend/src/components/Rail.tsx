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

/** Left rail — functional. Lists top-level repository scopes with a staleness ring
 * + unread dot. Click scrolls the sidebar to that scope and expands it.
 * Also carries pin toggle and command palette trigger. */
export function Rail({
  pinned,
  onTogglePinned,
  onOpenMonitor,
  onOpenSecrets,
  onOpenPalette,
}: RailProps) {
  const { repos, metaRepos, sessions, isUnread } = useSessions(
    useShallow((store) => ({
      repos: store.repos,
      metaRepos: store.metaRepos,
      sessions: store.sessions,
      isUnread: store.isUnread,
    })),
  );
  const repoStates = useRepos((store) => store.repos);

  const items = useMemo(() => {
    const byRepo = new Map<string, { unread: boolean; latest: number | null }>();
    for (const s of sessions) {
      if (s.meta_repo) continue;
      const entry = byRepo.get(s.repo) ?? { unread: false, latest: null };
      if (isUnread(s.id, s.last_event_at)) entry.unread = true;
      if (s.last_event_at) {
        const t = new Date(s.last_event_at).getTime();
        if (entry.latest === null || t > entry.latest) entry.latest = t;
      }
      byRepo.set(s.repo, entry);
    }
    const assigned = new Set(
      metaRepos.flatMap((metaRepo) =>
        metaRepo.members.map((member) => member.repo_name),
      ),
    );
    const metaItems: RailRepoItem[] = metaRepos.map((metaRepo) => {
      const memberNames = metaRepo.members.map((member) => member.repo_name);
      const memberSet = new Set(memberNames);
      const collectionSessions = sessions.filter(
        (session) => session.meta_repo?.id === metaRepo.id,
      );
      const collectionUnread = collectionSessions.some((session) =>
        isUnread(session.id, session.last_event_at),
      );
      const collectionLatest = latestSessionEvent(collectionSessions);
      let staleness: RailRepoItem["staleness"] = "green";
      let uncommitted = 0;
      let unread = collectionUnread;
      for (const repoName of memberSet) {
        const state = byRepo.get(repoName);
        unread ||= state?.unread ?? false;
        const git = repoStates[repoName]?.git ?? null;
        uncommitted += git?.uncommitted_count ?? 0;
        staleness = worstStaleness(
          staleness,
          stalenessFor(git, maxTimestamp(state?.latest ?? null, collectionLatest)),
        );
      }
      return {
        id: metaRepo.id,
        kind: "meta" as const,
        name: metaRepo.name,
        unread,
        staleness,
        branch: null,
        uncommitted,
        memberCount: memberNames.length,
      };
    });
    const repoItems: RailRepoItem[] = repos
      .filter((repo) => !assigned.has(repo.name))
      .slice()
      .sort((a, b) => a.name.localeCompare(b.name))
      .map((r) => {
        const st = byRepo.get(r.name) ?? { unread: false, latest: null };
        const git = repoStates[r.name]?.git ?? null;
        const staleness = stalenessFor(git, st.latest);
        return {
          id: r.name,
          kind: "repo" as const,
          name: r.name,
          unread: st.unread,
          staleness,
          branch: git?.branch ?? null,
          uncommitted: git?.uncommitted_count ?? 0,
          memberCount: 1,
        };
      });
    return [...metaItems, ...repoItems];
  }, [metaRepos, repos, sessions, repoStates, isUnread]);

  return (
    <nav className="rail" aria-label="Repositories">
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
          items.map((it) => <RailRepo key={`${it.kind}:${it.id}`} item={it} />)
        )}
      </div>

      <div className="rail__spacer" />

      <Tooltip label="Overview of open terminals" placement="right">
        <button
          type="button"
          className="rail__icon"
          onClick={onOpenMonitor}
          aria-label="Open overview"
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
  id: string;
  kind: "repo" | "meta";
  name: string;
  unread: boolean;
  staleness: "green" | "amber" | "red";
  branch: string | null;
  uncommitted: number;
  memberCount: number;
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
      {item.kind === "meta" ? (
        <span className="rail__tip-meta">
          {item.memberCount} {item.memberCount === 1 ? "repository" : "repositories"}
          {item.uncommitted > 0 ? ` · ${item.uncommitted} uncommitted` : ""}
        </span>
      ) : item.branch ? (
        <span className="rail__tip-meta">
          {item.branch}
          {item.uncommitted > 0 ? ` · ${item.uncommitted} uncommitted` : ""}
        </span>
      ) : null}
    </span>
  );

  const onClick = useCallback(
    () => {
      if (item.kind === "meta") {
        appCommands.revealMetaRepo({ metaRepoId: item.id });
      } else {
        appCommands.revealRepo({ repo: item.name });
      }
    },
    [item.id, item.kind, item.name],
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

function latestSessionEvent(sessions: Array<{ last_event_at: string | null }>) {
  let latest: number | null = null;
  for (const session of sessions) {
    if (!session.last_event_at) continue;
    const timestamp = new Date(session.last_event_at).getTime();
    if (latest === null || timestamp > latest) latest = timestamp;
  }
  return latest;
}

function maxTimestamp(a: number | null, b: number | null): number | null {
  if (a === null) return b;
  if (b === null) return a;
  return Math.max(a, b);
}

function worstStaleness(
  a: RailRepoItem["staleness"],
  b: RailRepoItem["staleness"],
): RailRepoItem["staleness"] {
  const rank = { green: 0, amber: 1, red: 2 } as const;
  return rank[a] >= rank[b] ? a : b;
}
