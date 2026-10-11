import { useCallback, useMemo, useState, type FormEvent } from "react";

import type {
  AgentLaunchType,
  CreateSessionRequest,
  MetaRepoView,
  RepoView,
  SessionView,
} from "../api/types";
import { Icon } from "../icons";
import { Overlay } from "./ui";

export type NewSessionFormValue = {
  working_dir: string;
  launch_agent: AgentLaunchType | "";
  workspace_mode: "isolated" | "main";
};

type SessionTarget = {
  name: string;
  repoName: string;
  metaRepoId?: string;
  available: boolean;
};

export function SessionCreationDialog({
  repos,
  metaRepos,
  initialMetaRepoId,
  createSession,
  onCreated,
  onClose,
}: {
  repos: RepoView[];
  metaRepos: MetaRepoView[];
  initialMetaRepoId?: string;
  createSession: (request: CreateSessionRequest) => Promise<SessionView>;
  onCreated: (id: string) => void;
  onClose: () => void;
}) {
  const targets = useMemo<SessionTarget[]>(
    () =>
      [
        ...metaRepos.map((meta) => ({
          name: meta.name,
          repoName: meta.primary_repo_name ?? meta.name,
          metaRepoId: meta.id,
          available:
            Boolean(meta.primary_repo_name) &&
            meta.members.length > 0 &&
            meta.members.every((member) => member.exists),
        })),
        ...repos.map((repo) => ({
          name: repo.name,
          repoName: repo.name,
          available: repo.exists !== false,
        })),
      ].sort((a, b) => a.name.localeCompare(b.name)),
    [metaRepos, repos],
  );
  const [target, setTarget] = useState<SessionTarget | null>(
    () =>
      targets.find(
        (candidate) =>
          candidate.metaRepoId === initialMetaRepoId &&
          initialMetaRepoId !== undefined &&
          candidate.available,
      ) ?? null,
  );
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const onQueryChange = useCallback(
    (event: React.ChangeEvent<HTMLInputElement>) =>
      setQuery(event.target.value),
    [],
  );
  const chooseTarget = useCallback(
    (event: React.MouseEvent<HTMLButtonElement>) => {
      const chosen = targets.find(
        (candidate) =>
          `${candidate.metaRepoId ? "meta" : "repo"}:${candidate.metaRepoId ?? candidate.repoName}` ===
          event.currentTarget.dataset.target,
      );
      if (chosen?.available) setTarget(chosen);
    },
    [targets],
  );
  const back = useCallback(() => {
    setTarget(null);
    setError(null);
  }, []);
  const close = useCallback(() => {
    if (!pending) onClose();
  }, [pending, onClose]);
  const submit = useCallback(
    async (form: NewSessionFormValue) => {
      if (!target?.available || pending) return;
      setPending(true);
      setError(null);
      try {
        const session = await createSession({
          ...(target.metaRepoId
            ? { meta_repo_id: target.metaRepoId }
            : { repo: target.repoName }),
          workspace_mode: target.metaRepoId ? "main" : form.workspace_mode,
          working_dir: form.working_dir.trim() || undefined,
          launch_agent: form.launch_agent || undefined,
        });
        onClose();
        onCreated(session.id);
      } catch (err) {
        setError(
          err instanceof Error ? err.message : "Could not start session.",
        );
      } finally {
        setPending(false);
      }
    },
    [target, pending, createSession, onClose, onCreated],
  );
  const filtered = targets.filter((candidate) =>
    candidate.name.toLowerCase().includes(query.trim().toLowerCase()),
  );

  return (
    <Overlay
      open
      modal
      title="New session"
      aria-label="New session"
      onClose={close}
      className="session-creation-dialog"
    >
      {target ? (
        <>
          <button
            type="button"
            className="sidebar__text-button"
            onClick={back}
            disabled={pending}
          >
            Change repository
          </button>
          <div className="sidebar__creation-target">{target.name}</div>
          {error && (
            <div className="sidebar__error" role="alert">
              {error}
            </div>
          )}
          <NewSessionForm
            repoName={target.repoName}
            mainOnly={Boolean(target.metaRepoId)}
            onSubmit={submit}
            onCancel={close}
            pending={pending}
          />
        </>
      ) : (
        <>
          <input
            className="sidebar__search"
            type="search"
            aria-label="Search repositories"
            placeholder="Search repositories…"
            value={query}
            onChange={onQueryChange}
            autoFocus
          />
          <div className="sidebar__targets">
            {filtered.map((candidate) => (
              <button
                key={`${candidate.metaRepoId ? "meta" : "repo"}:${candidate.metaRepoId ?? candidate.repoName}`}
                type="button"
                className="sidebar__target"
                data-target={`${candidate.metaRepoId ? "meta" : "repo"}:${candidate.metaRepoId ?? candidate.repoName}`}
                onClick={chooseTarget}
                disabled={!candidate.available}
              >
                <Icon
                  name={candidate.metaRepoId ? "layers" : "folder"}
                  size={14}
                />
                <span>{candidate.name}</span>
                <span className="sidebar__target-kind">
                  {candidate.metaRepoId ? "Collection" : "Repository"}
                  {!candidate.available ? " · unavailable" : ""}
                </span>
              </button>
            ))}
            {filtered.length === 0 && (
              <div className="sidebar__muted">No matching repositories.</div>
            )}
          </div>
        </>
      )}
    </Overlay>
  );
}

