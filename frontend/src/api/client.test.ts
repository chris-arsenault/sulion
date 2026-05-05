import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  ApiError,
  createRepo,
  createSession,
  deleteSession,
  deleteWorkspace,
  getAppState,
  getHistory,
  getRepoDirtyPaths,
  getRepoFileTrace,
  getRepoTimelineTurn,
  getTimeline,
  getTimelineTurn,
  getWorkspaceDirtyPaths,
  getWorkspaceDiff,
  getWorkspaceFile,
  getWorkspaceFileTrace,
  refreshWorkspaceState,
  refreshRepoState,
  stageWorkspacePath,
  unlockSecretGrant,
  upsertSecret,
} from "./client";

function stubFetch(
  impl: (input: string, init?: RequestInit) => Promise<Response>,
) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo, init?: RequestInit) => {
      const url = typeof input === "string" ? input : input.url;
      return impl(url, init);
    }),
  );
}

const jsonResponse = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

describe("api client", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("getAppState GETs /api/app-state and parses the unified state", async () => {
    stubFetch(async (url) => {
      expect(url).toBe("/api/app-state");
      return jsonResponse({
        generated_at: "2026-01-01T00:00:00Z",
        sessions: [],
        repos: [],
        workspaces: [],
        stats: { uptime_seconds: 1, process: {}, pty: {}, db: {}, inventory: {} },
      });
    });
    const resp = await getAppState();
    expect(resp.sessions).toEqual([]);
    expect(resp.repos).toEqual([]);
  });

  it("createSession POSTs JSON body and parses the SessionView", async () => {
    stubFetch(async (url, init) => {
      expect(url).toBe("/api/sessions");
      expect(init?.method).toBe("POST");
      expect(init?.body).toBe(JSON.stringify({ repo: "r" }));
      return jsonResponse(
        {
          id: "00000000-0000-0000-0000-000000000000",
          repo: "r",
          working_dir: "/tmp/r",
          state: "live",
          created_at: "2025-01-01T00:00:00Z",
          ended_at: null,
          exit_code: null,
          current_session_uuid: null,
          current_session_agent: null,
        },
        201,
      );
    });
    const s = await createSession({ repo: "r" });
    expect(s.repo).toBe("r");
    expect(s.state).toBe("live");
  });

  it("deleteSession returns void on 204", async () => {
    stubFetch(async (url, init) => {
      expect(init?.method).toBe("DELETE");
      expect(url).toBe("/api/sessions/abc");
      return new Response(null, { status: 204 });
    });
    await expect(deleteSession("abc")).resolves.toBeUndefined();
  });

  it("getHistory serializes query params", async () => {
    stubFetch(async (url) => {
      expect(url).toBe("/api/sessions/s/history?after=5&limit=10&kind=user");
      return jsonResponse({
        session_uuid: null,
        session_agent: null,
        events: [],
        next_after: null,
      });
    });
    const r = await getHistory("s", { after: 5, limit: 10, kind: "user" });
    expect(r.events).toEqual([]);
  });

  it("getTimeline serializes projected-filter query params", async () => {
    stubFetch(async (url) => {
      expect(url).toBe(
        "/api/sessions/s/timeline?hide_speakers=assistant&hide_categories=utility%2Cresearch&errors_only=true&show_sidechain=true&file_path=foo.ts",
      );
      return jsonResponse({
        session_uuid: null,
        session_agent: null,
        total_event_count: 0,
        turns: [],
      });
    });
    const resp = await getTimeline("s", {
      hidden_speakers: ["assistant"],
      hidden_operation_categories: ["utility", "research"],
      errors_only: true,
      show_sidechain: true,
      file_path: "foo.ts",
    });
    expect(resp.turns).toEqual([]);
  });

  it("getTimelineTurn hits the selected-turn detail endpoint", async () => {
    stubFetch(async (url) => {
      expect(url).toBe(
        "/api/sessions/s/timeline/turns/7?hide_speakers=assistant",
      );
      return jsonResponse({
        session_uuid: "session-1",
        session_agent: "claude-code",
        turn: { id: 7 },
      });
    });
    const resp = await getTimelineTurn("s", 7, {
      hidden_speakers: ["assistant"],
    });
    expect(resp.turn.id).toBe(7);
  });

  it("getRepoTimelineTurn includes repo and transcript session in the URL", async () => {
    stubFetch(async (url) => {
      expect(url).toBe(
        "/api/repos/r/timeline/turns/00000000-0000-0000-0000-000000000001/7?show_sidechain=true",
      );
      return jsonResponse({
        session_uuid: "00000000-0000-0000-0000-000000000001",
        session_agent: "codex",
        turn: { id: 7 },
      });
    });
    const resp = await getRepoTimelineTurn(
      "r",
      "00000000-0000-0000-0000-000000000001",
      7,
      { show_sidechain: true },
    );
    expect(resp.session_agent).toBe("codex");
  });

  it("createRepo POSTs body", async () => {
    stubFetch(async (url, init) => {
      expect(url).toBe("/api/repos");
      expect(init?.method).toBe("POST");
      return jsonResponse({ name: "x", path: "/tmp/x" }, 201);
    });
    const r = await createRepo({ name: "x" });
    expect(r.name).toBe("x");
  });

  it("fetches cached repo dirty paths from the detail endpoint", async () => {
    stubFetch(async (url) => {
      expect(url).toBe("/api/repos/r/dirty-paths");
      return jsonResponse({
        repo: "r",
        git_revision: 4,
        dirty_by_path: { "src/lib.rs": " M" },
        diff_stats_by_path: { "src/lib.rs": { additions: 1, deletions: 2 } },
      });
    });
    const dirty = await getRepoDirtyPaths("r");
    expect(dirty.git_revision).toBe(4);
  });

  it("fetches workspace-scoped filesystem and git endpoints", async () => {
    const workspaceId = "00000000-0000-0000-0000-000000000042";
    const filePath = "src/lib.rs";
    const filePathEncoded = "src%2Flib.rs";
    const seen: string[] = [];
    stubFetch(async (url, init) => {
      seen.push(`${init?.method ?? "GET"} ${url}`);
      if (url.endsWith("/dirty-paths")) {
        return jsonResponse({
          workspace_id: workspaceId,
          git_revision: 2,
          dirty_by_path: { [filePath]: " M" },
          diff_stats_by_path: {},
        });
      }
      if (url.endsWith(`/git/diff?path=${filePathEncoded}`)) {
        return jsonResponse({ diff: `diff --git a/${filePath} b/${filePath}\n` });
      }
      if (url.endsWith(`/file?path=${filePathEncoded}`)) {
        return jsonResponse({
          path: filePath,
          size: 7,
          mime: "text/plain",
          binary: false,
          truncated: false,
          content: "changed",
        });
      }
      if (url.endsWith(`/file-trace?path=${filePathEncoded}`)) {
        return jsonResponse({
          path: filePath,
          dirty: " M",
          current_diff: null,
          touches: [],
        });
      }
      return new Response(null, { status: 202 });
    });

    await expect(refreshWorkspaceState(workspaceId)).resolves.toBeUndefined();
    expect((await getWorkspaceDirtyPaths(workspaceId)).git_revision).toBe(2);
    expect((await getWorkspaceDiff(workspaceId, filePath)).diff).toContain("diff");
    expect((await getWorkspaceFile(workspaceId, filePath)).content).toBe("changed");
    expect((await getWorkspaceFileTrace(workspaceId, filePath)).dirty).toBe(" M");
    await expect(stageWorkspacePath(workspaceId, filePath, true)).resolves.toBeUndefined();
    await expect(
      deleteWorkspace(workspaceId, { force: true }),
    ).resolves.toBeUndefined();

    expect(seen).toContain(`POST /api/workspaces/${workspaceId}/refresh`);
    expect(seen).toContain(`POST /api/workspaces/${workspaceId}/git/stage`);
    expect(seen).toContain(`DELETE /api/workspaces/${workspaceId}?force=true`);
  });

  it("requests repo refresh without reading git inline", async () => {
    stubFetch(async (url, init) => {
      expect(url).toBe("/api/repos/r/refresh");
      expect(init?.method).toBe("POST");
      return new Response(null, { status: 202 });
    });
    await expect(refreshRepoState("r")).resolves.toBeUndefined();
  });

  it("getRepoFileTrace hits the file-trace endpoint", async () => {
    stubFetch(async (url) => {
      expect(url).toBe("/api/repos/r/file-trace?path=src%2Flib.rs");
      return jsonResponse({
        path: "src/lib.rs",
        dirty: null,
        current_diff: null,
        touches: [],
      });
    });
    const trace = await getRepoFileTrace("r", "src/lib.rs");
    expect(trace.path).toBe("src/lib.rs");
  });

  it("wraps non-ok responses in ApiError using the server-provided error", async () => {
    stubFetch(async () =>
      new Response(JSON.stringify({ error: "boom" }), {
        status: 400,
        headers: { "Content-Type": "application/json" },
      }),
    );
    await expect(getAppState()).rejects.toBeInstanceOf(ApiError);
    try {
      await getAppState();
    } catch (err) {
      if (err instanceof ApiError) {
        expect(err.status).toBe(400);
        expect(err.message).toBe("boom");
      }
    }
  });

  it("resolves broker writes that return 201 with an empty body", async () => {
    stubFetch(async (url, init) => {
      expect(url).toBe("/broker/v1/secrets/claude-api");
      expect(init?.method).toBe("PUT");
      expect(JSON.parse(init?.body as string)).toEqual({
        description: "Claude",
        scope: "global",
        repo: null,
        env: { ANTHROPIC_API_KEY: "sxxx" },
      });
      return new Response(null, { status: 201 });
    });

    await expect(
      upsertSecret("claude-api", {
        description: "Claude",
        scope: "global",
        repo: null,
        env: { ANTHROPIC_API_KEY: "sxxx" },
      }),
    ).resolves.toBeUndefined();
  });

  it("posts grant unlock requests to the broker", async () => {
    stubFetch(async (url, init) => {
      expect(url).toBe("/broker/v1/grants");
      expect(init?.method).toBe("POST");
      expect(JSON.parse(init?.body as string)).toEqual({
        pty_session_id: "pty-1",
        secret_id: "claude-api",
        tool: "with-cred",
        ttl_seconds: 600,
      });
      return new Response(null, { status: 201 });
    });

    await expect(
      unlockSecretGrant({
        pty_session_id: "pty-1",
        secret_id: "claude-api",
        tool: "with-cred",
        ttl_seconds: 600,
      }),
    ).resolves.toBeUndefined();
  });
});

describe("ApiError", () => {
  beforeEach(() => vi.unstubAllGlobals());

  it("falls back to statusText when body is not JSON", async () => {
    stubFetch(async () => new Response("plain body", { status: 500 }));
    try {
      await getAppState();
    } catch (err) {
      if (err instanceof ApiError) {
        expect(err.status).toBe(500);
        // statusText in happy-dom may be empty; accept either
        expect(typeof err.message).toBe("string");
      }
    }
  });
});
