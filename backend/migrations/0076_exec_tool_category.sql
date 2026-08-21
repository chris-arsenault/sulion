-- Codex code-mode `exec` wraps shell commands (tools.exec_command calls
-- inside a JS snippet). Categorize it with the other shell-execution
-- tools instead of letting it fall through to 'other'.

INSERT INTO tool_category_rules (match_kind, pattern, operation_type, operation_category, precedence)
VALUES ('exact', 'exec', 'exec', 'utility', 10)
ON CONFLICT (match_kind, pattern) DO NOTHING;
