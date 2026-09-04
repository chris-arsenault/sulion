-- Arbitrary-depth branch plans.
--
-- A sub-plan is an ordinary plan with a parent and one or more anchor phases
-- in that parent. Nesting lives at the plan level so a branch keeps its own
-- title, status, revision, event history, and PTY attachment; every existing
-- plan operation works on a branch unchanged.
--
-- root_plan_id and depth are denormalized at creation. Parentage is immutable,
-- so they never drift and no recursive CTE is needed on the app-state poll
-- path. A root plan carries parent_plan_id NULL, root_plan_id = id, depth 0.

ALTER TABLE plans
    ADD COLUMN parent_plan_id UUID REFERENCES plans(id) ON DELETE RESTRICT,
    ADD COLUMN root_plan_id UUID REFERENCES plans(id) ON DELETE RESTRICT,
    ADD COLUMN depth INTEGER NOT NULL DEFAULT 0 CHECK (depth >= 0);

UPDATE plans SET root_plan_id = id WHERE root_plan_id IS NULL;

ALTER TABLE plans
    ALTER COLUMN root_plan_id SET NOT NULL;

-- A root is its own root at depth 0; a branch points elsewhere below depth 0.
ALTER TABLE plans
    ADD CONSTRAINT plans_root_matches_parentage CHECK (
        (parent_plan_id IS NULL AND root_plan_id = id AND depth = 0)
        OR (parent_plan_id IS NOT NULL AND root_plan_id <> id AND depth > 0)
    );

CREATE INDEX plans_parent_idx ON plans(parent_plan_id) WHERE parent_plan_id IS NOT NULL;

CREATE INDEX plans_root_open_idx
    ON plans(root_plan_id)
    WHERE status IN ('active', 'paused');

-- Anchor phases: which parent steps this branch covers. Multiple rows let one
-- branch cover a span (parent steps 4-6). Every anchor must belong to the
-- branch's parent plan; the service enforces that, since a CHECK cannot join.
CREATE TABLE plan_branch_anchors (
    plan_id UUID NOT NULL REFERENCES plans(id) ON DELETE CASCADE,
    parent_phase_id UUID NOT NULL REFERENCES plan_phases(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (plan_id, parent_phase_id)
);

CREATE INDEX plan_branch_anchors_phase_idx ON plan_branch_anchors(parent_phase_id);
