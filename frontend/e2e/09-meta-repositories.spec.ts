import { expect, test } from "@playwright/test";

import {
  deleteSessionById,
  expectTerminalToContain,
  gotoApp,
  openSession,
  runTerminalCommand,
} from "./helpers";

let metaRepoId: string | null = null;
let sessionId: string | null = null;

test.afterEach(async ({ request }) => {
  if (sessionId) {
    await deleteSessionById(request, sessionId);
    sessionId = null;
  }
  if (metaRepoId) {
    const response = await request.delete(`/api/meta-repos/${metaRepoId}`);
    expect(response.ok()).toBeTruthy();
    metaRepoId = null;
  }
});

test("launches and restores a session for a meta-repository", async ({
  page,
  request,
}) => {
  const createMetaRepo = await request.post("/api/meta-repos", {
    data: {
      name: "E2E collection",
      members: ["atlas", "zephyr"],
      primary_repo_name: "atlas",
    },
  });
  expect(createMetaRepo.ok()).toBeTruthy();
  metaRepoId = ((await createMetaRepo.json()) as { id: string }).id;

  await gotoApp(page);
  const metaGroup = page.locator(`[data-meta-repo-id="${metaRepoId}"]`);
  await expect(metaGroup.getByText("E2E collection")).toBeVisible();
  await expect(metaGroup.locator('[data-repo-name="atlas"]')).toBeVisible();
  await expect(metaGroup.locator('[data-repo-name="zephyr"]')).toBeVisible();
  await expect(page.getByRole("button", { name: "Jump to E2E collection" })).toBeVisible();

  await page.getByLabel("New session in E2E collection").click();
  const createSessionResponse = page.waitForResponse(
    (response) =>
      response.url().endsWith("/api/sessions") &&
      response.request().method() === "POST",
  );
  await page.getByRole("button", { name: "Spawn" }).click();
  const response = await createSessionResponse;
  expect(response.ok()).toBeTruthy();
  sessionId = ((await response.json()) as { id: string }).id;
  expect(response.request().postDataJSON()).toMatchObject({
    meta_repo_id: metaRepoId,
    workspace_mode: "main",
  });

  const patch = await request.patch(`/api/sessions/${sessionId}`, {
    data: { label: "E2E collection shell" },
  });
  expect(patch.ok()).toBeTruthy();
  await expect(metaGroup.locator('[data-session-name="E2E collection shell"]')).toBeVisible({
    timeout: 10_000,
  });

  await openSession(page, "E2E collection shell");
  await runTerminalCommand(page, "printenv SULION_REPO_NAMES_JSON");
  await expectTerminalToContain(page, '["atlas","zephyr"]');
  await runTerminalCommand(page, "sulion-agent --type codex --mode mock --");
  await expectTerminalToContain(page, "Type a prompt and press Enter.");
  await runTerminalCommand(page, "verify collection session");
  await expectTerminalToContain(page, "wrote");
  await expect
    .poll(async () => page.getByTestId("turn-row").count(), { timeout: 20_000 })
    .toBeGreaterThan(0);

  await page.reload();
  const restoredGroup = page.locator(`[data-meta-repo-id="${metaRepoId}"]`);
  await expect(restoredGroup.locator(".sidebar__meta-name")).toHaveText(
    "E2E collection",
  );
  await expect(
    restoredGroup.locator('[data-session-name="E2E collection shell"]'),
  ).toBeVisible();
});
