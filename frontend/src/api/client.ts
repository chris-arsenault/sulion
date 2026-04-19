// Typed REST client. Stateless — callers do their own caching/polling.

import type {
  CreateRepoRequest,
  CreateSessionRequest,
  DiffResponse,
  DirListing,
  FileResponse,
  GitStatus,
  HistoryQuery,
  HistoryResponse,
  LibraryEntry,
  LibraryKind,
  ListReposResponse,
  ListSessionsResponse,
  RepoView,
  SaveLibraryInput,
  SearchHit,
  SearchScope,
  SessionView,
  StatsResponse,
  UpdateSessionRequest,
} from "./types";

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

async function parseErrorBody(resp: Response): Promise<string> {
  try {
    const body = await resp.json();
    if (body && typeof body.error === "string") return body.error;
  } catch {
    // Swallow — not JSON.
  }
  return resp.statusText || `HTTP ${resp.status}`;
}

async function request<T>(url: string, init?: RequestInit): Promise<T> {
  const resp = await fetch(url, {
    headers: { "Content-Type": "application/json" },
    ...init,
  });
  if (!resp.ok) {
    throw new ApiError(resp.status, await parseErrorBody(resp));
  }
  if (resp.status === 204) {
    return undefined as T;
  }
  return (await resp.json()) as T;
}

export function listSessions(): Promise<ListSessionsResponse> {
  return request<ListSessionsResponse>("/api/sessions");
}

