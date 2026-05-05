// Typed REST client. Stateless — callers do their own caching/polling.
import { getAccessToken } from "../auth/cognito";

import type {
  AppStateResponse,
  CreateRepoRequest,
  CreateSessionRequest,
  CreateFuturePromptInput,
  DiffResponse,
  DirListing,
  FileTraceResponse,
  FileResponse,
  FuturePromptEntry,
  FuturePromptListResponse,
  HistoryQuery,
  HistoryResponse,
  LibraryEntry,
  LibraryKind,
  MonitorTimelineRequest,
  MonitorTimelineResponse,
  RepoDirtyPathsResponse,
  RepoView,
  SaveLibraryInput,
  SessionView,
  SecretEnvelope,
  SecretGrantMetadata,
  SecretMetadata,
  SecretTool,
  TimelineQuery,
  TimelineSummaryResponse,
  TimelineTurnDetailResponse,
  UpdateSessionRequest,
  UpdateFuturePromptInput,
  AgentLaunchType,
  WorkspaceDirtyPathsResponse,
  WorkspaceView,
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

async function authHeaders(
  init?: RequestInit,
  opts?: { json?: boolean },
): Promise<Headers> {
  const headers = new Headers(init?.headers);
  if (opts?.json !== false && !headers.has("Content-Type")) {
    headers.set("Content-Type", "application/json");
  }
  const token = await getAccessToken();
  if (token && !headers.has("Authorization")) {
    headers.set("Authorization", `Bearer ${token}`);
  }
  return headers;
}

export async function authFetch(
  url: string,
  init?: RequestInit,
  opts?: { json?: boolean },
): Promise<Response> {
  return fetch(url, {
    ...init,
    headers: await authHeaders(init, opts),
  });
}

async function brokerRequest<T>(url: string, init?: RequestInit): Promise<T> {
  const resp = await authFetch(url, init);
  if (!resp.ok) {
    throw new ApiError(resp.status, await parseErrorBody(resp));
  }
  const text = await resp.text();
  if (!text.trim()) {
    return undefined as T;
  }
  return JSON.parse(text) as T;
}

async function request<T>(url: string, init?: RequestInit): Promise<T> {
  const resp = await authFetch(url, init);
  if (!resp.ok) {
    throw new ApiError(resp.status, await parseErrorBody(resp));
  }
  if (resp.status === 204) {
    return undefined as T;
  }
  const text = await resp.text();
  if (!text.trim()) {
    return undefined as T;
  }
  return JSON.parse(text) as T;
}

export function getAppState(): Promise<AppStateResponse> {
  return request<AppStateResponse>("/api/app-state");
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

export function startSessionAgent(
  id: string,
  agent: AgentLaunchType,
): Promise<void> {
  return request<void>(`/api/sessions/${id}/agent`, {
    method: "POST",
    body: JSON.stringify({ agent }),
  });
}

export function interruptSessionAgent(id: string): Promise<void> {
  return request<void>(`/api/sessions/${id}/agent/interrupt`, {
    method: "POST",
  });
}

export function sendSessionPrompt(id: string, text: string): Promise<void> {
  return request<void>(`/api/sessions/${id}/prompt`, {
    method: "POST",
    body: JSON.stringify({ text }),
  });
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
): Promise<TimelineSummaryResponse> {
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
  return request<TimelineSummaryResponse>(
    `/api/sessions/${sessionId}/timeline${suffix}`,
  );
}

export function getTimelineTurn(
  sessionId: string,
  turnId: number,
  query: TimelineQuery = {},
): Promise<TimelineTurnDetailResponse> {
  const params = new URLSearchParams();
  if (query.session) params.set("session", query.session);
  appendTimelineFilterParams(params, query);
  const qs = params.toString();
  const suffix = qs ? `?${qs}` : "";
  return request<TimelineTurnDetailResponse>(
    `/api/sessions/${sessionId}/timeline/turns/${turnId}${suffix}`,
  );
}

export function getRepoTimeline(
  repo: string,
  query: TimelineQuery = {},
): Promise<TimelineSummaryResponse> {
  const params = new URLSearchParams();
  appendTimelineFilterParams(params, query);
  const qs = params.toString();
  const suffix = qs ? `?${qs}` : "";
  return request<TimelineSummaryResponse>(
    `/api/repos/${encodeURIComponent(repo)}/timeline${suffix}`,
  );
}

export function getRepoTimelineTurn(
  repo: string,
  sessionUuid: string,
  turnId: number,
  query: TimelineQuery = {},
): Promise<TimelineTurnDetailResponse> {
  const params = new URLSearchParams();
  appendTimelineFilterParams(params, query);
  const qs = params.toString();
  const suffix = qs ? `?${qs}` : "";
  return request<TimelineTurnDetailResponse>(
    `/api/repos/${encodeURIComponent(repo)}/timeline/turns/${sessionUuid}/${turnId}${suffix}`,
  );
}

export function getMonitorTimeline(
  body: MonitorTimelineRequest,
): Promise<MonitorTimelineResponse> {
  return request<MonitorTimelineResponse>("/api/monitor/timeline", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

function appendTimelineFilterParams(
  params: URLSearchParams,
  query: TimelineQuery,
) {
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
}

export function createRepo(body: CreateRepoRequest): Promise<RepoView> {
  return request<RepoView>("/api/repos", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export interface ReindexResponse {
  sessions_rebuilt: number;
  events_preserved: number;
  canonical_events_rebuilt: number;
  timeline_sessions_rebuilt: number;
}

export function triggerReindex(): Promise<ReindexResponse> {
  return request<ReindexResponse>("/api/admin/reindex", { method: "POST" });
}

export function listSecrets(): Promise<SecretMetadata[]> {
  return brokerRequest<SecretMetadata[]>("/broker/v1/secrets");
}

export function getSecret(id: string): Promise<SecretEnvelope> {
  return brokerRequest<SecretEnvelope>(`/broker/v1/secrets/${encodeURIComponent(id)}`);
}

export function upsertSecret(id: string, body: SecretEnvelope): Promise<void> {
  return brokerRequest<void>(`/broker/v1/secrets/${encodeURIComponent(id)}`, {
    method: "PUT",
    body: JSON.stringify(body),
  });
}

export function deleteSecret(id: string): Promise<void> {
  return brokerRequest<void>(`/broker/v1/secrets/${encodeURIComponent(id)}`, {
    method: "DELETE",
  });
}

export function listSecretGrants(ptySessionId: string): Promise<SecretGrantMetadata[]> {
  const qs = new URLSearchParams({ pty_session_id: ptySessionId });
  return brokerRequest<SecretGrantMetadata[]>(`/broker/v1/grants?${qs.toString()}`);
}

export function unlockSecretGrant(body: {
  pty_session_id: string;
  secret_id: string;
  tool: SecretTool;
  ttl_seconds: number;
}): Promise<void> {
  return brokerRequest<void>("/broker/v1/grants", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export function revokeSecretGrant(body: {
  pty_session_id: string;
  secret_id: string;
  tool: SecretTool;
}): Promise<void> {
  return brokerRequest<void>("/broker/v1/grants", {
    method: "DELETE",
    body: JSON.stringify(body),
  });
}

export function getRepoDirtyPaths(name: string): Promise<RepoDirtyPathsResponse> {
  return request<RepoDirtyPathsResponse>(
    `/api/repos/${encodeURIComponent(name)}/dirty-paths`,
  );
}

export function listWorkspaces(): Promise<WorkspaceView[]> {
  return request<WorkspaceView[]>("/api/workspaces");
}

export function getWorkspace(id: string): Promise<WorkspaceView> {
  return request<WorkspaceView>(`/api/workspaces/${encodeURIComponent(id)}`);
}

export function deleteWorkspace(
  id: string,
  opts: { force?: boolean; deleteBranch?: boolean } = {},
): Promise<void> {
  const qs = new URLSearchParams();
  if (opts.force) qs.set("force", "true");
  if (opts.deleteBranch === false) qs.set("delete_branch", "false");
  const suffix = qs.toString() ? `?${qs}` : "";
  return request<void>(`/api/workspaces/${encodeURIComponent(id)}${suffix}`, {
    method: "DELETE",
  });
}

export function getWorkspaceDirtyPaths(
  id: string,
): Promise<WorkspaceDirtyPathsResponse> {
  return request<WorkspaceDirtyPathsResponse>(
    `/api/workspaces/${encodeURIComponent(id)}/dirty-paths`,
  );
}

export function refreshWorkspaceState(id: string): Promise<void> {
  return request<void>(`/api/workspaces/${encodeURIComponent(id)}/refresh`, {
    method: "POST",
  });
}

export function getWorkspaceFiles(
  id: string,
  path = "",
  all = false,
): Promise<DirListing> {
  const qs = new URLSearchParams();
  if (path) qs.set("path", path);
  if (all) qs.set("all", "true");
  const suffix = qs.toString() ? `?${qs}` : "";
  return request<DirListing>(
    `/api/workspaces/${encodeURIComponent(id)}/files${suffix}`,
  );
}

export function getWorkspaceFile(
  id: string,
  path: string,
): Promise<FileResponse> {
  return request<FileResponse>(
    `/api/workspaces/${encodeURIComponent(id)}/file?path=${encodeURIComponent(path)}`,
  );
}

export function getWorkspaceFileTrace(
  id: string,
  path: string,
): Promise<FileTraceResponse> {
  return request<FileTraceResponse>(
    `/api/workspaces/${encodeURIComponent(id)}/file-trace?path=${encodeURIComponent(path)}`,
  );
}

export function getWorkspaceDiff(
  id: string,
  path?: string,
): Promise<DiffResponse> {
  const qs = path ? `?path=${encodeURIComponent(path)}` : "";
  return request<DiffResponse>(
    `/api/workspaces/${encodeURIComponent(id)}/git/diff${qs}`,
  );
}

export function stageWorkspacePath(
  id: string,
  path: string,
  stage: boolean,
): Promise<void> {
  return request<void>(`/api/workspaces/${encodeURIComponent(id)}/git/stage`, {
    method: "POST",
    body: JSON.stringify({ path, stage }),
  });
}

export function refreshRepoState(name: string): Promise<void> {
  return request<void>(`/api/repos/${encodeURIComponent(name)}/refresh`, {
    method: "POST",
  });
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
  const resp = await authFetch(
    `/api/repos/${encodeURIComponent(name)}/upload${qs}`,
    { method: "POST", body: form },
    { json: false },
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
