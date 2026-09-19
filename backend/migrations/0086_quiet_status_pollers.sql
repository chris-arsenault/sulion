-- The repo and workspace status pollers update their row every 30 seconds
-- per repo and workspace. Both `next_status_at` columns were indexed, so
-- every one of those updates was a non-HOT update: a new heap tuple, new
-- index entries, a dead tuple for autovacuum, and a fresh full-page image
-- in WAL after every checkpoint. On 55 repo rows that grew the heap to
-- 184 MB and drove tens of gigabytes of WAL a day while nothing changed.
--
-- The due queries scan at most a few hundred rows, so the indexes bought
-- nothing. Dropping them lets a same-page update be heap-only; the lower
-- fill factor leaves each page room for those updates. The pollers now
-- also write only what changed, so the remaining per-cycle write is one
-- HOT update of the schedule columns.
--
-- The existing bloat is not reclaimed here: VACUUM cannot run inside the
-- migration transaction. Run once, out of band:
--   VACUUM FULL repo_runtime_state; VACUUM FULL workspaces;

DROP INDEX IF EXISTS repo_runtime_state_due_idx;
DROP INDEX IF EXISTS repo_runtime_state_git_activity_due_idx;
DROP INDEX IF EXISTS workspaces_due_idx;

ALTER TABLE repo_runtime_state SET (fillfactor = 50);
ALTER TABLE workspaces SET (fillfactor = 50);
