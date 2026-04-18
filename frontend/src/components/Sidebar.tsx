// Sidebar placeholder for #7. Fleshed out with repo tree + actions in #8.

import { useSessions } from "../state/SessionStore";
import "./Sidebar.css";

export function Sidebar() {
  const { sessions, repos, selectedSessionId, selectSession } = useSessions();

  return (
    <div className="sidebar">
      <div className="sidebar__header">
        <span className="sidebar__logo">shuttlecraft</span>
      </div>
      <div className="sidebar__section-title">Repos</div>
      <ul className="sidebar__list">
        {repos.length === 0 && <li className="sidebar__muted">— none —</li>}
        {repos.map((r) => (
          <li key={r.name} className="sidebar__repo">
            <span className="sidebar__repo-name">{r.name}</span>
          </li>
        ))}
      </ul>
      <div className="sidebar__section-title">Sessions</div>
      <ul className="sidebar__list">
        {sessions.length === 0 && <li className="sidebar__muted">— none —</li>}
        {sessions.map((s) => (
          <li key={s.id}>
            <button
              type="button"
              className={
                s.id === selectedSessionId
                  ? "sidebar__session sidebar__session--active"
                  : "sidebar__session"
              }
              onClick={() => selectSession(s.id)}
            >
              <span className={`sidebar__dot sidebar__dot--${s.state}`} />
              <span className="sidebar__session-repo">{s.repo}</span>
              <span className="sidebar__session-id">{s.id.slice(0, 8)}</span>
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
