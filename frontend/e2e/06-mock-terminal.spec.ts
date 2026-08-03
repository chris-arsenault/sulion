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
  await pasteIntoTerminal(page, largePaste);
  // Large pastes park behind the in-app ConfirmDialog (not a native browser
  // dialog): choose the save-as-file path.
  await page.getByRole("button", { name: "Save as file" }).click();
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

test("uploads a file larger than nginx's default body limit", async ({ request }) => {
  const payload = Buffer.alloc(2 * 1024 * 1024, 0x20);
  payload.write("%PDF-1.7\n", 0, "ascii");

  const upload = await request.post("/api/repos/atlas/upload?path=.upload-test", {
    multipart: {
      file: {
        name: "proxy-limit.pdf",
        mimeType: "application/pdf",
        buffer: payload,
      },
    },
  });
  expect(upload.ok(), await upload.text()).toBeTruthy();

  const raw = await request.get(
    "/api/repos/atlas/file/raw?path=.upload-test%2Fproxy-limit.pdf",
  );
  expect(raw.ok()).toBeTruthy();
  expect((await raw.body()).equals(payload)).toBe(true);
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
