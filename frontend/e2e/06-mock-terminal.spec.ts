import { expect, test } from "@playwright/test";

import {
  createMockTerminalSession,
  deleteSessionById,
  dropSessionWebSocket,
  expectTerminalToContain,
  gotoApp,
  listRepoEntries,
  openSession,
  pasteIntoTerminal,
  runTerminalCommand,
  readRepoFile,
} from "./helpers";

let mockSessionId: string | null = null;

test.afterEach(async ({ request }) => {
  if (!mockSessionId) return;
  await deleteSessionById(request, mockSessionId);
  mockSessionId = null;
});

test("renders the seeded snapshot, echoes input, streams chunks, and reports resizes", async ({
  page,
  request,
}) => {
  mockSessionId = await createMockTerminalSession(request, "Atlas Mock Snapshot");

  await gotoApp(page);
  await openSession(page, "Atlas Mock Snapshot");

  await expectTerminalToContain(page, "SULION MOCK TERMINAL READY");
  await expectTerminalToContain(page, "SNAPSHOT_SENTINEL");

  await runTerminalCommand(page, "status");
  await expectTerminalToContain(page, "status");
  await expectTerminalToContain(page, "MOCK_STATUS ok");

  await runTerminalCommand(page, "stream");
  await expectTerminalToContain(page, "STREAM_CHUNK_1");
  await expectTerminalToContain(page, "STREAM_CHUNK_2");
  await expectTerminalToContain(page, "STREAM_CHUNK_3");

  await page.setViewportSize({ width: 1440, height: 760 });
  await expectTerminalToContain(page, "MOCK_RESIZE rows=");
});

test("supports paste-as-file and reconnects the websocket without losing the session", async ({
  page,
  request,
}) => {
  mockSessionId = await createMockTerminalSession(request, "Atlas Mock Paste");

  await gotoApp(page);
  await openSession(page, "Atlas Mock Paste");

  const largePaste = Array.from({ length: 220 }, (_, index) => `line-${index}`)
    .join("\n");
  page.once("dialog", (dialog) => dialog.accept());
  await pasteIntoTerminal(page, largePaste);
  await page.waitForTimeout(200);
  await runTerminalCommand(page, "");

  await expectTerminalToContain(page, ".sulion-paste/paste-");

  const uploads = await listRepoEntries(request, "atlas", ".sulion-paste", true);
  const uploaded = uploads.entries.find((entry) => entry.kind === "file");
  expect(uploaded).toBeDefined();
  const uploadedFile = await readRepoFile(request, "atlas", `.sulion-paste/${uploaded!.name}`);
  expect(uploadedFile.content).toContain("line-219");

  await dropSessionWebSocket(request, mockSessionId);
  await page.waitForTimeout(600);

  await runTerminalCommand(page, "status");
  await expectTerminalToContain(page, "MOCK_STATUS ok");
});

test("surfaces terminal exit immediately and on reload via the ended-session UI", async ({
  page,
  request,
}) => {
  mockSessionId = await createMockTerminalSession(request, "Atlas Mock Exit");

  await gotoApp(page);
  await openSession(page, "Atlas Mock Exit");

  await runTerminalCommand(page, "exit");
  await expectTerminalToContain(page, "MOCK_EXIT 7");
  await expect(page.getByText(/shell exited with code 7/i)).toBeVisible();

  await page.reload();
  await expect(page.getByTestId("session-ended-pane")).toBeVisible();
  await expect(page.getByTestId("session-ended-pane")).toContainText("Session ended");
  await expect(page.getByTestId("session-ended-pane")).toContainText("code 7");
});
