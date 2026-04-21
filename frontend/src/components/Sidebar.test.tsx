import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("./LibraryPanel", () => ({
  LibraryPanel: () => null,
}));

vi.mock("./StatsStrip", () => ({
  StatsStrip: () => null,
}));

vi.mock("./ui", async () => {
  const actual = await vi.importActual<typeof import("./ui")>("./ui");
  return {
    ...actual,
    Tooltip: ({ children }: { children: unknown }) => children,
  };
});

import { Sidebar } from "./Sidebar";
import { ContextMenuHost } from "./common/ContextMenu";
import { resetTabStore, useTabStore } from "../state/TabStore";
import { subscribeToAppCommands } from "../state/AppCommands";

const REPO_ALPHA = "alpha";
const REPO_ALPHA_PATH = "/tmp/alpha";

type Endpoint = "/api/sessions" | "/api/repos" | string;

interface MockState {
  sessions: Array<Record<string, unknown>>;
  repos: Array<Record<string, unknown>>;
  createSessionCalls: Array<unknown>;
  createRepoCalls: Array<unknown>;
  deletedIds: string[];
  patches: Array<{ id: string; body: unknown }>;
}

function installFetchMock(): MockState {
  const state: MockState = {
    sessions: [],
    repos: [],
    createSessionCalls: [],
    createRepoCalls: [],
    deletedIds: [],
    patches: [],
  };

  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo, init?: RequestInit) => {
      const url: Endpoint =
        typeof input === "string" ? input : (input as Request).url;
      const method = init?.method ?? "GET";
      const jsonResp = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        });

      if (url === "/api/sessions" && method === "GET") {
        return jsonResp({ sessions: state.sessions });
      }
      if (url === "/api/sessions" && method === "POST") {
        const body = JSON.parse(init!.body as string);
        state.createSessionCalls.push(body);
        const s = {
          id: `00000000-0000-0000-0000-${String(
            state.sessions.length,
          ).padStart(12, "0")}`,
          repo: body.repo,
          working_dir: body.working_dir ?? `/tmp/${body.repo}`,
          state: "live",
          created_at: new Date().toISOString(),
          ended_at: null,
          exit_code: null,
          current_session_uuid: null,
          current_session_agent: null,
        };
        state.sessions.push(s);
        return jsonResp(s, 201);
      }
      if (url.startsWith("/api/sessions/") && method === "DELETE") {
        const id = url.split("/").pop()!;
        state.sessions = state.sessions.filter((s) => s.id !== id);
        state.deletedIds.push(id);
        return new Response(null, { status: 204 });
      }
      if (url.startsWith("/api/sessions/") && method === "PATCH") {
        const id = url.split("/").pop()!;
        const body = JSON.parse(init!.body as string);
        state.patches.push({ id, body });
        state.sessions = state.sessions.map((s) =>
          s.id === id ? { ...s, ...body } : s,
        );
        return new Response(null, { status: 204 });
      }
      if (url === "/api/repos" && method === "GET") {
        return jsonResp({ repos: state.repos });
      }
      if (url === "/api/repos" && method === "POST") {
        const body = JSON.parse(init!.body as string);
        state.createRepoCalls.push(body);
        const r = { name: body.name, path: `/tmp/${body.name}` };
        state.repos.push(r);
        return jsonResp(r, 201);
      }
      // New in ticket 1: repo git + files endpoints. Return empty
      // shapes so the nav renders without errors.
      if (url.match(/^\/api\/repos\/[^/]+\/git$/) && method === "GET") {
        return jsonResp({
          branch: "main",
          uncommitted_count: 0,
          untracked_count: 0,
          last_commit: null,
          recent_commits: [],
          dirty_by_path: {},
          diff_stats_by_path: {},
        });
      }
      if (url.match(/^\/api\/repos\/[^/]+\/files/) && method === "GET") {
        return jsonResp({ path: "", entries: [] });
      }
      if (url === "/api/stats" && method === "GET") {
        return jsonResp({
          uptime_seconds: 1,
          process: { memory_rss_bytes: 0, cpu_percent: 0, memory_limit_bytes: null },
          pty: { live_sessions: 0, live_agent_sessions: 0 },
          db: {
            database_size_bytes: 0,
          },
          inventory: {
            event_rows: 0,
            agent_sessions: 0,
            pty_sessions: 0,
            tracked_files: 0,
            events_inserted_since_boot: 0,
            parse_errors_since_boot: 0,
          },
        });
      }
      if ((url === "/api/library/references" || url === "/api/library/prompts") && method === "GET") {
        return jsonResp([]);
      }
      return new Response("", { status: 404 });
    }),
  );
  return state;
}

