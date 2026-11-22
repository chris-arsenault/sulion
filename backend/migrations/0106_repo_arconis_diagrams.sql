CREATE TABLE repo_arconis_diagrams (
    repo_name TEXT NOT NULL,
    workspace_id UUID NOT NULL,
    room_id TEXT NOT NULL CHECK (room_id ~ '^[0-9a-f]{48}$'),
    title TEXT NOT NULL,
    PRIMARY KEY (repo_name, workspace_id)
);
