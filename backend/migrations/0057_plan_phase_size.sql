-- Optional phase size for weighted burndown/velocity. NULL means unsized;
-- metrics treat unsized phases as weight 1 (s=1, m=2, l=3).
ALTER TABLE plan_phases
    ADD COLUMN size TEXT CHECK (size IN ('s', 'm', 'l'));
