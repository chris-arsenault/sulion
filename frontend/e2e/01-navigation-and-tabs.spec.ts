import { expect, test } from "@playwright/test";

import {
  gotoApp,
  openContextMenu,
  openSession,
  openTreeFile,
  restoreSeededSessionMetadata,
  selectMenuItem,
  tab,
  treeRow,
} from "./helpers";

test.afterEach(async ({ request }) => {
  await restoreSeededSessionMetadata(request);
});

test("opens seeded sessions, tabs, and preserves tab layout across reload", async ({ page }) => {
  await gotoApp(page);

  await expect(page.getByRole("button", { name: "Jump to atlas" })).toBeVisible();
  await expect(page.locator('[data-session-name="Atlas Claude"]')).toBeVisible();
  await expect(page.locator('[data-session-name="Atlas Codex"]')).toBeVisible();

  await page.getByRole("button", { name: "Open command palette" }).click();
  await page.getByPlaceholder("Type a command or jump to…").fill("Atlas Codex");
  await page.getByRole("option", { name: /Open session · atlas \/ Atlas Codex/i }).click();
  await expect(page.getByTestId("timeline-pane")).toBeVisible();

  await openSession(page, "Atlas Claude");
  await openTreeFile(page, "atlas", "data/config.json");
  await expect(page.getByTestId("file-tab")).toBeVisible();
  await expect(tab(page, "file", "config.json")).toBeVisible();

  await openContextMenu(treeRow(page, "atlas", "src/lib.rs"));
  await selectMenuItem(page, "Open diff");
  await expect(page.getByTestId("diff-tab")).toBeVisible();

  const dataTransfer = await page.evaluateHandle(() => new DataTransfer());
  const configTab = tab(page, "file", "config.json");
  const bottomDropZone = page.getByRole("button", { name: "Drop tab into bottom pane" });
  await configTab.dispatchEvent("dragstart", { dataTransfer });
  await bottomDropZone.dispatchEvent("dragover", { dataTransfer });
  await bottomDropZone.dispatchEvent("drop", { dataTransfer });
  await configTab.dispatchEvent("dragend", { dataTransfer });
  await expect(
    page.getByRole("tablist", { name: "bottom pane tabs" }).getByRole("tab", {
      name: /config\.json/i,
    }),
  ).toBeVisible();

  await page.reload();
  await expect(
    page.getByRole("tablist", { name: "bottom pane tabs" }).getByRole("tab", {
      name: /Atlas Claude · time/i,
    }),
  ).toBeVisible();
  await expect(
    page.getByRole("tablist", { name: "bottom pane tabs" }).getByRole("tab", {
      name: /config\.json/i,
    }),
  ).toBeVisible();
  await expect(page.getByTestId("diff-tab")).toBeVisible();
});

test("supports session rename, pin, and colour from the sidebar", async ({ page }) => {
  await gotoApp(page);

  const row = page.locator('[data-session-name="Atlas Claude"]');

  await row.dblclick();
  const input = page.getByLabel("Session name");
  await input.fill("Atlas Claude Temp");
  await input.press("Enter");
  await expect(page.locator('[data-session-name="Atlas Claude Temp"]')).toBeVisible();

  const renamed = page.locator('[data-session-name="Atlas Claude Temp"]');
  await openContextMenu(renamed);
  await selectMenuItem(page, "Pin to top");
  await expect(renamed.getByLabel("pinned")).toBeVisible();

  await openContextMenu(renamed);
  await page.getByRole("menuitem", { name: "Colour" }).hover();
  await page.getByRole("menuitem", { name: "teal" }).click();
  await expect(renamed.locator("xpath=ancestor::div[contains(@class, 'sidebar__row')]")).toHaveClass(
    /sidebar__row--color-teal/,
  );

  await openContextMenu(renamed);
  await selectMenuItem(page, "Unpin");
  await openContextMenu(renamed);
  await page.getByRole("menuitem", { name: "Colour" }).hover();
  await page.getByRole("menuitem", { name: "None" }).click();

  await renamed.dblclick();
  await page.getByLabel("Session name").fill("Atlas Claude");
  await page.getByLabel("Session name").press("Enter");
  await expect(page.locator('[data-session-name="Atlas Claude"]')).toBeVisible();
});
