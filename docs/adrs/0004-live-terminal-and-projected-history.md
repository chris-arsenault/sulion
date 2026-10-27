# ADR 0004: Live Terminal Pane and Transcript-Projected History Are Separate Pipelines

## Status

Accepted, 2026-04-18.

## Context

A PTY running Claude Code or Codex produces two kinds of output that want
different treatment. The live byte stream is a cursor-movement protocol:
the agent TUIs draw into the alternate screen, redraw spinners, and reflow
on resize. The conversation itself (prompts, assistant text, tool calls,
results) is written separately by the agent as an append-only JSONL
transcript.

Terminals are not built to replay cursor-movement streams as scrollback.
Pushing TUI redraws into a linear buffer produces visual garbage, and the
original design named "scrollback must be better than a terminal provides,
not worse" as a forcing requirement. At the same time, keystroke latency
on the live pane has to feel like SSH, which rules out routing PTY bytes
through React state.

Agent turns span minutes. The transcript lags the terminal by however long
ingestion takes, and a review surface a second or two behind the live pane
is imperceptible to the person reading it.

## Decision

Sulion keeps two rendering pipelines that never share a buffer.

The **live pane** is xterm.js mounted imperatively inside a `useEffect`
keyed on the session id. WebSocket bytes go straight to `term.write` and
`term.onData` flows straight to the connection's input sender. React
mounts and unmounts the container; it is not in the PTY-byte path. This is
invariant 2 in `docs/architecture.md`.

The server side feeds a **shadow emulator** (`vt100::Parser` behind a lock)
on every PTY read, including while no browser is attached. A reconnecting
client receives the emulator's formatted screen, including cursor position
and the alternate-screen buffer, as its first frame, then live bytes. This
is invariant 4.

**History** is the timeline pane. It renders rows projected from the
ingested JSONL in Postgres, fetched by polling the history endpoint with an
`after=<offset>` cursor so each request returns only new rows. The
ingester reads transcripts on a 500 ms tick and projects turns in the same
process; the browser poll sits on top of that. No history is derived from
the terminal buffer, and nothing in the request path reads a transcript
file (invariant 1).

The live pane originally shipped with `scrollback: 0`. Commit d96e3b3
raised it to 5000 lines because routine shell output (`npm install`,
`git diff`, test runs) is not in any transcript and was scrolling off the
viewport unrecoverably. TUI apps use the alternate screen, which xterm.js
keeps separate from scrollback, so the change does not reintroduce replayed
TUI output. Scrollback is a convenience for shell output; the timeline
remains the review surface.

## Alternatives considered

- **Replay terminal scrollback as the history view.** Rejected in the
  original design (`session-broker-design.md` at 0ae1d9d, "No
  terminal-side scrollback in the frontend") because terminals cannot
  replay cursor-movement streams into a linear buffer without producing
  garbage for any TUI application. The design called this live-versus-history
  split the key ergonomic insight.
- **React-managed terminal rendering.** Rejected by the same design
  document ("The terminal pane must live outside React's reconciliation")
  and implemented that way in 4e1fcfc, whose body states React is not in
  the PTY-byte rendering path and that the invariant is load-bearing for
  latency. The commit's tests assert that idle re-renders do not reconnect
  and only a session change remounts.
- **Postgres `LISTEN`/`NOTIFY` push for the timeline.** The original
  design chose polling "for MVP simplicity; Claude responses span minutes,
  so a 1–2 second lag is imperceptible", and 75c9372 implemented the
  1500 ms incremental poll. The ingester's own 500 ms read tick and batched
  projection bound the lag below that.
- **No client-side scrollback at all.** The first implementation
  (4e1fcfc, `scrollback: 0`). Reversed by d96e3b3 once shell output that
  no transcript records was being lost and wheel-up in a live terminal
  appeared broken.

## Consequences

- Timeline freshness is bounded by ingester lag plus the poll interval,
  not by the PTY stream. The live pane provides immediacy; the timeline
  provides reviewability.
- A reconnecting browser sees the current screen immediately because the
  shadow emulator was fed the whole time. The emulator lives next to the
  PTY masters (now in the devenv server) so that remains true across node
  restarts.
- Every history feature (filters, file traces, turn digests, retrieval) is
  built on Postgres rows, which is what made tiered retention (ADR 0003)
  and archive restore possible without touching the terminal path.
- The terminal component is tested with xterm.js mocked; its contract is
  the byte path, not rendered DOM.

## References

- [`../architecture.md`](../architecture.md) — Invariants 1, 2, 4
- [`../ingestion.md`](../ingestion.md) — Live projection
- 0ae1d9d — initial skeleton carrying `session-broker-design.md`
- 4e1fcfc — terminal pane, xterm.js outside React's reconciliation
- ca36dbd — WebSocket attach and shadow terminal emulator
- 75c9372 — virtualized timeline pane with polling
- d96e3b3 — enable 5000-line scrollback
