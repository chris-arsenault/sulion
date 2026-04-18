// Shown in place of the TerminalPane when the selected session isn't
// `live`. Orphaned sessions offer a one-click Resume (spawns a new PTY
// that boots straight into `claude --resume <old-uuid>`). Dead/deleted
// sessions just offer cleanup. The timeline pane is unaffected — the
// Claude session's events remain in Postgres.

import { useState } from "react";

import type { SessionView } from "../api/types";
import { useSessions } from "../state/SessionStore";
import "./SessionEndedPane.css";

interface Props {
  session: SessionView;
}

export function SessionEndedPane({ session }: Props) {
  const { createSession, deleteSession, selectSession } = useSessions();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const canResume =
    session.state === "orphaned" && session.current_claude_session_uuid != null;

  const onResume = async () => {
    if (!session.current_claude_session_uuid) return;
    setBusy(true);
    setError(null);
    try {
      await createSession({
        repo: session.repo,
        working_dir: session.working_dir,
        claude_resume_uuid: session.current_claude_session_uuid,
      });
      // createSession already selects the new session via the store.
    } catch (e) {
      setError(e instanceof Error ? e.message : "resume failed");
    } finally {
      setBusy(false);
    }
  };

  const onDelete = async () => {
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
  };

  const title = (() => {
    if (session.state === "orphaned") return "Session orphaned";
    if (session.state === "dead") return "Session ended";
    return "Session unavailable";
  })();

  const explanation = (() => {
    if (session.state === "orphaned") {
      return "The shell was running when the backend restarted. Its process is gone and the live terminal buffer is lost, but the Claude transcript is preserved in the timeline below.";
    }
    if (session.state === "dead") {
      return `The shell exited${
        session.exit_code != null ? ` with code ${session.exit_code}` : ""
      }. No live terminal to attach to.`;
    }
    return "This session is no longer available.";
  })();

  return (
    <div className="sep" data-testid="session-ended-pane">
      <div className="sep__card">
        <div className={`sep__badge sep__badge--${session.state}`}>
          {session.state}
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
          {session.current_claude_session_uuid && (
            <div>
              <dt>Last Claude session</dt>
              <dd>
                <code>{session.current_claude_session_uuid.slice(0, 8)}</code>
              </dd>
            </div>
          )}
        </dl>

        <div className="sep__actions">
          {canResume && (
            <button
              type="button"
              className="sep__btn sep__btn--primary"
              onClick={onResume}
              disabled={busy}
              title="Spawn a new PTY and run `claude --resume` against this Claude session"
            >
              {busy ? "Resuming…" : "Resume with new PTY"}
            </button>
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
