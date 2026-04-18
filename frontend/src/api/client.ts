// Typed REST client. Stateless — callers do their own caching/polling.

import type {
  CreateRepoRequest,
  CreateSessionRequest,
  HistoryQuery,
  HistoryResponse,
  ListReposResponse,
  ListSessionsResponse,
  RepoView,
  SessionView,
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

export function getHistory(
  sessionId: string,
  query: HistoryQuery = {},
): Promise<HistoryResponse> {
  const params = new URLSearchParams();
  if (query.after != null) params.set("after", String(query.after));
  if (query.limit != null) params.set("limit", String(query.limit));
  if (query.kind) params.set("kind", query.kind);
  if (query.claude_session) params.set("claude_session", query.claude_session);
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
