# ADR 0022: Unconfirmed Model Switches Interrupt the Turn and Ask in the Timeline

## Status

Accepted, 2026-09-17.

## Context

Both harnesses can move a session onto a different model without being
asked. Codex applies new thread settings between turns
(`thread_settings_applied`) and starts the next turn on the new model with a
`<model_switch>` developer note. Claude Code writes a `fallback` content
block mid-turn when the primary model's request is retried on a fallback
model. Neither records a reason. The closest evidence either leaves behind
is Codex's last rate-limit snapshot and Claude's fallback block with its
request iterations.

Until this decision the timeline showed the model only as a metadata line
on the turn. The user reported on 2026-09-17 that sessions in two
repositories had been downgraded the same day, that the change was hard to
notice "since it's just a display", and asked that a mid-turn model change
stop the turn and open a confirmation on the timeline with whatever
reasoning the data holds.

The ingester already observed every model-bearing record, the control
process already had an interrupt path to a live PTY through the node
protocol (the prompt bar's Interrupt button), and the app-state poll already
carried per-session flags to the timeline pane. The guard had to fit those
three surfaces.

## Decision

The ingester keeps two models per session on `agent_session_metadata`:
`observed_model`, the model the last record ran on, and `confirmed_model`,
the model the session launched with or the user later accepted. Every
model-bearing record is compared against both. A change from the observed
model writes an `agent_model_switches` row (migration 0085); a change away
from the confirmed model marks that row `enforced`. The row stores the
evidence the transcript held near the switch, because that is all either
harness writes.

Enforcement is a control-process loop in
`backend/src/api/model_switch_routes.rs`. It sends the interrupt key to any
live PTY whose running turn belongs to an enforced, unacknowledged switch,
through the same node request the Interrupt button uses. The app-state poll
carries the oldest such switch per PTY as `pending_model_switch`, and the
timeline pane opens a confirmation dialog on it showing the models, effort,
when the change was observed, whether a turn was interrupted, and the
transcript evidence.

*Continue* adopts the new model as `confirmed_model`. *Dismiss* keeps the
previous model expected: the switch stays recorded and the harness is not
switched back for the user. If the harness runs the next turn on the
unconfirmed model again, that turn is interrupted again. Routes are `GET
/api/sessions/:id/model-switches` and `POST
/api/sessions/:id/model-switches/:switch_id/acknowledge`. Detection lives in
`backend/src/model_switches.rs`.

Two adjacent fixes landed with the guard because they were prerequisites
for it to be correct: Codex `turn_aborted` maps to awaiting-prompt, and
synthetic API error records no longer overwrite a Claude session's reported
model.

## Alternatives considered

- **Keep the metadata line only.** The prior behaviour. Rejected because
  the user could not see the downgrade in time to act on it; the commit
  body for `ef35e9f` records that the timeline showed the change "only as a
  metadata line", and the user's request (session
  `43a00fd8-d6f3-4eec-9bab-e24a8b8202e0`, turn 517) names that as the
  problem.
- **Record the switch and adopt it silently.** Detection without
  interruption or a dialog would have given history a row but left the turn
  running on the wrong model. The user asked for the turn to be stopped and
  a confirmation shown; a silent record does neither.
- **Force the harness back to the previous model.** Rejected because
  Sulion has no supported way to change a harness's model from outside the
  session, and faking one would mean typing commands into the PTY. The
  commit body states the chosen semantics directly: Dismiss "keeps the
  previous one expected so restoring it by hand is not enforced", and the
  user guide says Dismiss "does not switch the harness back".

## Consequences

- A downgraded turn stops within one control-loop tick of the ingester
  seeing the switch, so work on the wrong model is bounded by the ingest
  latency rather than the turn length.
- The dialog can only show the evidence the transcript holds. For Codex
  that is a rate-limit snapshot; for Claude a fallback block. Neither
  states a reason, and the dialog says so.
- Dismiss is a record, not a remedy. The user restores the model in the
  harness; until they do, every turn on the unconfirmed model is
  interrupted, which is the intended pressure.
- Model switches are per-session rows and are deleted by the archive purge
  with the rest of a session's events; the digest keeps the turn, not the
  switch.
- `confirmed_model` is a second notion of "the session's model" beside the
  harness's own report. Code that reports a session's model has to pick one
  deliberately; the commit's fix for synthetic API error records was the
  first case where that mattered.

## References

- Shipped mechanism: [`../architecture.md`](../architecture.md), "Model
  switches"
- User-facing behaviour: [`../user-guide.md`](../user-guide.md)
- Change record: `CHANGELOG.md`, Unreleased, "Added a model-switch guard"
- Commit: `ef35e9f` — Stop turns on unconfirmed model switches and ask in
  the timeline
- Request: transcript session `43a00fd8-d6f3-4eec-9bab-e24a8b8202e0`,
  turn 517
