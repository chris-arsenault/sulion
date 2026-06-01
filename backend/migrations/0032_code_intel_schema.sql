-- Code intelligence stores compact structural facts about files under
-- Sulion repos/workspaces. Source text and full ASTs remain on disk.

CREATE TABLE IF NOT EXISTS code_roots (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    root_kind TEXT NOT NULL CHECK (root_kind IN ('repo', 'workspace')),
    name TEXT NOT NULL,
    path TEXT NOT NULL,
    repo_name TEXT,
    workspace_id UUID REFERENCES workspaces(id) ON DELETE SET NULL,
    git_head TEXT,
    last_scan_at TIMESTAMPTZ,
    deleted_at TIMESTAMPTZ,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS code_files (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    root_id UUID NOT NULL REFERENCES code_roots(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    language TEXT,
    content_hash TEXT,
    git_blob_hash TEXT,
    size_bytes BIGINT NOT NULL DEFAULT 0 CHECK (size_bytes >= 0),
    mtime TIMESTAMPTZ,
    line_count INTEGER NOT NULL DEFAULT 0 CHECK (line_count >= 0),
    parse_status TEXT NOT NULL DEFAULT 'pending'
        CHECK (parse_status IN ('pending', 'parsed', 'partial', 'failed', 'unsupported', 'deleted')),
    parse_error_count INTEGER NOT NULL DEFAULT 0 CHECK (parse_error_count >= 0),
    indexed_at TIMESTAMPTZ,
    deleted_at TIMESTAMPTZ,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS code_symbols (
    id TEXT PRIMARY KEY,
    root_id UUID NOT NULL REFERENCES code_roots(id) ON DELETE CASCADE,
    file_id UUID NOT NULL REFERENCES code_files(id) ON DELETE CASCADE,
    parent_symbol_id TEXT REFERENCES code_symbols(id) ON DELETE SET NULL,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    qualified_name TEXT NOT NULL,
    signature TEXT,
    visibility TEXT,
    exported BOOLEAN,
    disambiguator INTEGER NOT NULL DEFAULT 0 CHECK (disambiguator >= 0),
    decl_start_line INTEGER NOT NULL CHECK (decl_start_line >= 1),
    decl_start_col INTEGER NOT NULL CHECK (decl_start_col >= 1),
    decl_end_line INTEGER NOT NULL CHECK (decl_end_line >= 1),
    decl_end_col INTEGER NOT NULL CHECK (decl_end_col >= 1),
    body_start_line INTEGER,
    body_start_col INTEGER,
    body_end_line INTEGER,
    body_end_col INTEGER,
    doc_start_line INTEGER,
    doc_start_col INTEGER,
    doc_end_line INTEGER,
    doc_end_col INTEGER,
    confidence TEXT NOT NULL DEFAULT 'syntactic'
        CHECK (confidence IN ('semantic', 'syntactic', 'mixed', 'stale', 'partial')),
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (decl_end_line > decl_start_line OR (decl_end_line = decl_start_line AND decl_end_col >= decl_start_col)),
    CHECK (body_start_line IS NULL OR body_start_col IS NOT NULL),
    CHECK (body_start_line IS NULL OR body_end_line IS NOT NULL),
    CHECK (body_start_line IS NULL OR body_end_col IS NOT NULL),
    CHECK (doc_start_line IS NULL OR doc_start_col IS NOT NULL),
    CHECK (doc_start_line IS NULL OR doc_end_line IS NOT NULL),
    CHECK (doc_start_line IS NULL OR doc_end_col IS NOT NULL)
);

CREATE TABLE IF NOT EXISTS code_references (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    root_id UUID NOT NULL REFERENCES code_roots(id) ON DELETE CASCADE,
    file_id UUID NOT NULL REFERENCES code_files(id) ON DELETE CASCADE,
    symbol_id TEXT REFERENCES code_symbols(id) ON DELETE SET NULL,
    referenced_name TEXT NOT NULL,
    reference_kind TEXT NOT NULL DEFAULT 'use',
    start_line INTEGER NOT NULL CHECK (start_line >= 1),
    start_col INTEGER NOT NULL CHECK (start_col >= 1),
    end_line INTEGER NOT NULL CHECK (end_line >= 1),
    end_col INTEGER NOT NULL CHECK (end_col >= 1),
    confidence TEXT NOT NULL DEFAULT 'syntactic'
        CHECK (confidence IN ('semantic', 'syntactic', 'mixed', 'stale', 'partial')),
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (end_line > start_line OR (end_line = start_line AND end_col >= start_col))
);

CREATE TABLE IF NOT EXISTS code_imports (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    root_id UUID NOT NULL REFERENCES code_roots(id) ON DELETE CASCADE,
    file_id UUID NOT NULL REFERENCES code_files(id) ON DELETE CASCADE,
    import_path TEXT NOT NULL,
    imported_name TEXT,
    alias TEXT,
    start_line INTEGER NOT NULL CHECK (start_line >= 1),
    start_col INTEGER NOT NULL CHECK (start_col >= 1),
    end_line INTEGER NOT NULL CHECK (end_line >= 1),
    end_col INTEGER NOT NULL CHECK (end_col >= 1),
    confidence TEXT NOT NULL DEFAULT 'syntactic'
        CHECK (confidence IN ('semantic', 'syntactic', 'mixed', 'stale', 'partial')),
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (end_line > start_line OR (end_line = start_line AND end_col >= start_col))
);

CREATE TABLE IF NOT EXISTS code_index_jobs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    root_id UUID REFERENCES code_roots(id) ON DELETE SET NULL,
    status TEXT NOT NULL CHECK (status IN ('queued', 'running', 'complete', 'failed', 'cancelled')),
    trigger TEXT NOT NULL CHECK (trigger IN ('startup', 'manual', 'query', 'background')),
    path TEXT,
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    files_seen INTEGER NOT NULL DEFAULT 0 CHECK (files_seen >= 0),
    files_indexed INTEGER NOT NULL DEFAULT 0 CHECK (files_indexed >= 0),
    files_failed INTEGER NOT NULL DEFAULT 0 CHECK (files_failed >= 0),
    error TEXT,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
