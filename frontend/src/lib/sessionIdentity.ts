import type { SessionView } from "../api/types";

type SessionNames = Pick<SessionView, "id" | "label" | "agent_label">;

export function sessionIdentity(session: SessionNames) {
  const userName = session.label?.trim();
  const agentName = session.agent_label?.trim();
  return {
    primary: userName || agentName || session.id.slice(0, 8),
    secondary:
      userName && agentName && userName !== agentName ? agentName : null,
  };
}

export function sessionName(session: SessionNames): string {
  return sessionIdentity(session).primary;
}