describe("Sidebar", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    resetTabStore();
  });

  function setup() {
    return render(
      <>
        <Sidebar />
        <ContextMenuHost />
      </>,
    );
  }

  async function openSessionContextMenu(user: ReturnType<typeof userEvent.setup>, text: RegExp) {
    await user.pointer({
      keys: "[MouseRight]",
      target: screen.getByText(text),
    });
  }

  async function openRepoContextMenu(user: ReturnType<typeof userEvent.setup>, text: string) {
    await user.pointer({
      keys: "[MouseRight]",
      target: screen.getByText(text),
    });
  }

  it("renders repo groups and their sessions", async () => {
    const state = installFetchMock();
    state.repos.push({ name: REPO_ALPHA, path: REPO_ALPHA_PATH });
    state.repos.push({ name: "beta", path: "/tmp/beta" });
    state.sessions.push({
      id: "11111111-1111-1111-1111-111111111111",
      repo: REPO_ALPHA,
      working_dir: REPO_ALPHA_PATH,
      state: "live",
      created_at: new Date().toISOString(),
      ended_at: null,
      exit_code: null,
      current_session_uuid: null,
      current_session_agent: null,
    });
    setup();
    await waitFor(() => {
      expect(screen.getByText(REPO_ALPHA)).toBeDefined();
      expect(screen.getByText("beta")).toBeDefined();
    });
    await waitFor(() => {
      expect(screen.getByText(/11111111/)).toBeDefined();
    });
  });

  it("shows the future-prompts badge when the session has queued pending entries", async () => {
    const state = installFetchMock();
    state.repos.push({ name: REPO_ALPHA, path: REPO_ALPHA_PATH });
    state.sessions.push({
      id: "44444444-4444-4444-4444-444444444444",
      repo: REPO_ALPHA,
      working_dir: REPO_ALPHA_PATH,
      state: "live",
      created_at: new Date().toISOString(),
      ended_at: null,
      exit_code: null,
      current_session_uuid: "99999999-9999-9999-9999-999999999999",
      current_session_agent: "claude-code",
      future_prompts_pending_count: 3,
    });
    state.sessions.push({
      id: "55555555-5555-5555-5555-555555555555",
      repo: REPO_ALPHA,
      working_dir: REPO_ALPHA_PATH,
      state: "live",
      created_at: new Date().toISOString(),
      ended_at: null,
      exit_code: null,
      current_session_uuid: null,
      current_session_agent: null,
      future_prompts_pending_count: 0,
    });
    setup();

    const badge = await waitFor(() =>
      screen.getByLabelText("3 queued future prompts"),
    );
    expect(badge.textContent).toContain("3");
    // The second session has 0 queued prompts, so no second badge.
    expect(screen.getAllByTestId("session-future-prompts-badge")).toHaveLength(
      1,
    );
  });

  it("new-session inline form creates a session via the API", async () => {
    const state = installFetchMock();
    state.repos.push({ name: REPO_ALPHA, path: REPO_ALPHA_PATH });
    setup();
    const user = userEvent.setup();

    await waitFor(() => expect(screen.getByText(REPO_ALPHA)).toBeDefined());

    // Find the "+" button scoped to the alpha group.
    const spawnBtn = screen.getByLabelText("New session in alpha");
    await user.click(spawnBtn);
    const submitBtn = screen.getByRole("button", { name: /spawn/i });
    await user.click(submitBtn);

    await waitFor(() => {
      expect(state.createSessionCalls.length).toBe(1);
    });
    expect(state.createSessionCalls[0]).toEqual({ repo: REPO_ALPHA });
  });

  it("deletes a session from the shared context menu", async () => {
    const state = installFetchMock();
    state.repos.push({ name: REPO_ALPHA, path: REPO_ALPHA_PATH });
    state.sessions.push({
      id: "22222222-2222-2222-2222-222222222222",
      repo: REPO_ALPHA,
      working_dir: REPO_ALPHA_PATH,
      state: "live",
      created_at: new Date().toISOString(),
      ended_at: null,
      exit_code: null,
      current_session_uuid: null,
      current_session_agent: null,
    });
    setup();
    const user = userEvent.setup();

    await waitFor(() => expect(screen.getByText(/22222222/)).toBeDefined());

    await openSessionContextMenu(user, /22222222/);
    await user.click(screen.getByRole("menuitem", { name: /delete session/i }));

    // ConfirmDialog appears with a red Delete button; click it.
    const confirmBtn = await waitFor(() =>
      screen.getByRole("button", { name: "Delete" }),
    );
    await user.click(confirmBtn);

    await waitFor(() => {
      expect(state.deletedIds.length).toBe(1);
    });
  });

  it("cancelling the delete dialog does not fire DELETE", async () => {
    const state = installFetchMock();
    state.repos.push({ name: REPO_ALPHA, path: REPO_ALPHA_PATH });
    state.sessions.push({
      id: "33333333-3333-3333-3333-333333333333",
      repo: REPO_ALPHA,
      working_dir: REPO_ALPHA_PATH,
      state: "live",
      created_at: new Date().toISOString(),
      ended_at: null,
      exit_code: null,
      current_session_uuid: null,
      current_session_agent: null,
    });
    setup();
    const user = userEvent.setup();

    await waitFor(() => expect(screen.getByText(/33333333/)).toBeDefined());
    await openSessionContextMenu(user, /33333333/);
    await user.click(screen.getByRole("menuitem", { name: /delete session/i }));
    const cancelBtn = await waitFor(() =>
      screen.getByRole("button", { name: "Cancel" }),
    );
    await user.click(cancelBtn);

    // Give any stray requests a moment to fire — none should.
    await new Promise((r) => setTimeout(r, 50));
    expect(state.deletedIds.length).toBe(0);
  });

  it("fires PATCH { pinned: true } on Pin to top from the context menu", async () => {
    const state = installFetchMock();
    state.repos.push({ name: REPO_ALPHA, path: REPO_ALPHA_PATH });
    state.sessions.push({
      id: "44444444-4444-4444-4444-444444444444",
      repo: REPO_ALPHA,
      working_dir: REPO_ALPHA_PATH,
      state: "live",
      created_at: new Date().toISOString(),
      ended_at: null,
      exit_code: null,
      current_session_uuid: null,
      current_session_agent: null,
      last_event_at: null,
      label: null,
      pinned: false,
      color: null,
    });
    setup();
    const user = userEvent.setup();

    await waitFor(() => expect(screen.getByText(/44444444/)).toBeDefined());

    await openSessionContextMenu(user, /44444444/);
    await user.click(screen.getByRole("menuitem", { name: /pin to top/i }));

    await waitFor(() => expect(state.patches.length).toBe(1));
    expect(state.patches[0]).toEqual({
      id: "44444444-4444-4444-4444-444444444444",
      body: { pinned: true },
    });
  });

  it("opens a repo timeline tab from the repo context menu", async () => {
    const state = installFetchMock();
    state.repos.push({ name: REPO_ALPHA, path: REPO_ALPHA_PATH });
    setup();
    const user = userEvent.setup();

    await waitFor(() => expect(screen.getByText(REPO_ALPHA)).toBeDefined());
    await openRepoContextMenu(user, REPO_ALPHA);
    await user.click(screen.getByRole("menuitem", { name: /open repo timeline/i }));

    const tab = Object.values(useTabStore.getState().tabs).find(
      (item) => item.kind === "timeline" && item.repo === REPO_ALPHA,
    );
    expect(tab).toBeDefined();
  });

  it("dispatches the future-prompts command from the session context menu", async () => {
    const state = installFetchMock();
    state.repos.push({ name: REPO_ALPHA, path: REPO_ALPHA_PATH });
    state.sessions.push({
      id: "77777777-7777-7777-7777-777777777777",
      repo: REPO_ALPHA,
      working_dir: REPO_ALPHA_PATH,
      state: "live",
      created_at: new Date().toISOString(),
      ended_at: null,
      exit_code: null,
      current_session_uuid: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
      current_session_agent: "codex",
    });
    const seen: Array<unknown> = [];
    const unsubscribe = subscribeToAppCommands((command) => {
      seen.push(command);
    });
    setup();
    const user = userEvent.setup();

    await waitFor(() => expect(screen.getByText(/77777777/)).toBeDefined());
    await openSessionContextMenu(user, /77777777/);
    await user.click(screen.getByRole("menuitem", { name: /future prompts/i }));

    expect(seen).toContainEqual({
      type: "open-future-prompts",
      sessionId: "77777777-7777-7777-7777-777777777777",
    });
    unsubscribe();
  });

  it("renames a session through the menu", async () => {
    const state = installFetchMock();
    state.repos.push({ name: REPO_ALPHA, path: REPO_ALPHA_PATH });
    state.sessions.push({
      id: "55555555-5555-5555-5555-555555555555",
      repo: REPO_ALPHA,
      working_dir: REPO_ALPHA_PATH,
      state: "live",
      created_at: new Date().toISOString(),
      ended_at: null,
      exit_code: null,
      current_session_uuid: null,
      current_session_agent: null,
      last_event_at: null,
      label: null,
      pinned: false,
      color: null,
    });
    setup();
    const user = userEvent.setup();

    await waitFor(() => expect(screen.getByText(/55555555/)).toBeDefined());

    await openSessionContextMenu(user, /55555555/);
    await user.click(screen.getByRole("menuitem", { name: /rename/i }));

    const input = screen.getByLabelText("Session name");
    await user.clear(input);
    await user.type(input, "deploy-work{enter}");

    await waitFor(() => expect(state.patches.length).toBe(1));
    expect(state.patches[0]).toEqual({
      id: "55555555-5555-5555-5555-555555555555",
      body: { label: "deploy-work" },
    });
    // Label replaces the uuid prefix after the optimistic update.
    await waitFor(() => expect(screen.getByText("deploy-work")).toBeDefined());
  });

  it("picks a colour through the context-menu submenu", async () => {
    const state = installFetchMock();
    state.repos.push({ name: REPO_ALPHA, path: REPO_ALPHA_PATH });
    state.sessions.push({
      id: "66666666-6666-6666-6666-666666666666",
      repo: REPO_ALPHA,
      working_dir: REPO_ALPHA_PATH,
      state: "live",
      created_at: new Date().toISOString(),
      ended_at: null,
      exit_code: null,
      current_session_uuid: null,
      current_session_agent: null,
      last_event_at: null,
      label: null,
      pinned: false,
      color: null,
    });
    setup();
    const user = userEvent.setup();

    await waitFor(() => expect(screen.getByText(/66666666/)).toBeDefined());

    await openSessionContextMenu(user, /66666666/);
    await user.hover(screen.getByRole("menuitem", { name: /colour/i }));
    await waitFor(() => expect(screen.getByRole("menuitem", { name: /emerald/i })).toBeDefined());
    await user.click(screen.getByRole("menuitem", { name: /emerald/i }));

    await waitFor(() => expect(state.patches.length).toBe(1));
    expect(state.patches[0]).toEqual({
      id: "66666666-6666-6666-6666-666666666666",
      body: { color: "emerald" },
    });
  });

  it("pinned sessions float to the top of their repo group", async () => {
    const state = installFetchMock();
    state.repos.push({ name: REPO_ALPHA, path: REPO_ALPHA_PATH });
    // Newer session, unpinned.
    state.sessions.push({
      id: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
      repo: REPO_ALPHA,
      working_dir: REPO_ALPHA_PATH,
      state: "live",
      created_at: new Date(Date.now()).toISOString(),
      ended_at: null,
      exit_code: null,
      current_session_uuid: null,
      current_session_agent: null,
      last_event_at: null,
      label: null,
      pinned: false,
      color: null,
    });
    // Older session, pinned — should appear first.
    state.sessions.push({
      id: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
      repo: REPO_ALPHA,
      working_dir: REPO_ALPHA_PATH,
      state: "live",
      created_at: new Date(Date.now() - 3_600_000).toISOString(),
      ended_at: null,
      exit_code: null,
      current_session_uuid: null,
      current_session_agent: null,
      last_event_at: null,
      label: "pinned-one",
      pinned: true,
      color: null,
    });
    setup();

    await waitFor(() => {
      expect(screen.getByText("pinned-one")).toBeDefined();
      expect(screen.getByText(/aaaaaaaa/)).toBeDefined();
    });
    const ids = Array.from(document.querySelectorAll(".sidebar__session-id")).map(
      (el) => el.textContent ?? "",
    );
    const pinnedIdx = ids.findIndex((t) => t.includes("pinned-one"));
    const newerIdx = ids.findIndex((t) => t.includes("aaaaaaaa"));
    expect(pinnedIdx).toBeLessThan(newerIdx);
  });

  it("new-repo form POSTs to /api/repos", async () => {
    const state = installFetchMock();
    setup();
    const user = userEvent.setup();

    await waitFor(() => expect(screen.getByText(/no repos yet/i)).toBeDefined());

    const plus = screen.getByLabelText("New repo");
    await user.click(plus);

    const nameInput = screen.getByPlaceholderText(/repo name/i);
    await user.type(nameInput, "newrepo");
    const createBtn = screen.getByRole("button", { name: /create/i });
    await user.click(createBtn);

    await waitFor(() => {
      expect(state.createRepoCalls.length).toBe(1);
    });
    expect(state.createRepoCalls[0]).toEqual({
      name: "newrepo",
      git_url: undefined,
    });
  });
});
