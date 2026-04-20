import { expect, type APIRequestContext, type Locator, type Page } from "@playwright/test";

interface SessionView {
  id: string;
  label: string | null;
  current_session_agent: string | null;
}

interface DirListing {
  entries: Array<{ name: string; kind: "file" | "dir" }>;
}

interface FileResponse {
  content: string | null;
}

interface LibraryEntry {
  slug: string;
}

export async function gotoApp(page: Page) {
  await page.goto("/");
  const drawerButton = page.getByRole("button", { name: "Open sessions drawer" });
  if (await drawerButton.isVisible().catch(() => false)) {
    await expect(drawerButton).toBeVisible();
    return;
  }
  await expect(page.locator('[data-repo-name="atlas"]')).toBeVisible();
  await expect(page.locator('[data-repo-name="zephyr"]')).toBeVisible();
}

export async function openSession(page: Page, label: string) {
  await page.locator(`[data-session-name="${label}"]`).click();
  await expect(
    page.getByRole("tab", { name: new RegExp(`${escapeRegex(label)} · term`, "i") }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(
    page.getByRole("tab", { name: new RegExp(`${escapeRegex(label)} · time`, "i") }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(page.locator('[data-testid="terminal-pane"]:visible')).toBeVisible();
  await expect(page.locator('[data-testid="timeline-pane"]:visible')).toBeVisible();
}

export async function createMockTerminalSession(
  request: APIRequestContext,
  label: string,
) {
  const session = await createSession(request, {
    repo: "atlas",
    e2e_fixture: "mock-terminal",
  });
  await patchSession(request, session.id, { label, pinned: false, color: null });
  return session.id;
}

export async function createSession(
  request: APIRequestContext,
  data: Record<string, unknown>,
) {
  const response = await request.post("/api/sessions", { data });
  expect(response.ok()).toBeTruthy();
  return (await response.json()) as SessionView;
}

export async function createLabeledSession(
  request: APIRequestContext,
  repo: string,
  label: string,
) {
  const session = await createSession(request, { repo });
  await patchSession(request, session.id, { label, pinned: false, color: null });
  return session.id;
}

export async function deleteSessionById(request: APIRequestContext, id: string) {
  const response = await request.delete(`/api/sessions/${id}`);
  expect(response.ok()).toBeTruthy();
}

export async function dropSessionWebSocket(request: APIRequestContext, id: string) {
  const response = await request.post(`/api/sessions/${id}/e2e/drop-ws`);
  expect(response.ok()).toBeTruthy();
}

export function repoGroup(page: Page, repo: string) {
  return page.locator(`[data-repo-name="${repo}"]`);
}

export async function ensureFilesOpen(page: Page, repo: string) {
  const group = repoGroup(page, repo);
  const tree = group.locator(".sidebar__tree-body");
  if (await tree.isVisible().catch(() => false)) {
    return;
  }
  await group.getByRole("button", { name: /^Files/ }).click();
  await expect(tree).toBeVisible();
}

export async function openTreeFile(page: Page, repo: string, filePath: string) {
  await ensureFilesOpen(page, repo);
  await expandTreePath(page, repo, filePath);
  const row = treeRow(page, repo, filePath);
  await expect(row).toBeVisible();
  await row.click();
}

export function treeRow(page: Page, repo: string, filePath: string) {
  return page.locator(`.sidebar__tree-row[data-repo="${repo}"][data-path="${filePath}"]`);
}

export function tab(page: Page, kind: string, match?: string) {
  const base = page.locator(`[role="tab"][data-kind="${kind}"]`);
  return match ? base.filter({ hasText: match }) : base;
}

export function visibleTimelinePane(page: Page) {
  return page.locator('[data-testid="timeline-pane"]:visible').first();
}

export async function openContextMenu(locator: Locator) {
  await locator.click({ button: "right" });
}

export async function selectMenuItem(page: Page, name: string) {
  await page.getByRole("menuitem", { name }).click();
}

export async function terminalText(page: Page) {
  const mirror = page
    .locator('[data-testid="terminal-pane"]:visible')
    .locator('[data-testid="terminal-mirror"]');
  if (await mirror.count()) {
    return (await mirror.first().textContent()) ?? "";
  }
  return page.locator('.terminal-pane__host:visible').evaluate((host) => {
    const rows = Array.from(host.querySelectorAll(".xterm-rows > div"))
      .map((row) => row.textContent ?? "")
      .join("\n");
    const accessibility = Array.from(host.querySelectorAll(".xterm-accessibility-tree"))
      .map((node) => node.textContent ?? "")
      .join("\n");
    return `${rows}\n${accessibility}`;
  });
}

export async function focusTerminal(page: Page) {
  await page.locator('.terminal-pane__host:visible').click({ position: { x: 24, y: 24 } });
}

export async function runTerminalCommand(page: Page, command: string) {
  await focusTerminal(page);
  await page.keyboard.type(command);
  await page.keyboard.press("Enter");
}

export async function pressTerminalEnter(page: Page) {
  await focusTerminal(page);
  await page.keyboard.press("Enter");
}

export async function pasteIntoTerminal(page: Page, text: string) {
  await page.locator('.terminal-pane__host:visible').evaluate((host, payload) => {
    const textarea = host.querySelector("textarea");
    if (!(textarea instanceof HTMLTextAreaElement)) {
      throw new Error("terminal textarea not found");
    }
    const event = new Event("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "clipboardData", {
      value: {
        getData: (kind: string) => (kind === "text/plain" ? payload : ""),
      },
    });
    textarea.dispatchEvent(event);
  }, text);
}

export async function expectTerminalToContain(page: Page, text: string) {
  await expect
    .poll(async () => terminalText(page), {
      message: `expected terminal to contain ${text}`,
    })
    .toContain(text);
}

export async function expectTerminalNotToContain(page: Page, text: string) {
  await expect
    .poll(async () => terminalText(page), {
      message: `expected terminal not to contain ${text}`,
    })
    .not.toContain(text);
}

export async function restoreSeededSessionMetadata(request: APIRequestContext) {
  const sessions = await listSessions(request);
  for (const session of sessions) {
    if (session.current_session_agent === "claude-code") {
      await patchSession(request, session.id, {
        label: "Atlas Claude",
        pinned: false,
        color: null,
      });
    }
    if (session.current_session_agent === "codex") {
      await patchSession(request, session.id, {
        label: "Atlas Codex",
        pinned: false,
        color: null,
      });
    }
  }
}

export async function deleteLibraryEntries(
  request: APIRequestContext,
  kind: "prompts" | "references",
) {
  const response = await request.get(`/api/library/${kind}`);
  expect(response.ok()).toBeTruthy();
  const entries = (await response.json()) as LibraryEntry[];
  for (const entry of entries) {
    await request.delete(`/api/library/${kind}/${encodeURIComponent(entry.slug)}`);
  }
}

export async function listRepoEntries(
  request: APIRequestContext,
  repo: string,
  path = "",
  all = false,
) {
  const params = new URLSearchParams();
  if (path) params.set("path", path);
  if (all) params.set("all", "true");
  const suffix = params.toString();
  const url = suffix
    ? `/api/repos/${encodeURIComponent(repo)}/files?${suffix}`
    : `/api/repos/${encodeURIComponent(repo)}/files`;
  const response = await request.get(
    url,
  );
  expect(response.ok()).toBeTruthy();
  return (await response.json()) as DirListing;
}

export async function readRepoFile(
  request: APIRequestContext,
  repo: string,
  path: string,
) {
  const response = await request.get(
    `/api/repos/${encodeURIComponent(repo)}/file?path=${encodeURIComponent(path)}`,
  );
  expect(response.ok()).toBeTruthy();
  return (await response.json()) as FileResponse;
}

async function listSessions(request: APIRequestContext) {
  const response = await request.get("/api/sessions");
  expect(response.ok()).toBeTruthy();
  const body = (await response.json()) as { sessions: SessionView[] };
  return body.sessions;
}

async function patchSession(
  request: APIRequestContext,
  id: string,
  body: { label: string | null; pinned: boolean; color: string | null },
) {
  const response = await request.patch(`/api/sessions/${id}`, { data: body });
  expect(response.ok()).toBeTruthy();
}

function escapeRegex(value: string) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

async function expandTreePath(page: Page, repo: string, filePath: string) {
  const parts = filePath.split("/");
  let current = "";
  for (let index = 0; index < parts.length - 1; index += 1) {
    current = current ? `${current}/${parts[index]}` : parts[index];
    const dirRow = treeRow(page, repo, current);
    await expect(dirRow).toBeVisible();

    const nextPath =
      current === filePath ? current : `${current}/${parts[index + 1]}`;
    const nextRow = treeRow(page, repo, nextPath);
    if (await nextRow.isVisible().catch(() => false)) {
      continue;
    }

    await dirRow.click();
    await expect(nextRow).toBeVisible();
  }
}
