import { useCallback } from "react";

import type { CreateSessionRequest, SessionView } from "../api/types";
import { useSessions } from "../state/SessionStore";
import { useTabs } from "../state/TabStore";

/** The app's resumable rule, extracted verbatim from SessionEndedPane:
 * orphaned (PTY lost, e.g. backend restart) with a correlated session
 * uuid from a supported agent. Dead sessions (shell exited) are not
 * resumable. */
export function isResumableSession(session: SessionView): boolean {
  return (
    session.state === "orphaned" &&
    session.current_session_uuid != null &&
    (session.current_session_agent === "claude-code" ||
      session.current_session_agent === "codex")
  );
}

/** Resume an abandoned session into a fresh PTY: spawns the new session
 * against the old transcript, carries label/pin/colour over, rebinds the
 * old session's tabs, opens terminal + timeline, and deletes the husk.
 * Returns the created session. Throws on failure — callers own busy/error
 * presentation. */
export function useResumeSession() {
  const createSession = useSessions((store) => store.createSession);
  const updateSession = useSessions((store) => store.updateSession);
  const deleteSession = useSessions((store) => store.deleteSession);
  const selectSession = useSessions((store) => store.selectSession);
  const openTab = useTabs((store) => store.openTab);
  const rebindSessionTabs = useTabs((store) => store.rebindSessionTabs);

  return useCallback(
    async (session: SessionView): Promise<SessionView> => {
      if (!session.current_session_uuid) {
        throw new Error("session has no agent transcript to resume");
      }
      const resumeRequest: CreateSessionRequest = {
        repo: session.repo,
        resume_session_uuid: session.current_session_uuid,
        resume_agent: session.current_session_agent ?? "claude-code",
      };
      const workspaceId = session.workspace?.id;
      if (workspaceId && session.workspace?.kind !== "main") {
        resumeRequest.workspace_id = workspaceId;
      } else {
        resumeRequest.workspace_mode = "main";
        resumeRequest.working_dir = session.working_dir;
      }
      const created = await createSession(resumeRequest);
      // Carry the user's customisations onto the successor so a resume
      // feels seamless; skip the round-trip when nothing is customised.
      const hasCustomisation =
        session.label != null || session.pinned || session.color != null;
      if (hasCustomisation) {
        await updateSession(created.id, {
          label: session.label,
          pinned: session.pinned,
          color: session.color,
        });
      }
      rebindSessionTabs(session.id, created.id);
      openTab({ kind: "terminal", sessionId: created.id }, "top");
      openTab({ kind: "timeline", sessionId: created.id }, "bottom");
      selectSession(created.id);
      await deleteSession(session.id);
      return created;
    },
    [
      createSession,
      updateSession,
      deleteSession,
      selectSession,
      openTab,
      rebindSessionTabs,
    ],
  );
}
