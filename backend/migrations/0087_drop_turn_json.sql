-- `timeline_turns.turn_json` was a serialized copy of the projected turn
-- that nothing read: the timeline API serves `chunks_json` and joins
-- `timeline_operations`, retrieval reads `markdown`, and every rebuild
-- path derives from `events.payload`. It was also the largest column in
-- the database (2.5 GB of TOAST) and, because a live turn is re-upserted
-- on every ingester tick that appends to it, the largest single source of
-- write-ahead log: a 2 MB turn rewrote that value for each new event.
--
-- Dropping the column is a catalog change; the bytes in existing tuples
-- are reclaimed as those rows are rewritten or the table is vacuumed in
-- full. No data anyone consumes is lost.

ALTER TABLE timeline_turns DROP COLUMN turn_json;
