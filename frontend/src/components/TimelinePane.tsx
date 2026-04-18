// Timeline pane placeholder for #7 — virtualized blocks land in #10.

import "./TimelinePane.css";

export function TimelinePane({ sessionId }: { sessionId: string }) {
  return (
    <div className="timeline-pane" data-testid="timeline-pane">
      <div className="timeline-pane__placeholder">
        timeline for session {sessionId.slice(0, 8)}… (pending #10)
      </div>
    </div>
  );
}
