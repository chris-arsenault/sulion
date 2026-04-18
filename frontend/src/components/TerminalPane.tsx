// Terminal pane placeholder for #7 — real xterm.js wiring lands in #9.

import "./TerminalPane.css";

export function TerminalPane({ sessionId }: { sessionId: string }) {
  return (
    <div className="terminal-pane" data-testid="terminal-pane">
      <div className="terminal-pane__placeholder">
        terminal for session {sessionId.slice(0, 8)}… (pending #9)
      </div>
    </div>
  );
}
