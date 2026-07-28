-- One identity everywhere: uid 7321 is `sulion` with home /home/sulion in the
-- image, every deployment role, and every stored path. The old portable `dev`
-- account name and its /home/dev paths are retired; rows written during the
-- /home/dev era are rewritten so queries need exactly one prefix and no
-- era-detection branching.
--
-- Guarded by prefix match, so rows already on /home/sulion and repo-relative
-- paths are untouched. Historical event payloads (canonical_events,
-- event_blocks) are records of what happened and keep their original text.

UPDATE pty_sessions
   SET working_dir = '/home/sulion' || substr(working_dir, length('/home/dev') + 1)
 WHERE working_dir LIKE '/home/dev/%';

UPDATE repos
   SET path = '/home/sulion' || substr(path, length('/home/dev') + 1)
 WHERE path LIKE '/home/dev/%';

UPDATE repo_runtime_state
   SET path = '/home/sulion' || substr(path, length('/home/dev') + 1)
 WHERE path LIKE '/home/dev/%';

UPDATE workspaces
   SET path = '/home/sulion' || substr(path, length('/home/dev') + 1)
 WHERE path LIKE '/home/dev/%';

UPDATE agent_session_metadata
   SET cwd = '/home/sulion' || substr(cwd, length('/home/dev') + 1)
 WHERE cwd LIKE '/home/dev/%';

-- Code-intelligence indexes are derived data keyed by absolute root paths.
-- Roots from the /home/dev era are stale on every current deployment; drop
-- them (children cascade) rather than rewrite — the indexer rebuilds live
-- roots on its own.
DELETE FROM code_roots WHERE path LIKE '/home/dev/%';
