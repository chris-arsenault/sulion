ALTER TABLE tool_category_rules DROP CONSTRAINT tool_category_rules_operation_category_check;
ALTER TABLE tool_category_rules ADD CONSTRAINT tool_category_rules_operation_category_check
    CHECK (operation_category IN ('create_content', 'inspect', 'utility', 'research',
                                 'delegate', 'workflow', 'plan', 'other'));

INSERT INTO tool_category_rules
    (match_kind, pattern, operation_type, operation_category, precedence)
VALUES ('exact', 'sulion_plan', 'sulion_plan', 'plan', 10)
ON CONFLICT (match_kind, pattern) DO UPDATE
SET operation_type = EXCLUDED.operation_type,
    operation_category = EXCLUDED.operation_category,
    precedence = EXCLUDED.precedence;
