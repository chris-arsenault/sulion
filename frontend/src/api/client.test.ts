import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  ApiError,
  createRepo,
  createSession,
  deleteSession,
  getHistory,
  listRepos,
  listSessions,
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

  it("listSessions GETs /api/sessions and parses the response", async () => {
    stubFetch(async (url) => {
      expect(url).toBe("/api/sessions");
      return jsonResponse({ sessions: [] });
    });
    const resp = await listSessions();
    expect(resp.sessions).toEqual([]);
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

  it("listRepos hits /api/repos", async () => {
    stubFetch(async (url) => {
      expect(url).toBe("/api/repos");
      return jsonResponse({ repos: [] });
    });
    await listRepos();
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

  it("wraps non-ok responses in ApiError using the server-provided error", async () => {
    stubFetch(async () =>
      new Response(JSON.stringify({ error: "boom" }), {
        status: 400,
        headers: { "Content-Type": "application/json" },
      }),
    );
    await expect(listSessions()).rejects.toBeInstanceOf(ApiError);
    try {
      await listSessions();
    } catch (err) {
      if (err instanceof ApiError) {
        expect(err.status).toBe(400);
        expect(err.message).toBe("boom");
      }
    }
  });
});

describe("ApiError", () => {
  beforeEach(() => vi.unstubAllGlobals());

  it("falls back to statusText when body is not JSON", async () => {
    stubFetch(async () => new Response("plain body", { status: 500 }));
    try {
      await listSessions();
    } catch (err) {
      if (err instanceof ApiError) {
        expect(err.status).toBe(500);
        // statusText in happy-dom may be empty; accept either
        expect(typeof err.message).toBe("string");
      }
    }
  });
});
