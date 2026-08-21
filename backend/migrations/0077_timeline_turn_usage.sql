-- Per-turn token accounting, projected from event payloads (claude
-- per-message usage, codex cumulative deltas) at projection time.

ALTER TABLE timeline_turns
    ADD COLUMN IF NOT EXISTS input_tokens BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS output_tokens BIGINT NOT NULL DEFAULT 0;
