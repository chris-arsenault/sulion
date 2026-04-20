import { expect, test } from "@playwright/test";

import { gotoApp, openSession, openTreeFile, tab, visibleTimelinePane } from "./helpers";

test("walks timeline details, filters, hover cards, and file trace links", async ({ page }) => {
  await gotoApp(page);
  await openSession(page, "Atlas Claude");

  const timelinePane = visibleTimelinePane(page);
  const firstTurn = timelinePane.getByTestId("turn-row").first();
  await firstTurn.click();
  await expect(page.getByLabel("Prompt actions")).toContainText("PROMPT_TIMELINE_SENTINEL");

  await page.getByLabel("View thinking").click();
  await expect(page.getByTestId("thinking-flyout")).toContainText(
    "Need to confirm the old implementation first.",
  );
  await page.getByLabel("Close thinking").click();

  const fileFilter = page.getByLabel("Filter to turns referencing file path");
  await fileFilter.fill("missing.txt");
  await expect(page.getByText("No turns match current filters.")).toBeVisible();
  await page.getByRole("button", { name: "Show all" }).click();
  await expect(firstTurn).toBeVisible();

  const readTool = page.locator('[data-testid="tool-pair-row"][data-tool-type="read"]');
  await readTool.hover();
  await expect(page.getByTestId("tool-hover-card")).toContainText("src/lib.rs");
  await expect(page.getByTestId("tool-hover-card")).toContainText('"old"');

  await openTreeFile(page, "atlas", "src/lib.rs");
  await expect(page.getByTestId("file-tab")).toBeVisible();
  await expect(page.getByLabel("Related timeline turns")).toContainText("inspect");
  await page.locator(".ft__trace-button").filter({ hasText: "inspect" }).first().click();
  const focusedTimelinePane = visibleTimelinePane(page);
  await expect(focusedTimelinePane).toBeVisible();
  await expect(
    focusedTimelinePane.locator('[data-testid="turn-row"][aria-pressed="true"]').first(),
  ).toBeVisible();

  await openTreeFile(page, "atlas", "data/config.json");
  await expect(tab(page, "file", "config.json")).toBeVisible();
  const activeFileTab = page.locator('[data-testid="file-tab"]:visible');
  await expect(activeFileTab).toContainText("mode");
  await activeFileTab.getByRole("button", { name: "raw" }).click();
  await expect(activeFileTab).toContainText('"seeded"');
});
