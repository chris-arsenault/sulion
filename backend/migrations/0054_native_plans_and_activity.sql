-- Repo-scoped published plans plus PTY activity state.
--
-- Published plans are intentionally separate from agent transcript/internal
-- planning data. Attachments answer which live PTY is currently working on a
-- plan; events preserve meaningful state transitions without making the
-- mutable tables event-sourced.

CREATE TABLE plans (
    id UUID PRIMARY KEY,
    repo_name TEXT NOT NULL,
    title TEXT NOT NULL,
    summary TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'paused', 'completed', 'canceled')),
    revision BIGINT NOT NULL DEFAULT 1,
    created_by_pty_id UUID REFERENCES pty_sessions(id) ON DELETE SET NULL,
    created_by_agent_session_uuid UUID REFERENCES claude_sessions(session_uuid) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    closed_at TIMESTAMPTZ
);

CREATE INDEX plans_repo_open_updated_idx
    ON plans(repo_name, updated_at DESC)
    WHERE status IN ('active', 'paused');

CREATE INDEX plans_repo_updated_idx
    ON plans(repo_name, updated_at DESC);

CREATE TABLE plan_phases (
    id UUID PRIMARY KEY,
    plan_id UUID NOT NULL REFERENCES plans(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position > 0),
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'in_progress', 'blocked', 'completed', 'skipped')),
    status_note TEXT,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT plan_phases_plan_position_key
        UNIQUE (plan_id, position) DEFERRABLE INITIALLY IMMEDIATE
);

CREATE INDEX plan_phases_plan_status_position_idx
    ON plan_phases(plan_id, status, position);

CREATE TABLE plan_attachments (
    id UUID PRIMARY KEY,
    plan_id UUID NOT NULL REFERENCES plans(id) ON DELETE CASCADE,
    pty_session_id UUID NOT NULL REFERENCES pty_sessions(id) ON DELETE CASCADE,
    agent_session_uuid UUID REFERENCES claude_sessions(session_uuid) ON DELETE SET NULL,
    attached_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    detached_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX plan_attachments_active_pty_uidx
    ON plan_attachments(pty_session_id)
    WHERE detached_at IS NULL;

CREATE UNIQUE INDEX plan_attachments_active_plan_pty_uidx
    ON plan_attachments(plan_id, pty_session_id)
    WHERE detached_at IS NULL;

CREATE INDEX plan_attachments_plan_active_idx
    ON plan_attachments(plan_id, attached_at)
    WHERE detached_at IS NULL;

CREATE TABLE plan_events (
    id BIGSERIAL PRIMARY KEY,
    plan_id UUID NOT NULL REFERENCES plans(id) ON DELETE CASCADE,
    phase_id UUID REFERENCES plan_phases(id) ON DELETE SET NULL,
    event_type TEXT NOT NULL,
    actor_kind TEXT NOT NULL CHECK (actor_kind IN ('agent', 'user', 'system')),
    pty_session_id UUID REFERENCES pty_sessions(id) ON DELETE SET NULL,
    agent_session_uuid UUID REFERENCES claude_sessions(session_uuid) ON DELETE SET NULL,
    from_status TEXT,
    to_status TEXT,
    note TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX plan_events_plan_created_idx
    ON plan_events(plan_id, created_at DESC, id DESC);

CREATE TABLE session_activity_state (
    pty_session_id UUID PRIMARY KEY REFERENCES pty_sessions(id) ON DELETE CASCADE,
    state TEXT NOT NULL
        CHECK (state IN ('working', 'awaiting_prompt', 'needs_input', 'blocked', 'unknown')),
    summary TEXT,
    reason TEXT,
    source TEXT NOT NULL
        CHECK (source IN ('launcher', 'hook', 'ingester', 'agent', 'user')),
    confidence TEXT NOT NULL
        CHECK (confidence IN ('explicit', 'derived', 'unknown')),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX session_activity_attention_idx
    ON session_activity_state(state, updated_at DESC)
    WHERE state IN ('needs_input', 'blocked');
