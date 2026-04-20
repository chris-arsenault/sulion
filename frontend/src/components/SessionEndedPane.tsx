// Shown in place of the TerminalPane when the selected session isn't
// `live`. Orphaned supported-agent sessions offer a one-click Resume
// into a fresh PTY. Dead/deleted sessions just offer cleanup.

import { useCallback, useState } from "react";

import type { SessionView } from "../api/types";
import { useSessions } from "../state/SessionStore";
import { useTabs } from "../state/TabStore";
import { Icon } from "../icons";
import { Tooltip } from "./ui";
import "./SessionEndedPane.css";

interface Props {
  session: SessionView;
}

export function SessionEndedPane({ session }: Props) {
  const createSession = useSessions((store) => store.createSession);
  const deleteSession = useSessions((store) => store.deleteSession);
  const selectSession = useSessions((store) => store.selectSession);
  const openTab = useTabs((store) => store.openTab);
  const rebindSessionTabs = useTabs((store) => store.rebindSessionTabs);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const resumeAgent = session.current_session_agent;

  const canResume =
    session.state === "orphaned" &&
    session.current_session_uuid != null &&
    (resumeAgent === "claude-code" || resumeAgent === "codex");

  const onResume = useCallback(async () => {
    if (!session.current_session_uuid) return;
    setBusy(true);
    setError(null);
    try {
      const created = await createSession({
        repo: session.repo,
        working_dir: session.working_dir,
        resume_session_uuid: session.current_session_uuid,
        resume_agent: resumeAgent ?? "claude-code",
      });
      rebindSessionTabs(session.id, created.id);
      openTab({ kind: "terminal", sessionId: created.id }, "top");
      openTab({ kind: "timeline", sessionId: created.id }, "bottom");
      selectSession(created.id);
      await deleteSession(session.id);
    } catch (e) {
      setError(e instanceof Error ? e.message : "resume failed");
    } finally {
      setBusy(false);
    }
  }, [
    createSession,
    session.current_session_uuid,
    session.id,
    session.repo,
    session.working_dir,
    resumeAgent,
    openTab,
    rebindSessionTabs,
    selectSession,
    deleteSession,
  ]);

  const onDelete = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      await deleteSession(session.id);
      selectSession(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : "delete failed");
    } finally {
      setBusy(false);
    }
  }, [deleteSession, selectSession, session.id]);

  const title = (() => {
    if (session.state === "orphaned") return "Session orphaned";
    if (session.state === "dead") return "Session ended";
    return "Session unavailable";
  })();

  const explanation = (() => {
    if (session.state === "orphaned") {
      return "The shell was running when the backend restarted. Its process is gone and the live terminal buffer is lost, but the transcript is preserved in the timeline below.";
    }
    if (session.state === "dead") {
      return `The shell exited${
        session.exit_code != null ? ` with code ${session.exit_code}` : ""
      }. No live terminal to attach to.`;
    }
    return "This session is no longer available.";
  })();

  const resumeTitle = (() => {
    if (resumeAgent === "codex") {
      return "Spawn a new PTY and run `sulion-agent --type codex --mode real -- resume` against this Codex session";
    }
    return "Spawn a new PTY and run `sulion-agent --type claude --mode real -- --dangerously-skip-permissions --resume` against this Claude session";
  })();

  const sigil =
    session.state === "orphaned"
      ? "session-orphan"
      : session.state === "dead"
        ? "session-dead"
        : "session-dead";

  return (
    <div className="sep" data-testid="session-ended-pane">
      <div className="sep__card">
        <div className={`sep__badge sep__badge--${session.state}`}>
          <Icon name={sigil} size={14} />
          <span>{session.state}</span>
        </div>
        <h2 className="sep__title">{title}</h2>
        <p className="sep__message">{explanation}</p>

        <dl className="sep__meta">
          <div>
            <dt>Repo</dt>
            <dd>{session.repo}</dd>
          </div>
          <div>
            <dt>Working dir</dt>
            <dd>
              <code>{session.working_dir}</code>
            </dd>
          </div>
          {session.current_session_uuid && (
            <div>
              <dt>Last session</dt>
              <dd>
                <code>
                  {(session.current_session_agent ?? "session")}{" "}
                  {session.current_session_uuid.slice(0, 8)}
                </code>
              </dd>
            </div>
          )}
        </dl>

        <div className="sep__actions">
          {canResume && (
            <Tooltip label={resumeTitle}>
              <button
                type="button"
                className="sep__btn sep__btn--primary"
                onClick={onResume}
                disabled={busy}
              >
                {busy ? "Resuming…" : "Resume with new PTY"}
              </button>
            </Tooltip>
          )}
          <button
            type="button"
            className="sep__btn sep__btn--destructive"
            onClick={onDelete}
            disabled={busy}
          >
            Delete
          </button>
        </div>

        {error && <div className="sep__error">{error}</div>}
      </div>
    </div>
  );
}
