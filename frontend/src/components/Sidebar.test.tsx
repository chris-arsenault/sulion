import { afterEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
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
import { appCommands, subscribeToAppCommands } from "../state/AppCommands";
import { appStatePayload, jsonResponse } from "../test/appState";

const REPO_ALPHA = "alpha";
const REPO_ALPHA_PATH = "/tmp/alpha";

interface MockState {
  sessions: Array<Record<string, unknown>>;
  repos: Array<Record<string, unknown>>;
  workspaces: Array<Record<string, unknown>>;
  secrets: Array<Record<string, unknown>>;
  grantsBySession: Record<string, Array<Record<string, unknown>>>;
  createSessionCalls: Array<unknown>;
  createRepoCalls: Array<unknown>;
  deletedIds: string[];
  deletedWorkspaceRequests: Array<{ id: string; query: string }>;
  patches: Array<{ id: string; body: unknown }>;
  unlocks: Array<unknown>;
  revokes: Array<unknown>;
}

function installFetchMock(): MockState {
  const state: MockState = {
    sessions: [],
    repos: [],
    workspaces: [],
    secrets: [],
    grantsBySession: {},
    createSessionCalls: [],
    createRepoCalls: [],
    deletedIds: [],
    deletedWorkspaceRequests: [],
    patches: [],
    unlocks: [],
    revokes: [],
  };

  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo, init?: RequestInit) => {
      const url = typeof input === "string" ? input : (input as Request).url;
      const method = init?.method ?? "GET";
      const jsonResp = jsonResponse;

      if (url === "/api/app-state" && method === "GET") {
        return jsonResp(
          appStatePayload({
            sessions: state.sessions,
            repos: state.repos,
            workspaces: state.workspaces,
          }),
        );
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
      if (url.startsWith("/api/workspaces/") && method === "DELETE") {
        const [path, query = ""] = url.split("?");
        const id = path.split("/").pop()!;
        state.workspaces = state.workspaces.filter((w) => w.id !== id);
        state.deletedWorkspaceRequests.push({ id, query });
        return new Response(null, { status: 204 });
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
      if (url === "/api/repos" && method === "POST") {
        const body = JSON.parse(init!.body as string);
        state.createRepoCalls.push(body);
        const r = { name: body.name, path: `/tmp/${body.name}` };
        state.repos.push(r);
        return jsonResp(r, 201);
      }
      if (url.match(/^\/api\/repos\/[^/]+\/files/) && method === "GET") {
        return jsonResp({ path: "", entries: [] });
      }
      if ((url === "/api/library/references" || url === "/api/library/prompts") && method === "GET") {
        return jsonResp([]);
      }
      if (url === "/broker/v1/secrets" && method === "GET") {
        return jsonResp(state.secrets);
      }
      if (url.startsWith("/broker/v1/grants?") && method === "GET") {
        const qs = new URLSearchParams(url.split("?")[1]);
        return jsonResp(state.grantsBySession[qs.get("pty_session_id") ?? ""] ?? []);
      }
      if (url === "/broker/v1/grants" && method === "POST") {
        const body = JSON.parse(init!.body as string);
        state.unlocks.push(body);
        const grant = {
          secret_id: body.secret_id,
          tool: body.tool,
          granted_by_sub: "user",
          granted_by_username: null,
          expires_at: new Date(Date.now() + body.ttl_seconds * 1000).toISOString(),
        };
        const current = state.grantsBySession[body.pty_session_id] ?? [];
        state.grantsBySession[body.pty_session_id] = [...current, grant];
        return new Response(null, { status: 201 });
      }
      if (url === "/broker/v1/grants" && method === "DELETE") {
        const body = JSON.parse(init!.body as string);
        state.revokes.push(body);
        state.grantsBySession[body.pty_session_id] = (
          state.grantsBySession[body.pty_session_id] ?? []
        ).filter(
          (grant) =>
            grant.secret_id !== body.secret_id || grant.tool !== body.tool,
        );
        return new Response(null, { status: 204 });
      }
      return new Response("", { status: 404 });
    }),
  );
  return state;
}

