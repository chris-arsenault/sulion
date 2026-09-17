-- Model switches observed in a transcript session. Both harnesses can move
-- a session onto a different model without the user asking: Codex applies
-- new thread settings between turns and starts the next turn on the new
-- model with a `<model_switch>` developer note; Claude Code records a
-- `fallback` content block mid-turn when the primary model's request is
-- retried on the fallback model. Neither writes a reason.
--
-- The detector keeps two baselines per session: `observed_model` is the last
-- model any transcript record ran on, and `confirmed_model` is the model the
-- user launched with or later accepted in the timeline's switch dialog. A
-- change from the observed model records a row; a change away from the
-- confirmed model is `enforced`, which stops the turn running under it and
-- opens the dialog. Returning to the confirmed model is recorded but never
-- enforced, so restoring the model by hand does not trigger the guard.

ALTER TABLE agent_session_metadata
    ADD COLUMN observed_model TEXT,
    ADD COLUMN observed_effort TEXT,
    ADD COLUMN confirmed_model TEXT;

CREATE TABLE agent_model_switches (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_uuid UUID NOT NULL REFERENCES claude_sessions(session_uuid) ON DELETE CASCADE,
    agent TEXT NOT NULL,
    byte_offset BIGINT NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL,
    detected_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Which transcript record revealed the change:
    -- codex_thread_settings | codex_turn_context | claude_fallback | claude_message
    source TEXT NOT NULL,
    from_model TEXT,
    to_model TEXT NOT NULL,
    from_effort TEXT,
    to_effort TEXT,
    turn_id TEXT,
    -- A turn was underway when the change was observed.
    turn_in_flight BOOLEAN NOT NULL DEFAULT FALSE,
    -- The new model differs from the confirmed one: stop the turn, ask.
    enforced BOOLEAN NOT NULL,
    -- Whatever the transcript held near the switch that may explain it:
    -- the rate-limit snapshot Codex last reported, the fallback block and
    -- request iterations Claude wrote.
    context JSONB NOT NULL DEFAULT '{}'::jsonb,
    interrupted_at TIMESTAMPTZ,
    interrupt_error TEXT,
    acknowledged_at TIMESTAMPTZ,
    -- Acknowledged by choosing to continue on the new model.
    adopted BOOLEAN,
    UNIQUE (session_uuid, byte_offset)
);

CREATE INDEX agent_model_switches_open_idx
    ON agent_model_switches (session_uuid, observed_at)
    WHERE acknowledged_at IS NULL;

CREATE INDEX agent_model_switches_recent_idx
    ON agent_model_switches (session_uuid, observed_at DESC);