export function createSession(body: CreateSessionRequest): Promise<SessionView> {
  return request<SessionView>("/api/sessions", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export function deleteSession(id: string): Promise<void> {
  return request<void>(`/api/sessions/${id}`, { method: "DELETE" });
}

/** Update user-facing session metadata. Any field you omit is left
 * unchanged server-side. Pass null for label/color to clear (backend
 * interprets empty string as null). */
export function updateSession(
  id: string,
  body: UpdateSessionRequest,
): Promise<void> {
  // Normalise: null → empty string (backend converts to NULL).
  const payload: Record<string, unknown> = { ...body };
  if (payload.label === null) payload.label = "";
  if (payload.color === null) payload.color = "";
  return request<void>(`/api/sessions/${id}`, {
    method: "PATCH",
    body: JSON.stringify(payload),
  });
}

export function getHistory(
  sessionId: string,
  query: HistoryQuery = {},
): Promise<HistoryResponse> {
  const params = new URLSearchParams();
  if (query.after != null) params.set("after", String(query.after));
  if (query.limit != null) params.set("limit", String(query.limit));
  if (query.kind) params.set("kind", query.kind);
  if (query.session) params.set("session", query.session);
  const qs = params.toString();
  return request<HistoryResponse>(
    `/api/sessions/${sessionId}/history${qs ? `?${qs}` : ""}`,
  );
}

export function listRepos(): Promise<ListReposResponse> {
  return request<ListReposResponse>("/api/repos");
}

export function createRepo(body: CreateRepoRequest): Promise<RepoView> {
  return request<RepoView>("/api/repos", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export function getStats(): Promise<StatsResponse> {
  return request<StatsResponse>("/api/stats");
}

export function getRepoGit(name: string): Promise<GitStatus> {
  return request<GitStatus>(`/api/repos/${encodeURIComponent(name)}/git`);
}

export function getRepoFiles(
  name: string,
  path = "",
  all = false,
): Promise<DirListing> {
  const qs = new URLSearchParams();
  if (path) qs.set("path", path);
  if (all) qs.set("all", "true");
  return request<DirListing>(
    `/api/repos/${encodeURIComponent(name)}/files${qs.toString() ? `?${qs}` : ""}`,
  );
}

export function getRepoFile(name: string, path: string): Promise<FileResponse> {
  return request<FileResponse>(
    `/api/repos/${encodeURIComponent(name)}/file?path=${encodeURIComponent(path)}`,
  );
}

export function getRepoDiff(name: string, path?: string): Promise<DiffResponse> {
  const qs = path ? `?path=${encodeURIComponent(path)}` : "";
  return request<DiffResponse>(`/api/repos/${encodeURIComponent(name)}/git/diff${qs}`);
}

export function stageRepoPath(
  name: string,
  path: string,
  stage: boolean,
): Promise<void> {
  return request<void>(`/api/repos/${encodeURIComponent(name)}/git/stage`, {
    method: "POST",
    body: JSON.stringify({ path, stage }),
  });
}

/** Upload files via multipart. `path` is the directory under the repo
 * root to drop into; empty for repo root. Returns the first-written
 * absolute path. */
export async function uploadRepoFile(
  name: string,
  path: string,
  file: File,
): Promise<{ path: string; size: number }> {
  const form = new FormData();
  form.append("file", file, file.name);
  const qs = path ? `?path=${encodeURIComponent(path)}` : "";
  const resp = await fetch(
    `/api/repos/${encodeURIComponent(name)}/upload${qs}`,
    { method: "POST", body: form },
  );
  if (!resp.ok) {
    throw new ApiError(resp.status, await parseErrorBody(resp));
  }
  return resp.json();
}

/** Stream NDJSON search hits. Each callback call delivers one parsed hit. */
export async function searchStream(
  params: {
    q: string;
    scope: SearchScope;
    repo?: string;
    session?: string;
    signal?: AbortSignal;
  },
  onHit: (hit: SearchHit) => void,
): Promise<void> {
  const qs = new URLSearchParams();
  qs.set("q", params.q);
  qs.set("scope", params.scope);
  if (params.repo) qs.set("repo", params.repo);
  if (params.session) qs.set("session", params.session);
  const resp = await fetch(`/api/search?${qs}`, { signal: params.signal });
  if (!resp.ok || !resp.body) {
    throw new ApiError(resp.status, await parseErrorBody(resp));
  }
  const reader = resp.body.getReader();
  const decoder = new TextDecoder();
  let buf = "";
  while (true) {
    const { value, done } = await reader.read();
    if (done) break;
    buf += decoder.decode(value, { stream: true });
    let idx: number;
    while ((idx = buf.indexOf("\n")) !== -1) {
      const line = buf.slice(0, idx).trim();
      buf = buf.slice(idx + 1);
      if (!line) continue;
      try {
        onHit(JSON.parse(line) as SearchHit);
      } catch {
        // Skip malformed lines.
      }
    }
  }
}

// ─── library (refs + prompts) ────────────────────────────────────────

export function listLibrary(
  repo: string,
  kind: LibraryKind,
): Promise<LibraryEntry[]> {
  return request<LibraryEntry[]>(
    `/api/repos/${encodeURIComponent(repo)}/library/${kind}`,
  );
}

export function getLibraryEntry(
  repo: string,
  kind: LibraryKind,
  slug: string,
): Promise<LibraryEntry> {
  return request<LibraryEntry>(
    `/api/repos/${encodeURIComponent(repo)}/library/${kind}/${encodeURIComponent(slug)}`,
  );
}

export function saveLibraryEntry(
  repo: string,
  kind: LibraryKind,
  input: SaveLibraryInput,
  slug?: string,
): Promise<LibraryEntry> {
  const url = slug
    ? `/api/repos/${encodeURIComponent(repo)}/library/${kind}/${encodeURIComponent(slug)}`
    : `/api/repos/${encodeURIComponent(repo)}/library/${kind}`;
  return request<LibraryEntry>(url, {
    method: "PUT",
    body: JSON.stringify(input),
  });
}

export function deleteLibraryEntry(
  repo: string,
  kind: LibraryKind,
  slug: string,
): Promise<void> {
  return request<void>(
    `/api/repos/${encodeURIComponent(repo)}/library/${kind}/${encodeURIComponent(slug)}`,
    { method: "DELETE" },
  );
}