export function NewSessionForm({
  repoName,
  mainOnly = false,
  onSubmit,
  onCancel,
  pending = false,
}: {
  repoName: string;
  mainOnly?: boolean;
  onSubmit: (form: NewSessionFormValue) => void;
  onCancel: () => void;
  pending?: boolean;
}) {
  const [workingDir, setWorkingDir] = useState("");
  const [launchAgent, setLaunchAgent] = useState<AgentLaunchType | "">("");
  const [workspaceMode, setWorkspaceMode] = useState<
    NewSessionFormValue["workspace_mode"]
  >(mainOnly ? "main" : "isolated");
  const submit = useCallback(
    (event: FormEvent) => {
      event.preventDefault();
      if (!pending)
        onSubmit({
          working_dir: workspaceMode === "main" ? workingDir : "",
          launch_agent: launchAgent,
          workspace_mode: workspaceMode,
        });
    },
    [pending, onSubmit, workspaceMode, workingDir, launchAgent],
  );
  const onInputChange = useCallback(
    (event: React.ChangeEvent<HTMLInputElement>) =>
      setWorkingDir(event.target.value),
    [],
  );
  const onAgentChange = useCallback(
    (event: React.ChangeEvent<HTMLSelectElement>) =>
      setLaunchAgent(event.target.value as AgentLaunchType | ""),
    [],
  );
  const onWorkspaceModeChange = useCallback(
    (event: React.ChangeEvent<HTMLSelectElement>) =>
      setWorkspaceMode(
        event.target.value as NewSessionFormValue["workspace_mode"],
      ),
    [],
  );
  const onInputKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLInputElement>) => {
      if (event.key === "Escape") onCancel();
    },
    [onCancel],
  );
  return (
    <form className="sidebar__form" onSubmit={submit}>
      <input
        type="text"
        value={workingDir}
        onChange={onInputChange}
        onKeyDown={onInputKeyDown}
        placeholder={
          workspaceMode === "main"
            ? `working dir (default: repos/${repoName})`
            : "working dir is the isolated workspace root"
        }
        disabled={pending || workspaceMode !== "main"}
        autoFocus
        aria-label="working directory"
      />
      {!mainOnly && (
        <select
          value={workspaceMode}
          onChange={onWorkspaceModeChange}
          aria-label="workspace mode"
          disabled={pending}
          autoFocus
        >
          <option value="isolated">Isolated worktree</option>
          <option value="main">Main working tree</option>
        </select>
      )}
      <select
        value={launchAgent}
        onChange={onAgentChange}
        aria-label="launch agent"
        disabled={pending}
      >
        <option value="">Shell only</option>
        <option value="claude">Launch Claude</option>
        <option value="codex">Launch Codex</option>
        <option value="fugu">Launch Fugu</option>
      </select>
      <div className="sidebar__form-actions">
        <button type="button" onClick={onCancel} disabled={pending}>
          Cancel
        </button>
        <button type="submit" disabled={pending}>
          {pending ? "Starting…" : "Spawn"}
        </button>
      </div>
    </form>
  );
}
