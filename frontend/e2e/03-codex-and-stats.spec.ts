import { expect, test } from "@playwright/test";

import { gotoApp, openSession } from "./helpers";

test("uses unified app-state for ambient status without legacy poll endpoints", async ({
  page,
}) => {
  const appStateRequests: string[] = [];
  const legacyPollRequests: string[] = [];
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (url.pathname === "/api/app-state") {
      appStateRequests.push(url.pathname);
    }
    if (
      (url.pathname === "/api/sessions" && request.method() === "GET") ||
      (url.pathname === "/api/repos" && request.method() === "GET") ||
      url.pathname === "/api/stats" ||
      /^\/api\/repos\/[^/]+\/git$/.test(url.pathname)
    ) {
      legacyPollRequests.push(`${request.method()} ${url.pathname}`);
    }
  });

  await gotoApp(page);
  await expect.poll(() => appStateRequests.length).toBeGreaterThan(0);
  await page.waitForTimeout(2500);
  expect(legacyPollRequests).toEqual([]);
});

test("covers codex lineage drill-down and stats visibility", async ({ page }) => {
  await gotoApp(page);

  await expect(page.getByTestId("stats-strip")).toBeVisible();
  await page.getByRole("button", { name: "Toggle stats details" }).click();
  await expect(page.getByTestId("stats-strip")).toContainText("db size");
  await expect(page.getByTestId("stats-strip")).toContainText("events");
  await expect(page.getByTestId("stats-strip")).toContainText("agent sessions");

  await openSession(page, "Atlas Codex");
  await page.getByTestId("turn-row").first().click();

  const taskTool = page.locator('[data-testid="tool-pair-row"][data-tool-type="task"]');
  await expect(taskTool).toBeVisible();
  await page.getByRole("button", { name: /View agent log/i }).click();
  await expect(page.getByTestId("subagent-modal")).toBeVisible();
  await expect(page.getByTestId("subagent-modal")).toContainText("No edits made.");
});
