-- Prompt/reference library moves from node-local markdown files into the
-- database so entries survive topology changes and are served by whichever
-- process answers /api/library.
CREATE TABLE library_entries (
    kind TEXT NOT NULL,
    slug TEXT NOT NULL,
    name TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (kind, slug)
);