function workspaceFixture(overrides: Record<string, unknown> = {}) {
  return {
    id: "99999999-9999-9999-9999-999999999999",
    repo_name: REPO_ALPHA,
    kind: "worktree",
    path: "/tmp/workspaces/alpha/99999999-9999-9999-9999-999999999999",
    branch_name: "sulion/alpha/999999999999",
    base_ref: "main",
    base_sha: "aaaaaaaaaaaa",
    merge_target: "main",
    created_by_session_id: null,
    state: "active",
    created_at: new Date(Date.now() - 60_000).toISOString(),
    updated_at: new Date().toISOString(),
    git: {
      revision: 1,
      branch: "sulion/alpha/999999999999",
      uncommitted_count: 0,
      untracked_count: 0,
      last_commit: null,
      recent_commits: [],
      refreshing: false,
      status_error: null,
    },
    ...overrides,
  };
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

  async function hoverMenuItem(
    user: ReturnType<typeof userEvent.setup>,
    name: string | RegExp,
  ) {
    const item = await screen.findByRole("menuitem", { name });
    await user.hover(item);
    return item;
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

  it("leaves the Files subsection collapsed when a reveal-file fires while it is closed", async () => {
    const state = installFetchMock();
    state.repos.push({ name: REPO_ALPHA, path: REPO_ALPHA_PATH });
    setup();
    const user = userEvent.setup();
    await waitFor(() => expect(screen.getByText(REPO_ALPHA)).toBeDefined());
    await user.click(screen.getByRole("button", { name: REPO_ALPHA }));

    // The Files label exists in the header, but the tree body should
    // not render until the subsection is opened. Initial state is
    // closed; if the reveal listener wrongly force-opened it, the
    // hidden upload input (only present inside FileTree) would appear.
    expect(screen.queryByText(/show all \(incl\. ignored\)/i)).toBeNull();

    act(() => {
      appCommands.revealFile({ repo: REPO_ALPHA, path: "src/lib.rs" });
    });

    // Files should still be collapsed — the tree body remains absent.
    expect(screen.queryByText(/show all \(incl\. ignored\)/i)).toBeNull();
  });

  it("does not reopen a manually collapsed repo for reveal commands", async () => {
    const state = installFetchMock();
    state.repos.push({ name: REPO_ALPHA, path: REPO_ALPHA_PATH });
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
    const user = userEvent.setup();

    await waitFor(() => expect(screen.getByText(/11111111/)).toBeDefined());
    await user.click(screen.getByRole("button", { name: REPO_ALPHA }));
    expect(screen.queryByText(/11111111/)).toBeNull();

    act(() => {
      appCommands.revealRepo({ repo: REPO_ALPHA });
      appCommands.revealFile({ repo: REPO_ALPHA, path: "src/lib.rs" });
    });

    expect(screen.queryByText(/11111111/)).toBeNull();
  });

  it("keeps repo collapse state across sidebar remounts", async () => {
    const state = installFetchMock();
    state.repos.push({ name: REPO_ALPHA, path: REPO_ALPHA_PATH });
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
    const rendered = setup();
    const user = userEvent.setup();

    await waitFor(() => expect(screen.getByText(/11111111/)).toBeDefined());
    await user.click(screen.getByRole("button", { name: REPO_ALPHA }));
    expect(screen.queryByText(/11111111/)).toBeNull();

    rendered.unmount();
    setup();

    await waitFor(() => expect(screen.getByText(REPO_ALPHA)).toBeDefined());
    expect(screen.queryByText(/11111111/)).toBeNull();
  });

  it("collapses empty repos first, then all repos", async () => {
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
    const user = userEvent.setup();

    await waitFor(() => expect(screen.getByText(/11111111/)).toBeDefined());
    await user.click(screen.getByRole("button", { name: "beta" }));
    expect(screen.getByText("— no sessions —")).toBeDefined();

    await user.click(
      screen.getByRole("button", { name: "Collapse repos without sessions" }),
    );
    expect(screen.queryByText("— no sessions —")).toBeNull();
    expect(screen.getByText(/11111111/)).toBeDefined();

    await user.click(screen.getByRole("button", { name: "Collapse all repos" }));
    expect(screen.queryByText(/11111111/)).toBeNull();
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
    await user.click(screen.getByRole("button", { name: REPO_ALPHA }));

    // Find the "+" button scoped to the alpha group.
    const spawnBtn = screen.getByLabelText("New session in alpha");
    await user.click(spawnBtn);
    const submitBtn = screen.getByRole("button", { name: /spawn/i });
    await user.click(submitBtn);

    await waitFor(() => {
      expect(state.createSessionCalls.length).toBe(1);
    });
    expect(state.createSessionCalls[0]).toEqual({
      repo: REPO_ALPHA,
      workspace_mode: "isolated",
    });
  });

  it("shows isolated workspaces and opens their workspace diff", async () => {
    const state = installFetchMock();
    const workspace = workspaceFixture({
      git: {
        revision: 1,
        branch: "sulion/alpha/999999999999",
        uncommitted_count: 2,
        untracked_count: 1,
        last_commit: null,
        recent_commits: [],
        refreshing: false,
        status_error: null,
      },
    });
    state.repos.push({ name: REPO_ALPHA, path: REPO_ALPHA_PATH });
    state.workspaces.push(workspace);
    setup();
    const user = userEvent.setup();

    const label = await screen.findByText("alpha/999999999999");
    expect(screen.getByText(/2 dirty/)).toBeDefined();
    await user.click(label.closest("button")!);

    const tab = Object.values(useTabStore.getState().tabs).find(
      (item) =>
        item.kind === "diff" &&
        item.repo === REPO_ALPHA &&
        item.workspaceId === workspace.id,
    );
    expect(tab).toBeDefined();
  });

  it("resumes an isolated workspace from its row action", async () => {
    const state = installFetchMock();
    const workspace = workspaceFixture();
    state.repos.push({ name: REPO_ALPHA, path: REPO_ALPHA_PATH });
    state.workspaces.push(workspace);
    setup();
    const user = userEvent.setup();

    await user.click(
      await screen.findByLabelText("Resume workspace alpha/999999999999"),
    );

    await waitFor(() => expect(state.createSessionCalls).toHaveLength(1));
    expect(state.createSessionCalls[0]).toEqual({
      repo: REPO_ALPHA,
      workspace_id: workspace.id,
    });
  });

  it("deletes an isolated workspace from its row action", async () => {
    const state = installFetchMock();
    const workspace = workspaceFixture();
    state.repos.push({ name: REPO_ALPHA, path: REPO_ALPHA_PATH });
    state.workspaces.push(workspace);
    setup();
    const user = userEvent.setup();

    await user.click(
      await screen.findByLabelText("Delete workspace alpha/999999999999"),
    );
    await user.click(await screen.findByRole("button", { name: "Delete" }));

    await waitFor(() =>
      expect(state.deletedWorkspaceRequests).toEqual([
        { id: workspace.id, query: "" },
      ]),
    );
    expect(screen.queryByText("alpha/999999999999")).toBeNull();
  });

  it("force deletes an isolated workspace from its context menu", async () => {
    const state = installFetchMock();
    const workspace = workspaceFixture();
    state.repos.push({ name: REPO_ALPHA, path: REPO_ALPHA_PATH });
    state.workspaces.push(workspace);
    setup();
    const user = userEvent.setup();

    await user.pointer({
      keys: "[MouseRight]",
      target: await screen.findByText("alpha/999999999999"),
    });
    await user.click(
      await screen.findByRole("menuitem", { name: "Force delete workspace" }),
    );
    await user.click(
      await screen.findByRole("button", { name: "Force delete" }),
    );

    await waitFor(() =>
      expect(state.deletedWorkspaceRequests).toEqual([
        { id: workspace.id, query: "force=true" },
      ]),
    );
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

  it("enables and revokes a secret grant from the session context menu", async () => {
    const sessionId = "88888888-8888-8888-8888-888888888888";
    const state = installFetchMock();
    state.repos.push({ name: REPO_ALPHA, path: REPO_ALPHA_PATH });
    state.secrets.push({
      id: "claude-api",
      description: "Claude",
      scope: "global",
      repo: null,
      env_keys: ["ANTHROPIC_API_KEY"],
      updated_at: "2026-04-24T00:00:00Z",
    });
    state.sessions.push({
      id: sessionId,
      repo: REPO_ALPHA,
      working_dir: REPO_ALPHA_PATH,
      state: "live",
      created_at: new Date().toISOString(),
      ended_at: null,
      exit_code: null,
      current_session_uuid: null,
      current_session_agent: null,
      last_event_at: null,
      label: "secret-session",
      pinned: false,
      color: null,
    });
    setup();
    const user = userEvent.setup();

    await waitFor(() => expect(screen.getByText("secret-session")).toBeDefined());
    await openSessionContextMenu(user, /secret-session/);
    await hoverMenuItem(user, "Secrets");
    await hoverMenuItem(user, "Enable secret");
    await hoverMenuItem(user, "claude-api");
    await hoverMenuItem(user, "with-cred");
    await user.click(await screen.findByRole("menuitem", { name: "10m" }));

    await waitFor(() => expect(state.unlocks).toHaveLength(1));
    expect(state.unlocks[0]).toEqual({
      pty_session_id: sessionId,
      secret_id: "claude-api",
      tool: "with-cred",
      ttl_seconds: 600,
    });

    await openSessionContextMenu(user, /secret-session/);
    await hoverMenuItem(user, "Secrets");
    await hoverMenuItem(user, "Active secrets");
    await user.click(await screen.findByRole("menuitem", { name: /claude-api · with-cred/ }));

    await waitFor(() => expect(state.revokes).toHaveLength(1));
    expect(state.revokes[0]).toEqual({
      pty_session_id: sessionId,
      secret_id: "claude-api",
      tool: "with-cred",
    });
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

  it("hard refresh reloads a single repo file tree", async () => {
    let filesCalls = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo, init?: RequestInit) => {
        const url = typeof input === "string" ? input : (input as Request).url;
        const method = init?.method ?? "GET";
        const jsonResp = jsonResponse;

        if (url === "/api/app-state" && method === "GET") {
          return jsonResp(
            appStatePayload({
              repos: [{ name: REPO_ALPHA, path: REPO_ALPHA_PATH }],
            }),
          );
        }
        if ((url === "/api/library/references" || url === "/api/library/prompts") && method === "GET") {
          return jsonResp([]);
        }
        if (url === "/api/repos/alpha/files" && method === "GET") {
          filesCalls += 1;
          const name = filesCalls === 1 ? "first.txt" : "second.txt";
          return jsonResp({
            path: "",
            entries: [
              {
                name,
                kind: "file",
                size: 1,
                mtime: null,
                dirty: null,
                diff: null,
              },
            ],
          });
        }
        if (url === "/api/repos/alpha/refresh" && method === "POST") {
          return new Response(null, { status: 202 });
        }
        return new Response("", { status: 404 });
      }),
    );

    setup();
    const user = userEvent.setup();

    await waitFor(() => expect(screen.getByText(REPO_ALPHA)).toBeDefined());
    await user.click(screen.getByRole("button", { name: REPO_ALPHA }));
    await user.click(screen.getByRole("button", { name: /files/i }));

    await waitFor(() => {
      expect(screen.getByText("first.txt")).toBeDefined();
    });

    await user.click(screen.getByLabelText("Hard refresh alpha"));

    await waitFor(() => {
      expect(screen.getByText("second.txt")).toBeDefined();
    });
  });
});
