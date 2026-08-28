-- Code Mode shell calls canonicalize to their executable so the timeline
-- carries useful operation identity. Read-only inspection programs share
-- the same category as native read/grep/glob tools; unlisted executables
-- fall back to the nested shell tool's utility category during projection.

INSERT INTO tool_category_rules
    (match_kind, pattern, operation_type, operation_category, precedence)
VALUES
    ('exact', 'rg', 'rg', 'inspect', 10),
    ('exact', 'sed', 'sed', 'inspect', 10),
    ('exact', 'nl', 'nl', 'inspect', 10),
    ('exact', 'wc', 'wc', 'inspect', 10),
    ('exact', 'cat', 'cat', 'inspect', 10),
    ('exact', 'ls', 'ls', 'inspect', 10),
    ('exact', 'head', 'head', 'inspect', 10),
    ('exact', 'tail', 'tail', 'inspect', 10),
    ('exact', 'pwd', 'pwd', 'inspect', 10),
    ('exact', 'jq', 'jq', 'inspect', 10),
    ('exact', 'ps', 'ps', 'inspect', 10)
ON CONFLICT (match_kind, pattern) DO UPDATE
SET operation_type = EXCLUDED.operation_type,
    operation_category = EXCLUDED.operation_category,
    precedence = EXCLUDED.precedence;
