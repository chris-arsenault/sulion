// Typed REST client. Stateless — callers do their own caching/polling.

import type {
  CreateRepoRequest,
  CreateSessionRequest,
  CreateFuturePromptInput,
  DiffResponse,
  DirListing,
  FileTraceResponse,
  FileResponse,
  FuturePromptEntry,
  FuturePromptListResponse,
  GitStatus,
  HistoryQuery,
  HistoryResponse,
  LibraryEntry,
  LibraryKind,
  ListReposResponse,
  ListSessionsResponse,
  RepoView,
  SaveLibraryInput,
  SessionView,
  StatsResponse,
  TimelineQuery,
  TimelineResponse,
  UpdateSessionRequest,
  UpdateFuturePromptInput,
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
  const suffix = qs ? `?${qs}` : "";
  return request<HistoryResponse>(
    `/api/sessions/${sessionId}/history${suffix}`,
  );
}

export function getTimeline(
  sessionId: string,
  query: TimelineQuery = {},
): Promise<TimelineResponse> {
  const params = new URLSearchParams();
  if (query.session) params.set("session", query.session);
  if (query.hidden_speakers?.length) {
    params.set("hide_speakers", query.hidden_speakers.join(","));
  }
  if (query.hidden_operation_categories?.length) {
    params.set("hide_categories", query.hidden_operation_categories.join(","));
  }
  if (query.errors_only) params.set("errors_only", "true");
  if (query.show_bookkeeping) params.set("show_bookkeeping", "true");
  if (query.show_sidechain) params.set("show_sidechain", "true");
  if (query.file_path) params.set("file_path", query.file_path);
  const qs = params.toString();
  const suffix = qs ? `?${qs}` : "";
  return request<TimelineResponse>(
    `/api/sessions/${sessionId}/timeline${suffix}`,
  );
}

export function getRepoTimeline(
  repo: string,
  query: TimelineQuery = {},
): Promise<TimelineResponse> {
  const params = new URLSearchParams();
  if (query.hidden_speakers?.length) {
    params.set("hide_speakers", query.hidden_speakers.join(","));
  }
  if (query.hidden_operation_categories?.length) {
    params.set("hide_categories", query.hidden_operation_categories.join(","));
  }
  if (query.errors_only) params.set("errors_only", "true");
  if (query.show_bookkeeping) params.set("show_bookkeeping", "true");
  if (query.show_sidechain) params.set("show_sidechain", "true");
  if (query.file_path) params.set("file_path", query.file_path);
  const qs = params.toString();
  const suffix = qs ? `?${qs}` : "";
  return request<TimelineResponse>(
    `/api/repos/${encodeURIComponent(repo)}/timeline${suffix}`,
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

export interface ReindexResponse {
  sessions_cleared: number;
  offsets_cleared: number;
}

export function triggerReindex(): Promise<ReindexResponse> {
  return request<ReindexResponse>("/api/admin/reindex", { method: "POST" });
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
  const suffix = qs.toString() ? `?${qs}` : "";
  return request<DirListing>(
    `/api/repos/${encodeURIComponent(name)}/files${suffix}`,
  );
}

export function getRepoFile(name: string, path: string): Promise<FileResponse> {
  return request<FileResponse>(
    `/api/repos/${encodeURIComponent(name)}/file?path=${encodeURIComponent(path)}`,
  );
}

export function getRepoFileTrace(
  name: string,
  path: string,
): Promise<FileTraceResponse> {
  return request<FileTraceResponse>(
    `/api/repos/${encodeURIComponent(name)}/file-trace?path=${encodeURIComponent(path)}`,
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

// ─── global library ──────────────────────────────────────────────────

export function listLibrary(
  kind: LibraryKind,
): Promise<LibraryEntry[]> {
  return request<LibraryEntry[]>(`/api/library/${kind}`);
}

export function getLibraryEntry(
  kind: LibraryKind,
  slug: string,
): Promise<LibraryEntry> {
  return request<LibraryEntry>(
    `/api/library/${kind}/${encodeURIComponent(slug)}`,
  );
}

export function saveLibraryEntry(
  kind: LibraryKind,
  input: SaveLibraryInput,
  slug?: string,
): Promise<LibraryEntry> {
  const url = slug
    ? `/api/library/${kind}/${encodeURIComponent(slug)}`
    : `/api/library/${kind}`;
  return request<LibraryEntry>(url, {
    method: "PUT",
    body: JSON.stringify(input),
  });
}

export function deleteLibraryEntry(
  kind: LibraryKind,
  slug: string,
): Promise<void> {
  return request<void>(`/api/library/${kind}/${encodeURIComponent(slug)}`, {
    method: "DELETE",
  });
}

// ─── future prompts ──────────────────────────────────────────────────

export function listFuturePrompts(
  sessionId: string,
): Promise<FuturePromptListResponse> {
  return request<FuturePromptListResponse>(
    `/api/sessions/${sessionId}/future-prompts`,
  );
}

export function createFuturePrompt(
  sessionId: string,
  input: CreateFuturePromptInput,
): Promise<FuturePromptEntry> {
  return request<FuturePromptEntry>(
    `/api/sessions/${sessionId}/future-prompts`,
    {
      method: "PUT",
      body: JSON.stringify(input),
    },
  );
}

export function updateFuturePrompt(
  sessionId: string,
  id: string,
  input: UpdateFuturePromptInput,
): Promise<FuturePromptEntry> {
  return request<FuturePromptEntry>(
    `/api/sessions/${sessionId}/future-prompts/${encodeURIComponent(id)}`,
    {
      method: "PATCH",
      body: JSON.stringify(input),
    },
  );
}

export function deleteFuturePrompt(
  sessionId: string,
  id: string,
): Promise<void> {
  return request<void>(
    `/api/sessions/${sessionId}/future-prompts/${encodeURIComponent(id)}`,
    {
      method: "DELETE",
    },
  );
}
