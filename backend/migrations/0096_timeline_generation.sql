-- Offsets repeat after replay; caches must distinguish projection lifetimes.
ALTER TABLE timeline_session_state
    ADD COLUMN generation UUID NOT NULL DEFAULT gen_random_uuid();
